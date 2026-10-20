#!/usr/bin/env python3
"""Peak memory of ppvpn-core and sail in the iOS Network Extension's shape
(#388): ne-mem (crates/core-lab/src/bin/ne-mem.rs) holds the Engine as the
extension will, a TUN instance on a mobile platform with no local proxy, and
reports its footprint (phys_footprint on macOS, RssAnon on Linux) per
scenario.
As root: sail opens the TUN. Runs on macOS and Linux.

    sudo tools/perf/ne_mem.py --ne-mem BIN --fakenode BIN --loadgen BIN [--workers 1,0]
        [--variant NAME[=SETTINGS] ...] [--dhat PREFIX] [--out FILE]

A variant is a run with sail's start settings in PPVPN_LAB_SAIL_SETTINGS
(ne-mem built with ppvpn-core's `lab` feature): one of VARIANTS by name, or
NAME='{"profile":"desktop","set":["relay.buffer_size=8"]}'. `default`, the
first, is core's own options. Each variant runs once per --workers value.

--dhat (ne-mem built with its `dhat` feature) writes each run's heap
profile to PREFIX-<variant>-w<workers>.json; dhat's view of it takes the
file, and tools/perf/dhat_top.py sums it up. Its snapshot is at the heap's
peak, so --conns picks what it shows: '' for the downloads, 500 for the
connections.

The host's routes stay as they are but for one: TEST-NET-3 (203.0.113.0/24)
into the TUN, which the loads dial. fakenode (-redirect) sends the node's
dials to that range to its own sinks on loopback, so nothing loops and
nothing leaves the host, except: the rule sets, downloaded by the engine
from GitHub (the public sets the backend serves, about 1 MB in all), and the
DNS scenario's DoT queries, through the node to public resolvers.

Scenarios, in this order, each a MARK line with the peak since the previous
one (see ne-mem's doc):

  engine_new       Engine::new, nothing applied
  start            apply (the rule sets downloaded) and start
  idle             IDLE_SECONDS later
  conns<N>         N connections held open at once, echoing a little each
                   (--conns, 100, 250 and 500: the slope is the cost of one)
  download_<node>  8 connections downloading as fast as they go (unpaced)
  switch           a switch of the selected node during 8 streams
  dns              2000 queries for distinct names, through the TUN's DNS
  ruleset_refresh  a new profile revision whose rule sets are new files
  settled          SETTLE_SECONDS after the last load
  restart_warm     stop, then start again with the rule sets cached

Prints one NEMEM JSON line per scenario and a Markdown table at the end
(also appended to $GITHUB_STEP_SUMMARY when set). A scenario whose load
failed (a connection refused, an echo short, a download empty, DNS mostly
unanswered) is marked ✗: its numbers are not the load's. Exits 1 when one
of the first variant's did.
"""

import argparse
import hashlib
import json
import os
import platform
import shutil
import socket
import struct
import subprocess
import sys
import tempfile
import threading
import time
import urllib.request

TEST_NET = "203.0.113.0/24"
TARGET_IP = "203.0.113.10"
DNS_IP = "203.0.113.53"
IDLE_SECONDS = 30
SETTLE_SECONDS = 10
BUDGET_MIB = 35

RAW = "https://raw.githubusercontent.com"
# The rule sets of a production profile (ids as the backend names them) and,
# for the refresh, the files a later revision points to: larger ones where
# the sources have them, so the refresh is the heavy case.
RULE_SETS = {
    "home": (f"{RAW}/SagerNet/sing-geosite/rule-set/geosite-private.srs",) * 2,
    "ads": (f"{RAW}/SagerNet/sing-geosite/rule-set/geosite-category-ads-all.srs",
            f"{RAW}/SagerNet/sing-geosite/rule-set/geosite-category-ads.srs"),
    "cn-domain": (f"{RAW}/SagerNet/sing-geosite/rule-set/geosite-cn.srs",
                  f"{RAW}/chg1f/sing-geosite-mixed/rule-set/direct.srs"),
    "proxy-domain": (f"{RAW}/chg1f/sing-geosite-mixed/rule-set/proxy.srs",
                     f"{RAW}/SagerNet/sing-geosite/rule-set/geosite-geolocation-!cn.srs"),
    "cn-ip": (f"{RAW}/SagerNet/sing-geoip/rule-set/geoip-cn.srs",) * 2,
}

# sail's mobile profile in parts, over the desktop one core runs (sail's
# runtime/options.rs has the values and why).
_MOBILE_RELAY = ["relay.buffer_size=8", "relay.buffer_max_size=64", "relay.buffer_pool=256"]
VARIANTS = {
    "relay": {"profile": "desktop", "set": _MOBILE_RELAY},
    "window": {"profile": "desktop", "set": ["netstack.receive_window=4", "netstack.receive_window_max=128",
                                            "netstack.read_ahead=32"]},
    "budget": {"profile": "desktop", "set": ["netstack.budget=mobile"]},
    "mobile": {"profile": "mobile"},
    # Not the profile's: the sessions a 35 MiB budget carries at about 150
    # KiB each (#388); the dispatcher refuses the rest, the TUN's included.
    "cap": {"profile": "desktop", "set": ["inbound.max_connections=128"]},
}


def log(*parts):
    print(*parts, file=sys.stderr, flush=True)


def sha256_of(url):
    with urllib.request.urlopen(url, timeout=30) as r:
        body = r.read()
    return hashlib.sha256(body).hexdigest(), len(body)


def rule_sets(revision_index):
    out, sizes = [], {}
    for rid, urls in RULE_SETS.items():
        url = urls[revision_index]
        digest, size = sha256_of(url)
        sizes[rid] = size
        out.append({"id": rid, "url": url, "sha256": digest, "update_interval_seconds": 86400})
    return out, sizes


def profile(revision, ports, sets):
    """Two nodes on loopback (Shadowsocks 2022 and AnyTLS, the fake node's),
    a production profile's routing rules and rule sets."""
    def node(nid, protocol, port, credentials, tls=None, udp=True):
        ingress = {"role": "primary", "endpoint_key": f"{nid}-1", "replica_ordinal": 0, "protocol": protocol,
                   "endpoint": {"domain": "localhost", "port": port}, "credentials": credentials,
                   "capabilities": {"tcp": True, "udp": udp}}
        if tls:
            ingress["tls"] = tls
        return {"id": nid, "name": nid, "entry_key": "perf", "capabilities": {"tcp": True, "udp": udp},
                "ingresses": [ingress]}
    nodes = [
        node("ss", "shadowsocks", ports["ss_port"],
             {"shadowsocks": {"method": "2022-blake3-aes-128-gcm", "user_key": "AAAAAAAAAAAAAAAAAAAAAA=="}}),
        node("anytls", "anytls", ports["anytls_port"], {"anytls": {"password": "perf-anytls-password"}},
             tls={"server_name": "localhost", "insecure": True}, udp=False),
    ]
    rules = [
        {"id": "official-api", "match": {"domains": ["example.com"]}, "action": {"type": "direct"}, "baseline": True},
        {"id": "bypass-private", "match": {"ip_is_private": True}, "action": {"type": "direct"}, "baseline": True},
        {"id": "rs-home", "match": {"rule_set_ids": ["home"]}, "action": {"type": "direct"}},
        {"id": "rs-ads", "match": {"rule_set_ids": ["ads"]}, "action": {"type": "reject"}},
        {"id": "rs-cn-domain", "match": {"rule_set_ids": ["cn-domain"]}, "action": {"type": "direct"}},
        {"id": "rs-proxy-domain", "match": {"rule_set_ids": ["proxy-domain"]},
         "action": {"type": "proxy", "target": "selected"}},
        {"id": "geoip-cn", "match": {"rule_set_ids": ["cn-ip"]}, "action": {"type": "direct"}},
    ]
    return {"schema_version": 1, "revision": revision, "generated_at": "2026-01-01T00:00:00Z",
            "expires_at": "2099-01-01T00:00:00Z", "nodes": nodes,
            "selection": {"mode": "manual", "default_node_id": "anytls"},
            "routing": {"rule_sets": sets, "rules": rules, "final": {"type": "proxy", "target": "selected"}}}


class NeMem:
    def __init__(self, binary, work, workers, platform_name, variant, settings, dhat=None):
        self.state = os.path.join(work, "state")
        os.makedirs(self.state, mode=0o700, exist_ok=True)
        self.log_path = os.path.join(work, f"ne-mem-{variant}-w{workers}.log")
        self.dhat = dhat
        env = dict(os.environ)
        env.pop("PPVPN_LAB_SAIL_SETTINGS", None)
        if settings:
            env["PPVPN_LAB_SAIL_SETTINGS"] = settings
        self.proc = subprocess.Popen(
            [binary, "--state-dir", self.state, "--platform", platform_name,
             "--workers", str(workers), "--log-file", self.log_path,
             *(["--dhat-file", f"{dhat}-{variant}-w{workers}.json"] if dhat else [])],
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True, bufsize=1, env=env)
        ready = self.read()
        if not ready.get("ok"):
            raise RuntimeError(f"ne-mem: {ready}")

    def read(self):
        line = self.proc.stdout.readline()
        if not line:
            raise RuntimeError(f"ne-mem exited ({self.proc.poll()}); log: {self.log_path}")
        return json.loads(line)

    def call(self, cmd, **fields):
        self.proc.stdin.write(json.dumps({"cmd": cmd, **fields}) + "\n")
        self.proc.stdin.flush()
        out = self.read()
        if not out.get("ok"):
            raise RuntimeError(f"{cmd}: {out.get('error')}")
        return out

    def quit(self):
        try:
            self.proc.stdin.write('{"cmd":"quit"}\n')
            self.proc.stdin.flush()
            # dhat writes its profile at quit.
            self.proc.wait(timeout=180 if self.dhat else 20)
        except Exception:
            self.proc.kill()


def interfaces():
    if sys.platform == "darwin":
        return set(subprocess.run(["ifconfig", "-l"], capture_output=True, text=True).stdout.split())
    return set(os.listdir("/sys/class/net"))


def route_test_net(ifname):
    if sys.platform == "darwin":
        subprocess.run(["route", "-n", "add", "-net", TEST_NET, "-interface", ifname], check=True,
                       stdout=subprocess.DEVNULL)
    else:
        subprocess.run(["ip", "route", "replace", TEST_NET, "dev", ifname], check=True)


def unroute_test_net():
    if sys.platform == "darwin":
        subprocess.run(["route", "-n", "delete", "-net", TEST_NET], stdout=subprocess.DEVNULL,
                       stderr=subprocess.DEVNULL)
    else:
        subprocess.run(["ip", "route", "del", TEST_NET], stderr=subprocess.DEVNULL)


def wait_rule_sets(engine, timeout=60):
    deadline = time.time() + timeout
    while time.time() < deadline:
        sets = engine.call("status")["status"].get("rule_sets", [])
        if sets and all(s.get("state") == "ready" for s in sets):
            return sets
        time.sleep(0.5)
    raise RuntimeError(f"rule sets not ready: {sets}")


def loadgen(args, *extra, failures_ok=True):
    command = [args.loadgen, "-direct", *extra]
    out = subprocess.run(command, capture_output=True, text=True, timeout=600)
    if out.returncode != 0 and not failures_ok:
        raise RuntimeError(f"loadgen {extra}: {out.stderr.strip()}")
    try:
        return json.loads(out.stdout.strip().splitlines()[-1])
    except (IndexError, json.JSONDecodeError):
        return {"error": out.stderr.strip()[-300:]}


def dns_burst(count, rate=400):
    """count A queries for distinct names under example.net, through the
    TUN's DNS (any address port 53 is hijacked), rate a second; returns the
    answers that came back."""
    sock = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
    sock.settimeout(0.2)
    answered = 0
    stop = threading.Event()

    def receive():
        nonlocal answered
        while not stop.is_set():
            try:
                sock.recvfrom(4096)
                answered += 1
            except OSError:
                pass
    t = threading.Thread(target=receive, daemon=True)
    t.start()
    for i in range(count):
        name = f"n{i}-{os.getpid()}.example.net"
        q = struct.pack(">HHHHHH", i & 0xFFFF, 0x0100, 1, 0, 0, 0)
        q += b"".join(bytes([len(p)]) + p.encode() for p in name.split(".")) + b"\0"
        q += struct.pack(">HH", 1, 1)
        sock.sendto(q, (DNS_IP, 53))
        time.sleep(1 / rate)
    time.sleep(5)
    stop.set()
    t.join()
    sock.close()
    return answered


def failure(row):
    """Why the scenario's load failed, or None."""
    load = row.get("load")
    if load is not None:
        if "error" in load or load.get("failed_connections"):
            return f"{load.get('failed_connections', '?')} connections failed"
        if load.get("bytes_received", 0) == 0:
            return "nothing received"
        if load.get("bytes_sent") and load["bytes_received"] < load["bytes_sent"]:
            return f"echo short: {load['bytes_received']} of {load['bytes_sent']} bytes"
    if "answered" in row and row["answered"] < 0.95 * row["queries"]:
        return f"DNS: {row['answered']} of {row['queries']} answered"
    return None


def run(args, variant, settings, workers, ports, sets1, sets2, work):
    rows = []
    plat = "ios" if sys.platform == "darwin" else "android"
    before = interfaces()
    engine = NeMem(args.ne_mem, work, workers, plat, variant, settings, args.dhat)
    try:
        def mark(label, **extra):
            row = engine.call("mark", label=label)
            row.pop("ok", None)
            row["variant"] = variant
            row["workers"] = workers
            row.update(extra)
            row["failed"] = failure(row)
            print("NEMEM " + json.dumps(row), flush=True)
            rows.append(row)

        mark("engine_new")
        request = {"profile": profile("ne-mem#1", ports, sets1),
                   "allowed_rule_set_hosts": ["raw.githubusercontent.com"]}
        engine.call("apply", request=request)
        engine.call("start")
        wait_rule_sets(engine)
        new = sorted(interfaces() - before)
        if len(new) != 1:
            raise RuntimeError(f"want one new interface, got {new}")
        route_test_net(new[0])
        mark("start", tun=new[0])

        time.sleep(args.idle_seconds)
        mark("idle")

        target = f"{TARGET_IP}:{ports['sink_port']}"
        for conns in (int(c) for c in args.conns.split(",") if c):
            res = loadgen(args, "-target", target, "-mode", "stream", "-conns", str(conns), "-rate-mbit", "20",
                          "-duration", f"{args.load_seconds}s")
            mark(f"conns{conns}", load=res)

        for node in ("anytls", "ss"):
            engine.call("select", node=node)
            res = loadgen(args, "-target", f"{TARGET_IP}:{ports['source_port']}", "-mode", "stream",
                          "-direction", "down", "-conns", "8", "-rate-mbit", "0",
                          "-duration", f"{args.load_seconds}s")
            mark(f"download_{node}", load=res)

        holder = {}
        th = threading.Thread(target=lambda: holder.update(load=loadgen(
            args, "-target", target, "-mode", "stream", "-conns", "8", "-rate-mbit", "50",
            "-duration", "12s")))
        th.start()
        time.sleep(3)
        engine.call("select", node="anytls")
        time.sleep(3)
        engine.call("select", node="ss")
        time.sleep(3)
        engine.call("select", node="anytls")
        th.join()
        mark("switch", load=holder.get("load"))

        answered = dns_burst(args.dns_queries)
        mark("dns", queries=args.dns_queries, answered=answered)

        request = {"profile": profile("ne-mem#2", ports, sets2),
                   "allowed_rule_set_hosts": ["raw.githubusercontent.com"]}
        engine.call("apply", request=request)
        wait_rule_sets(engine)
        mark("ruleset_refresh")

        time.sleep(SETTLE_SECONDS)
        mark("settled")

        engine.call("stop")
        unroute_test_net()
        before = interfaces()
        engine.call("start")
        wait_rule_sets(engine)
        new = sorted(interfaces() - before)
        mark("restart_warm", tun=new[0] if new else "")
    finally:
        unroute_test_net()
        engine.quit()
    return rows


def table(rows, sizes):
    lines = [f"Budget: {BUDGET_MIB} MiB for ppvpn-core and sail. "
             f"Host: {platform.system()} {platform.machine()}, {os.cpu_count()} CPUs. "
             f"Rule sets (bytes, revision 1 / 2): "
             + ", ".join(f"{k} {sizes[0][k]}/{sizes[1][k]}" for k in RULE_SETS) + ".",
             "",
             "| variant | workers | scenario | peak MiB | after MiB | heap peak MiB | heap after MiB | threads "
             "| RSS peak / resident MiB | source | load |",
             "| --- | --- | --- | --- | --- | --- | --- | --- | --- | --- | --- |"]
    for r in rows:
        over = " ⚠" if r["peak_kib"] / 1024 > BUDGET_MIB else ""
        load = f"✗ {r['failed']}" if r.get("failed") else ""
        lines.append(f"| {r['variant']} | {r['workers'] or 'ncpu'} | {r['label']} | {r['peak_kib'] / 1024:.1f}{over} | "
                     f"{r['footprint_kib'] / 1024:.1f} | {r['heap_peak_kib'] / 1024:.1f} | "
                     f"{r['heap_live_kib'] / 1024:.1f} | {r['threads']} | "
                     f"{r.get('rss_peak_kib', r.get('resident_kib', 0)) / 1024:.1f} | {r['peak_source']} | {load} |")
    return "\n".join(lines)


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--ne-mem", required=True)
    parser.add_argument("--fakenode", required=True)
    parser.add_argument("--loadgen", required=True)
    parser.add_argument("--workers", default="1,0",
                        help="comma-separated tokio worker counts, one run each (0: one per CPU)")
    parser.add_argument("--idle-seconds", type=int, default=IDLE_SECONDS)
    parser.add_argument("--load-seconds", type=int, default=20)
    parser.add_argument("--dns-queries", type=int, default=2000)
    parser.add_argument("--conns", default="100,250,500")
    parser.add_argument("--variant", action="append", default=[], metavar="NAME=SETTINGS",
                        help="also run with these sail start settings (JSON), or VARIANTS[NAME]; repeatable")
    parser.add_argument("--dhat", metavar="PREFIX", help="ne-mem's heap profiles (its dhat feature) to PREFIX-*.json")
    parser.add_argument("--out", help="also write the rows as JSON here")
    args = parser.parse_args()
    variants = [("default", "")]
    for v in args.variant:
        name, _, settings = v.partition("=")
        settings = settings or json.dumps(VARIANTS[name])
        json.loads(settings)
        variants.append((name, settings))
    if os.geteuid() != 0:
        sys.exit("run as root: sail opens the TUN")

    sets1, sizes1 = rule_sets(0)
    sets2, sizes2 = rule_sets(1)
    work = tempfile.mkdtemp(prefix="ne-mem-")
    node = subprocess.Popen([args.fakenode, "-dir", work, "-redirect", TEST_NET],
                            stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    rows = []
    try:
        ports = json.loads(node.stdout.readline())
        for variant, settings in variants:
            for workers in (int(w) for w in args.workers.split(",")):
                # A fresh state directory each run: start downloads the rule sets.
                shutil.rmtree(os.path.join(work, "state"), ignore_errors=True)
                rows += run(args, variant, settings, workers, ports, sets1, sets2, work)
    finally:
        node.stdin.close()
        node.wait(timeout=10)
        for name in os.listdir(work):
            if name.endswith(".log"):
                dest = shutil.copy(os.path.join(work, name), os.getcwd())
                os.chmod(dest, 0o644)
        shutil.rmtree(work, ignore_errors=True)
    report = table(rows, (sizes1, sizes2))
    print(report)
    if os.environ.get("GITHUB_STEP_SUMMARY"):
        with open(os.environ["GITHUB_STEP_SUMMARY"], "a") as f:
            f.write("## ne-mem (#388)\n\n" + report + "\n")
    if args.out:
        with open(args.out, "w") as f:
            json.dump({"rows": rows, "rule_set_bytes": [sizes1, sizes2],
                       "variants": dict(variants)}, f, indent=1)
        os.chmod(args.out, 0o644)
    if any(r["failed"] for r in rows if r["variant"] == "default"):
        sys.exit("a load failed on the default variant (✗ above)")


if __name__ == "__main__":
    main()
