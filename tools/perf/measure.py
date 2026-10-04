#!/usr/bin/env python3
"""Measures an engine for the performance checks' tier A or B, on Linux.

    measure.py --engine-bin BIN --fakenode BIN --loadgen BIN [--rounds N]
    measure.py --tier b --engine NAME=BIN [--engine NAME=BIN ...]
               --fakenode BIN --loadgen BIN [--rounds N]
               [--engine-cpus LIST --load-cpus LIST --node-cpus LIST]
               [--label KEY=VALUE ...]
    measure.py --tier c --engine NAME=BIN [--engine NAME=BIN ...]
               --fakenode BIN --loadgen BIN [--rounds N] [--netem COND ...]
               [--engine-cpus LIST --load-cpus LIST --node-cpus LIST]
               [--label KEY=VALUE ...]

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

Tier B (#214's thresholds that depend on the machine: CPU, throughput,
latency), the standard instance only, each engine of --engine in turn in
every round (A B A B ...: Go and Rust measured as a pair, on one machine,
in one run), with a `PERF {...}` line per engine and round carrying its
name ("engine"):
    idle_cpu_pct                   the engine's CPU over 60 s idle (100 =
                                   one core), after apply + start + 10 s
    idle_switches_per_s            its context switches a second meanwhile
                                   (wakeups, roughly)
    direct_rtt_p50_us / _p99_us    64-byte round trips to the sink, not
                                   through the proxy: the baseline
    <proto>.tput1_mbit             one connection, unpaced, 20 s: Mbit/s
                                   echoed back
    <proto>.tput8_mbit             the same with 8 connections
    <proto>.tput<n>_engine_cpu_pct the CPU meanwhile of the engine, loadgen
    <proto>.tput<n>_load_cpu_pct   and fakenode (100 = one core each)
    <proto>.tput<n>_node_cpu_pct
    <proto>.tput<n>_load_limited   1 when the engine had CPU to spare (under
                                   80% of its CPUs) while loadgen or fakenode
                                   was near its limit (85% of theirs): the
                                   number is the load side's, not judged
    <proto>.cpu100_pct             the engine's CPU at 100 Mbit/s (8
                                   connections, paced), 20 s
    <proto>.rtt_p50_us / _p99_us   64-byte round trips through the proxy
    <proto>.extra_p50_us / _p99_us those less the direct ones: the proxy's
                                   and the node's extra latency
    <proto>.connect_p50_us         a new connection (20 a second, 20 s)
                                   until its first byte comes back
    <proto>.connect_p99_us
--part splits the run by what it needs, so that each part asks for the
cores it uses: light (idle, latency, connections, CPU at 100 Mbit/s; the
load and the fake node can share a core) and throughput (unpaced: the
engine, the load and the fake node each busy; ask for 8 CPUs and give the
engine a physical core, loadgen one and fakenode two, as the fake node is
the busiest). report.py merges the parts'
logs of one engine.
A first `ENV {...}` line says where: the CPU model, cores, memory, kernel,
the cores each part was pinned to, and --label's (the hostq job). The
engine, the load and the fake node run on the cores given (taskset), so
that they do not take each other's.

Tier C (#214's G4, a weak network): as tier B, each engine in turn, but
everything runs in a network namespace of its own (root), whose loopback
has a 1500-byte MTU and a netem qdisc for the packets to and from the
node's ports only, both ways: the engine-to-node link is impaired, the
load's link to the local proxy and the node's to the sink are not. One
level each (--netem picks; default all):
    none     nothing
    loss     1% of packets each way
    delay    50 ms each way (100 ms more round trip)
    jitter   50 ms +- 20 ms each way, normally distributed (reorders)
Per condition and engine, a `PERF {...}` line ("profile": "netem") with,
for each node, <cond>.<proto>.:
    rtt_p50_us / _p99_us           64-byte round trips through the proxy
    tput8_mbit                     8 connections, unpaced, echoed back
    connect_p50_us / _p99_us       new connections (20 a second) until
                                   their first byte comes back
    connect_failed                 new connections that failed
    stream_failed                  round-trip and throughput connections
                                   that failed
each load --c-load-seconds (20). Connections that fail are counted, not an
error. The ENV line says whether segmentation offload could be turned off
on the loopback (offload_off; ethtool): with it on, a lost packet can be
several segments.
"""

import argparse
import http.client
import json
import os
import re
import resource
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

    def __init__(self, binary, work, name, extra, netns=None, env=None, wrap=None):
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
        if wrap:
            # taskset execs the command too.
            command = wrap(command)
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


def cpu_ticks(pid):
    """utime + stime of pid, in clock ticks (all its threads)."""
    with open(f"/proc/{pid}/stat") as f:
        fields = f.read().rsplit(")", 1)[1].split()
    return int(fields[11]) + int(fields[12])


def switches(engine):
    """Context switches of all the engine's threads (/proc/<pid>/status
    counts its main thread only)."""
    total = 0
    for task in os.listdir(f"/proc/{engine.process.pid}/task"):
        try:
            with open(f"/proc/{engine.process.pid}/task/{task}/status") as f:
                for line in f:
                    if line.startswith(("voluntary_ctxt_switches:", "nonvoluntary_ctxt_switches:")):
                        total += int(line.split()[1])
        except FileNotFoundError:
            pass  # a thread that ended meanwhile
    return total


def pinned(cpus, command):
    return ["taskset", "-c", cpus, *command] if cpus else command


def loadgen(args, credential, ports, mode, *extra, failures_ok=False):
    """One loadgen run through the proxy (credential None: direct).
    failures_ok: failed connections are counted, not an error (tier C)."""
    command = [args.loadgen, "-target", f"127.0.0.1:{ports['sink_port']}", "-mode", mode, *extra]
    if credential is None:
        command.append("-direct")
    else:
        command += ["-proxy", f"{credential['listen']}:{credential['port']}",
                    "-user", credential["username"], "-pass", credential["password"]]
    result = json.loads(subprocess.run(pinned(args.load_cpus, command), check=True,
                                       capture_output=True, text=True).stdout)
    if result["failed_connections"] and not failures_ok:
        raise RuntimeError(f"{mode}: {result['failed_connections']} connections failed: {result}")
    return result


def busy(engine, run):
    """run(), and the engine's CPU meanwhile, in percent of one core."""
    ticks, started = cpu_ticks(engine.process.pid), time.monotonic()
    result = run()
    used = (cpu_ticks(engine.process.pid) - ticks) / os.sysconf("SC_CLK_TCK")
    return result, round(100 * used / (time.monotonic() - started), 2)


def three_busy(engine, node_pid, run):
    """run() (a loadgen run), and the CPU meanwhile of the engine, of
    loadgen (the child run waited for) and of fakenode, in percent of one
    core each."""
    hz = os.sysconf("SC_CLK_TCK")
    engine_ticks, node_ticks = cpu_ticks(engine.process.pid), cpu_ticks(node_pid)
    children = resource.getrusage(resource.RUSAGE_CHILDREN)
    started = time.monotonic()
    result = run()
    elapsed = time.monotonic() - started
    after = resource.getrusage(resource.RUSAGE_CHILDREN)
    load = (after.ru_utime + after.ru_stime) - (children.ru_utime + children.ru_stime)
    pct = lambda seconds: round(100 * seconds / elapsed, 1)
    return result, (pct((cpu_ticks(engine.process.pid) - engine_ticks) / hz), pct(load),
                    pct((cpu_ticks(node_pid) - node_ticks) / hz))


# Throughput is the engine's only while the engine is what is busy: below
# ENGINE_SPARE percent of the CPUs it was given, with loadgen or fakenode at
# least at LOAD_BUSY percent of theirs, the number is the load side's and is
# not judged (report.py). Each share is of the process's own CPUs (one
# hyper-thread is 100, a whole core 200).
ENGINE_SPARE = 80
LOAD_BUSY = 85


def share(pct, cpus):
    """pct (100 = one CPU) as a percentage of the CPUs given (a taskset
    list), or of one CPU when none was given."""
    n = len(cpus.split(",")) if cpus else 1
    return 100 * pct / (100 * n)


def measure_tier_b(args, work, ports, env, name, binary, node_pid):
    """One engine's tier B row."""
    seconds = f"{args.b_load_seconds}s"
    engine = Engine(binary, work, f"b-{name}", ["--tun=false", "--local-proxy=true"], env=env,
                    wrap=lambda c: pinned(args.engine_cpus, c))
    try:
        engine.call("apply-profile", {"profile": profile("perf#1", ports)})
        engine.call("start")
        time.sleep(args.idle_seconds)
        row = {"profile": "tierb", "engine": name}
        light, heavy = args.part in ("all", "light"), args.part in ("all", "throughput")
        if light:
            before = switches(engine)
            _, row["idle_cpu_pct"] = busy(engine, lambda: time.sleep(args.b_idle_seconds))
            row["idle_switches_per_s"] = round((switches(engine) - before) / args.b_idle_seconds, 2)
            direct = loadgen(args, None, ports, "pingpong", "-duration", seconds)
            row["direct_rtt_p50_us"], row["direct_rtt_p99_us"] = direct["p50_us"], direct["p99_us"]
        for node in PROFILE_NODES:
            credential = engine.call("get-local-proxy-credential", {"node_id": node})
            for conns in (1, 8) if heavy else ():
                # With the engine's CPU meanwhile: near its cores' 100 per
                # core, the engine is the bottleneck; well below, the load
                # or the fake node is, and the number says less.
                result, (engine_pct, load_pct, node_pct) = three_busy(engine, node_pid, lambda: loadgen(
                    args, credential, ports, "stream", "-conns", str(conns), "-rate-mbit", "0", "-duration", seconds))
                key = f"{node}.tput{conns}"
                row[f"{key}_mbit"] = round(result["bytes_received"] * 8 / result["elapsed_ms"] / 1000, 1)
                row[f"{key}_engine_cpu_pct"], row[f"{key}_load_cpu_pct"], row[f"{key}_node_cpu_pct"] = \
                    engine_pct, load_pct, node_pct
                row[f"{key}_load_limited"] = int(
                    share(engine_pct, args.engine_cpus) < ENGINE_SPARE
                    and max(share(load_pct, args.load_cpus), share(node_pct, args.node_cpus)) >= LOAD_BUSY)
            if not light:
                continue
            _, row[f"{node}.cpu100_pct"] = busy(engine, lambda: loadgen(
                args, credential, ports, "stream", "-conns", "8", "-rate-mbit", "100", "-duration", seconds))
            rtt = loadgen(args, credential, ports, "pingpong", "-duration", seconds)
            row[f"{node}.rtt_p50_us"], row[f"{node}.rtt_p99_us"] = rtt["p50_us"], rtt["p99_us"]
            row[f"{node}.extra_p50_us"] = rtt["p50_us"] - direct["p50_us"]
            row[f"{node}.extra_p99_us"] = rtt["p99_us"] - direct["p99_us"]
            setup = loadgen(args, credential, ports, "connect", "-churn-rate", "20", "-duration", seconds)
            row[f"{node}.connect_p50_us"], row[f"{node}.connect_p99_us"] = setup["p50_us"], setup["p99_us"]
        return row
    finally:
        engine.stop()


# Tier C (G4): one level each of loss, latency and jitter, on the path
# between the engine and the node only. netem delays each direction, so the
# round trip gains twice the delay.
NETEM = {
    "none": [],
    "loss": ["loss", "1%"],
    "delay": ["delay", "50ms"],
    "jitter": ["delay", "50ms", "20ms", "distribution", "normal"],
}


def netem_namespace():
    """Runs this script again inside a network namespace of its own, as tier
    C changes the loopback's queueing; returns its exit code. Inside, does
    nothing and returns None."""
    if os.environ.get("PERF_NETEM_NETNS"):
        return None
    if os.geteuid() != 0:
        sys.exit("tier C needs root: it makes a network namespace and sets netem in it")
    netns = f"perfc{os.getpid()}"
    subprocess.run(["ip", "netns", "add", netns], check=True)
    try:
        # A real link's MTU, so that a lost packet is a segment's worth.
        for command in (["ip", "link", "set", "lo", "mtu", "1500"], ["ip", "link", "set", "lo", "up"]):
            subprocess.run(["ip", "netns", "exec", netns, *command], check=True)
        return subprocess.run(["ip", "netns", "exec", netns, sys.executable, *sys.argv],
                              env=dict(os.environ, PERF_NETEM_NETNS=netns)).returncode
    finally:
        subprocess.run(["ip", "netns", "del", netns], check=False)


def netem_setup(ports):
    """Sends the node ports' packets, both ways, through a netem qdisc on the
    namespace's loopback; the rest (loadgen to the local proxy, the node to
    the sink) is not impaired. Returns whether segmentation offload is off,
    as a lost offloaded packet is many segments."""
    offload_off = subprocess.run(["ethtool", "-K", "lo", "tso", "off", "gso", "off", "gro", "off"],
                                 capture_output=True).returncode == 0 if shutil.which("ethtool") else False
    tc = lambda *a: subprocess.run(["tc", *a], check=True)
    # Bands 1-3 take what prio's default priomap sends them; band 4 only the
    # filtered packets.
    tc("qdisc", "add", "dev", "lo", "root", "handle", "1:", "prio", "bands", "4")
    tc("qdisc", "add", "dev", "lo", "parent", "1:4", "handle", "40:", "netem")
    for port in (ports["ss_port"], ports["anytls_port"]):
        for side in ("dport", "sport"):
            tc("filter", "add", "dev", "lo", "parent", "1:", "protocol", "ip", "prio", "1",
               "u32", "match", "ip", side, str(port), "0xffff", "flowid", "1:4")
    return offload_off


def netem_set(condition):
    # An empty netem qdisc passes everything as it comes.
    subprocess.run(["tc", "qdisc", "change", "dev", "lo", "parent", "1:4", "handle", "40:", "netem",
                    *(NETEM[condition] or ["delay", "0ms"])], check=True)


def measure_tier_c(args, work, ports, env, name, binary, condition):
    """One engine's tier C row under one netem condition: per node, round
    trips, unpaced throughput over 8 connections, and new connections; with
    the connections that failed rather than stopping at them."""
    seconds = f"{args.c_load_seconds}s"
    engine = Engine(binary, work, f"c-{name}-{condition}", ["--tun=false", "--local-proxy=true"], env=env,
                    wrap=lambda c: pinned(args.engine_cpus, c))
    try:
        engine.call("apply-profile", {"profile": profile("perf#1", ports)})
        engine.call("start")
        time.sleep(args.idle_seconds)
        row = {"profile": "netem", "engine": name}
        for node in PROFILE_NODES:
            credential = engine.call("get-local-proxy-credential", {"node_id": node})
            key = f"{condition}.{node}"
            run = lambda mode, *extra: loadgen(args, credential, ports, mode, "-duration", seconds, *extra,
                                               failures_ok=True)
            rtt = run("pingpong")
            tput = run("stream", "-conns", "8", "-rate-mbit", "0")
            setup = run("connect", "-churn-rate", "20")
            row[f"{key}.tput8_mbit"] = round(tput["bytes_received"] * 8 / tput["elapsed_ms"] / 1000, 1)
            # No times when nothing completed: the failures say so.
            for metric, result in (("rtt", rtt), ("connect", setup)):
                for p in ("p50", "p99"):
                    if f"{p}_us" in result:
                        row[f"{key}.{metric}_{p}_us"] = result[f"{p}_us"]
            row[f"{key}.connect_failed"] = setup["failed_connections"]
            row[f"{key}.stream_failed"] = rtt["failed_connections"] + tput["failed_connections"]
        return row
    finally:
        engine.stop()


def plain(value):
    """A value report.py takes as an environment value."""
    return re.sub(r"[^A-Za-z0-9 ._:/+=,()-]", " ", str(value)).strip()[:120]


def environment(args):
    model = ""
    with open("/proc/cpuinfo") as f:
        for line in f:
            if line.startswith("model name"):
                model = line.split(":", 1)[1].strip()
                break
    with open("/proc/meminfo") as f:
        memory = int(f.readline().split()[1]) // 1024
    env = {"cpu_model": model, "cpus_online": os.cpu_count(), "memory_mib": memory,
           "kernel": os.uname().release, "engine_cpus": args.engine_cpus or "any",
           "load_cpus": args.load_cpus or "any", "node_cpus": args.node_cpus or "any",
           "date": time.strftime("%Y-%m-%d", time.gmtime())}
    for spec in args.label or []:
        key, _, value = spec.partition("=")
        env[key] = value
    return {key: plain(value) for key, value in env.items()}


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
    parser.add_argument("--tier", choices=("a", "b", "c"), default="a")
    parser.add_argument("--engine-bin", help="tier A: the engine")
    parser.add_argument("--engine", action="append", metavar="NAME=BIN",
                        help="tiers B and C: an engine, by name (go, rust); all of them each round, in turn")
    parser.add_argument("--engine-cpus", help="tiers B and C: the engine's cores (taskset -c)")
    parser.add_argument("--load-cpus", help="tiers B and C: loadgen's cores")
    parser.add_argument("--node-cpus", help="tier B: fakenode's cores")
    parser.add_argument("--label", action="append", metavar="KEY=VALUE",
                        help="tiers B and C: recorded in the ENV line (hostq_job=...)")
    parser.add_argument("--part", choices=("all", "light", "throughput"), default="all",
                        help="tier B: light (idle, latency, connections, CPU at 100 Mbit/s: two "
                             "physical cores do) or throughput (unpaced: each part its own cores)")
    parser.add_argument("--b-idle-seconds", type=float, default=60)
    parser.add_argument("--b-load-seconds", type=int, default=20)
    parser.add_argument("--netem", action="append", choices=tuple(NETEM),
                        help="tier C: the conditions (default: all of them)")
    parser.add_argument("--c-load-seconds", type=int, default=20)
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
    if args.tier == "a" and not args.engine_bin:
        parser.error("tier A needs --engine-bin")
    if args.tier in ("b", "c"):
        if not args.engine:
            parser.error(f"tier {args.tier.upper()} needs --engine NAME=BIN")
        engines = [spec.partition("=")[::2] for spec in args.engine]
        if any(not name or not binary for name, binary in engines):
            parser.error("--engine is NAME=BIN")
    if args.tier == "c":
        code = netem_namespace()
        if code is not None:
            sys.exit(code)
        conditions = args.netem or list(NETEM)
    if args.tier in ("b", "c"):
        env_line = environment(args)
        if args.tier == "b":
            env_line["part"] = args.part
            print("ENV " + json.dumps(env_line, sort_keys=True), flush=True)
        else:
            # Printed once whether offload is off is known.
            env_line["netem"] = ",".join(conditions)
    for round_number in range(1, args.rounds + 1):
        work = tempfile.mkdtemp(prefix="perf-")
        os.chmod(work, 0o700)
        node_cpus = args.node_cpus if args.tier in ("b", "c") else None
        fakenode = subprocess.Popen(pinned(node_cpus, [args.fakenode, "-dir", work]), stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        netem = False
        try:
            ports = json.loads(fakenode.stdout.readline())
            # The engine trusts the fake node's certificate (AnyTLS) through
            # the system store's override.
            env = dict(os.environ, SSL_CERT_FILE=ports["certificate"])
            if args.tier == "b":
                rows = [measure_tier_b(args, work, ports, env, name, binary, fakenode.pid)
                        for name, binary in engines]
            elif args.tier == "c":
                netem = True
                offload_off = netem_setup(ports)
                if round_number == 1:
                    env_line["offload_off"] = int(offload_off)
                    print("ENV " + json.dumps(env_line, sort_keys=True), flush=True)
                rows = []
                for condition in conditions:
                    netem_set(condition)
                    rows += [measure_tier_c(args, work, ports, env, name, binary, condition)
                             for name, binary in engines]
            else:
                rows = [measure_standard(args, work, ports, env)]
            if args.tier == "a" and not args.skip_tun:
                if os.geteuid() == 0:
                    rows.append(measure_tun(args, work))
                else:
                    print("measure.py: not root; the TUN instance is skipped", file=sys.stderr)
            for row in rows:
                row["round"] = round_number
                print("PERF " + json.dumps(row, sort_keys=True), flush=True)
        finally:
            if netem:
                subprocess.run(["tc", "qdisc", "del", "dev", "lo", "root"], check=False)
            fakenode.stdin.close()
            fakenode.terminate()
            fakenode.wait(10)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
