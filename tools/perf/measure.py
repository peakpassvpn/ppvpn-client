#!/usr/bin/env python3
"""Measures an engine for the performance checks' tier A, on Linux.

    measure.py --engine-bin BIN --fakenode BIN --loadgen BIN [--rounds N]

The engine is a process that serves Core API v1 on a Unix socket with the
`ppvpn-core serve` command line and log format: ppvpn-core itself, or the
Rust engine's lab binary later. Nothing here links to either: the engine is
started, driven over the API, and measured from outside (/proc and its
log), so the same script measures both.

Each round prints one `PERF {...}` line per instance (see report.py):

  standard (local proxy, no TUN):
    idle_rss_kb                    VmRSS after apply + start + 10 s idle
    <proto>.stream_peak_rss_kb     VmHWM during 8 connections, 50 Mbit/s
                                   in total, 30 s (echoed: as much back)
    <proto>.stream_after_rss_kb    VmRSS 30 s after that load
    <proto>.churn_peak_rss_kb      VmHWM during 200 new connections/s,
                                   4 KiB each way, 30 s
    <proto>.churn_after_rss_kb     VmRSS 30 s after that load
    apply_total_ms                 an apply-profile call that switches
                                   kernels (new revision, rules changed)
    kernel_switch_ms               the same apply's "apply timing" line
  tun (TUN, no local proxy; root only, in its own network namespace):
    idle_rss_kb

<proto> is ss (Shadowsocks 2022) or anytls, both against tools' fakenode on
loopback; nothing leaves the host.
"""

import argparse
import http.client
import json
import os
import re
import shutil
import socket
import subprocess
import sys
import tempfile
import time

PROFILE_NODES = ("ss", "anytls")


class UnixHTTPConnection(http.client.HTTPConnection):
    def __init__(self, path, timeout=30):
        super().__init__("localhost", timeout=timeout)
        self.path = path

    def connect(self):
        self.sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
        self.sock.settimeout(self.timeout)
        self.sock.connect(self.path)


class Engine:
    """One engine process serving Core API v1."""

    def __init__(self, binary, work, name, extra, netns=None, env=None):
        self.work = os.path.join(work, name)
        os.makedirs(os.path.join(self.work, "state"), mode=0o700)
        self.socket = os.path.join(self.work, "core.sock")
        self.secret_file = os.path.join(self.work, "session.secret")
        self.log_path = os.path.join(self.work, "core.log")
        command = [binary, "serve", "--socket", self.socket, "--session-secret-file", self.secret_file,
                   "--state-dir", os.path.join(self.work, "state"), "--platform", "linux",
                   "--log-file", self.log_path, *extra]
        if netns:
            # ip netns exec execs the command: the child's pid is the engine's.
            command = ["ip", "netns", "exec", netns, *command]
        self.process = subprocess.Popen(command, stdin=subprocess.PIPE, env=env)
        deadline = time.time() + 20
        while not (os.path.exists(self.socket) and os.path.exists(self.secret_file)
                   and os.path.getsize(self.secret_file) > 0):
            if self.process.poll() is not None or time.time() > deadline:
                raise RuntimeError(f"{name}: the engine did not come up; see {self.log_path}")
            time.sleep(0.05)
        with open(self.secret_file) as f:
            self.secret = f.read().strip()

    def call(self, method, body=None):
        conn = UnixHTTPConnection(self.socket)
        payload = json.dumps(body or {})
        conn.request("POST", f"/v1/{method}", payload, {
            "Authorization": f"Bearer {self.secret}", "X-Core-API-Version": "1", "Content-Type": "application/json"})
        response = json.loads(conn.getresponse().read())
        conn.close()
        if not response.get("ok"):
            raise RuntimeError(f"{method}: {response.get('error')}")
        return response.get("data")

    def status(self, field):
        with open(f"/proc/{self.process.pid}/status") as f:
            for line in f:
                if line.startswith(field + ":"):
                    return int(line.split()[1])
        raise RuntimeError(f"no {field}")

    def reset_peak(self):
        # Resets VmHWM to the current RSS (Linux 4.0+).
        with open(f"/proc/{self.process.pid}/clear_refs", "w") as f:
            f.write("5")

    def log_field(self, message, field):
        value = None
        with open(self.log_path) as f:
            for line in f:
                if f'msg="{message}"' in line or f"msg={message} " in line:
                    match = re.search(rf"\b{field}=(\d+)", line)
                    if match:
                        value = int(match.group(1))
        return value

    def stop(self):
        try:
            self.call("stop")
        except Exception:
            pass
        self.process.stdin.close()
        self.process.terminate()
        try:
            self.process.wait(10)
        except subprocess.TimeoutExpired:
            self.process.kill()


def profile(revision, ports, rules_variant=0):
    """The measured profile: one Shadowsocks 2022 and one AnyTLS node on
    loopback (domain localhost), a few rules; rules_variant changes the
    rules, so an apply of it switches kernels."""
    nodes = [
        {"id": "ss", "name": "SS", "entry_key": "perf", "capabilities": {"tcp": True, "udp": True},
         "ingresses": [{"role": "primary", "endpoint_key": "ss-1", "replica_ordinal": 0, "protocol": "shadowsocks",
                        "endpoint": {"domain": "localhost", "port": ports["ss_port"]},
                        "credentials": {"shadowsocks": {"method": "2022-blake3-aes-128-gcm",
                                                        "user_key": "AAAAAAAAAAAAAAAAAAAAAA=="}},
                        "capabilities": {"tcp": True, "udp": True}}]},
        {"id": "anytls", "name": "AnyTLS", "entry_key": "perf", "capabilities": {"tcp": True, "udp": False},
         "ingresses": [{"role": "primary", "endpoint_key": "anytls-1", "replica_ordinal": 0, "protocol": "anytls",
                        "endpoint": {"domain": "localhost", "port": ports["anytls_port"]},
                        "credentials": {"anytls": {"password": "perf-anytls-password"}},
                        "tls": {"server_name": "localhost"},
                        "capabilities": {"tcp": True, "udp": False}}]},
    ]
    rules = [
        {"id": "direct-example", "match": {"domain_suffixes": [f"direct{rules_variant}.example"]}, "action": {"type": "direct"}},
        {"id": "reject-example", "match": {"domain_suffixes": ["blocked.example"]}, "action": {"type": "reject"}},
    ]
    return {"schema_version": 1, "revision": revision, "generated_at": "2026-01-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z", "nodes": nodes,
            "selection": {"mode": "manual", "default_node_id": "ss"},
            "routing": {"rules": rules, "final": {"type": "proxy", "target": "selected"}}}


def run_load(args, engine, credential, ports, mode):
    command = [args.loadgen, "-proxy", f"{credential['listen']}:{credential['port']}",
               "-user", credential["username"], "-pass", credential["password"],
               "-target", f"127.0.0.1:{ports['sink_port']}", "-mode", mode, "-duration", f"{args.load_seconds}s"]
    engine.reset_peak()
    out = subprocess.run(command, check=True, capture_output=True, text=True).stdout
    result = json.loads(out)
    if result["failed_connections"]:
        raise RuntimeError(f"{mode}: {result['failed_connections']} connections failed: {result}")
    peak = engine.status("VmHWM")
    time.sleep(args.settle_seconds)
    return peak, engine.status("VmRSS")


def measure_standard(args, work, ports, env):
    engine = Engine(args.engine_bin, work, "standard", ["--tun=false", "--local-proxy=true"], env=env)
    try:
        engine.call("apply-profile", {"profile": profile("perf#1", ports)})
        engine.call("start")
        time.sleep(args.idle_seconds)
        row = {"profile": "standard", "idle_rss_kb": engine.status("VmRSS")}
        for node in PROFILE_NODES:
            credential = engine.call("get-local-proxy-credential", {"node_id": node})
            for mode in ("stream", "churn"):
                peak, after = run_load(args, engine, credential, ports, mode)
                row[f"{node}.{mode}_peak_rss_kb"] = peak
                row[f"{node}.{mode}_after_rss_kb"] = after
        started = time.monotonic()
        engine.call("apply-profile", {"profile": profile("perf#2", ports, rules_variant=1)})
        row["apply_total_ms"] = round((time.monotonic() - started) * 1000)
        switch = engine.log_field("apply timing", "kernel_switch_ms")
        if switch is not None:
            row["kernel_switch_ms"] = switch
        return row
    finally:
        engine.stop()


def measure_tun(args, work):
    netns = f"perf{os.getpid()}"
    setup = [
        ["ip", "netns", "add", netns],
        ["ip", "netns", "exec", netns, "ip", "link", "set", "lo", "up"],
        ["ip", "netns", "exec", netns, "ip", "link", "add", "dummy0", "type", "dummy"],
        ["ip", "netns", "exec", netns, "ip", "addr", "add", "10.200.0.2/24", "dev", "dummy0"],
        ["ip", "netns", "exec", netns, "ip", "link", "set", "dummy0", "up"],
        ["ip", "netns", "exec", netns, "ip", "route", "add", "default", "via", "10.200.0.1"],
    ]
    try:
        for command in setup:
            subprocess.run(command, check=True)
        engine = Engine(args.engine_bin, work, "tun", ["--tun", "--local-proxy=false"], netns=netns)
        try:
            # Ports are not dialled while idle: any loopback values do.
            engine.call("apply-profile", {"profile": profile("perf#1", {"ss_port": 1, "anytls_port": 2})})
            engine.call("start")
            time.sleep(args.idle_seconds)
            return {"profile": "tun", "idle_rss_kb": engine.status("VmRSS")}
        finally:
            engine.stop()
    finally:
        subprocess.run(["ip", "netns", "del", netns], check=False)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--engine-bin", required=True)
    parser.add_argument("--fakenode", required=True)
    parser.add_argument("--loadgen", required=True)
    parser.add_argument("--rounds", type=int, default=1)
    parser.add_argument("--idle-seconds", type=float, default=10)
    parser.add_argument("--load-seconds", type=int, default=30)
    parser.add_argument("--settle-seconds", type=float, default=30)
    parser.add_argument("--skip-tun", action="store_true")
    args = parser.parse_args()
    if sys.platform != "linux":
        sys.exit("measure.py runs on Linux (/proc)")
    for round_number in range(1, args.rounds + 1):
        work = tempfile.mkdtemp(prefix="perf-")
        os.chmod(work, 0o700)
        fakenode = subprocess.Popen([args.fakenode, "-dir", work], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            ports = json.loads(fakenode.stdout.readline())
            # The engine trusts the fake node's certificate (AnyTLS) through
            # the system store's override.
            env = dict(os.environ, SSL_CERT_FILE=ports["certificate"])
            rows = [measure_standard(args, work, ports, env)]
            if not args.skip_tun:
                if os.geteuid() == 0:
                    rows.append(measure_tun(args, work))
                else:
                    print("measure.py: not root; the TUN instance is skipped", file=sys.stderr)
            for row in rows:
                row["round"] = round_number
                print("PERF " + json.dumps(row, sort_keys=True), flush=True)
        finally:
            fakenode.stdin.close()
            fakenode.terminate()
            fakenode.wait(10)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
