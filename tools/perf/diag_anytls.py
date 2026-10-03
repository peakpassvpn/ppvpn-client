#!/usr/bin/env python3
"""Diagnostics for a protocol's throughput, Go and Rust in turn (#193):
not a tier B metric, a look at where a gap comes from. On Linux, as root
(perf and strace attach to the engine).

    diag_anytls.py --engine go=BIN --engine rust=BIN --fakenode BIN --loadgen BIN
                   --engine-cpus L --load-cpus L --node-cpus L [--node anytls] [--rounds N]

For each engine, each direction (up: to fakenode's discard port; down: from
its source port) and each count of connections (1, 2, 4, 8, 16), one
unpaced run prints a DIAG line: Mbit/s, the most TCP connections the engine
held to the node's port meanwhile, and its busiest threads' CPU. At 8
connections two more runs per direction:
  SYSCALLS  the engine's write/read-family system calls (perf's syscall
            tracepoints), per GiB moved;
  WRITES    the sizes its writes returned (strace -yy, 3 s), apart for the
            sockets to the node and those to the client: how the bytes are
            cut before they leave.
"""

import argparse
import collections
import json
import os
import re
import shutil
import subprocess
import sys
import tempfile
import threading
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure as m  # noqa: E402

COUNTS = (1, 2, 4, 8, 16)
CALLS = ("write", "writev", "sendto", "sendmsg", "read", "readv", "recvfrom", "recvmsg")
BUCKETS = (64, 256, 1024, 4096, 16384, 65536)


def threads(pid):
    out = {}
    for t in os.listdir(f"/proc/{pid}/task"):
        try:
            with open(f"/proc/{pid}/task/{t}/stat") as f:
                raw = f.read()
            fields = raw.rsplit(")", 1)[1].split()
            out[t] = (raw[raw.index("(") + 1:raw.rindex(")")], int(fields[11]) + int(fields[12]))
        except (FileNotFoundError, ValueError):
            pass
    return out


def sessions(port):
    out = subprocess.run(["ss", "-Htn", "state", "established", f"( dport = :{port} )"],
                         capture_output=True, text=True).stdout
    return len([line for line in out.splitlines() if line.strip()])


def load(args, credential, ports, direction, conns, seconds):
    target = ports["discard_port"] if direction == "up" else ports["source_port"]
    command = [args.loadgen, "-target", f"127.0.0.1:{target}", "-mode", "stream", "-direction", direction,
               "-conns", str(conns), "-duration", f"{seconds}s",
               "-proxy", f"{credential['listen']}:{credential['port']}",
               "-user", credential["username"], "-pass", credential["password"]]
    result = json.loads(subprocess.run(m.pinned(args.load_cpus, command), check=True,
                                       capture_output=True, text=True).stdout)
    moved = result["bytes_sent"] if direction == "up" else result["bytes_received"]
    return moved, result["elapsed_ms"]


def run(args, engine, credential, ports, direction, conns):
    hz = os.sysconf("SC_CLK_TCK")
    before = threads(engine.process.pid)
    peak, stop = [0], threading.Event()

    def watch():
        while not stop.wait(1):
            peak[0] = max(peak[0], sessions(ports[f"{args.node}_port"]))
    watcher = threading.Thread(target=watch)
    watcher.start()
    started = time.monotonic()
    moved, elapsed_ms = load(args, credential, ports, direction, conns, args.seconds)
    elapsed = time.monotonic() - started
    stop.set()
    watcher.join()
    per = []
    for t, (name, ticks) in threads(engine.process.pid).items():
        used = ticks - before.get(t, (name, 0))[1]
        if used:
            per.append([round(100 * used / hz / elapsed, 1), name])
    per.sort(reverse=True)
    return {"mbit": round(moved * 8 / elapsed_ms / 1000, 1), "tcp_to_node": peak[0],
            "engine_cpu_pct": round(sum(p for p, _ in per), 1), "threads": per[:5]}


def syscalls(args, engine, credential, ports, direction):
    events = ",".join(f"syscalls:sys_enter_{c}" for c in CALLS)
    perf = subprocess.Popen(["perf", "stat", "-x", ",", "-e", events, "-p", str(engine.process.pid),
                             "--", "sleep", str(args.seconds)], stderr=subprocess.PIPE, text=True)
    moved, _ = load(args, credential, ports, direction, 8, args.seconds)
    counts = {}
    for line in perf.communicate()[1].splitlines():
        parts = line.split(",")
        if len(parts) > 2 and parts[2].startswith("syscalls:sys_enter_") and parts[0].isdigit():
            counts[parts[2].removeprefix("syscalls:sys_enter_")] = int(parts[0])
    gib = moved / 2**30
    return {"gib": round(gib, 2), "per_gib": {c: round(n / gib) for c, n in counts.items() if gib}}


FD = re.compile(r"^\d+\s+(\w+)\(\d+<TCP:\[[^\]]*?:(\d+)->[^\]]*?:(\d+)\]>.*=\s+(\d+)$")


def writes(args, engine, credential, ports, direction, proxy_port):
    out = tempfile.mktemp(prefix="strace-")
    strace = subprocess.Popen(["timeout", "3", "strace", "-f", "-yy", "-s", "0", "-o", out,
                               "-e", "trace=write,writev,sendto,sendmsg", "-p", str(engine.process.pid)],
                              stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
    load(args, credential, ports, direction, 8, 5)
    strace.wait()
    hist = {"to_node": collections.Counter(), "to_client": collections.Counter()}
    node_port = ports[f"{args.node}_port"]
    with open(out) as f:
        for line in f:
            match = FD.match(line.strip())
            if not match:
                continue
            _, local, remote, size = match.groups()
            side = "to_node" if int(remote) == node_port else "to_client" if int(local) == proxy_port else None
            if side:
                n = int(size)
                hist[side][next((f"<={b}" for b in BUCKETS if n <= b), f">{BUCKETS[-1]}")] += 1
    os.unlink(out)
    order = [f"<={b}" for b in BUCKETS] + [f">{BUCKETS[-1]}"]
    return {side: {k: c[k] for k in order if c[k]} for side, c in hist.items()}


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--engine", action="append", required=True, metavar="NAME=BIN")
    p.add_argument("--fakenode", required=True)
    p.add_argument("--loadgen", required=True)
    p.add_argument("--engine-cpus")
    p.add_argument("--load-cpus")
    p.add_argument("--node-cpus")
    p.add_argument("--node", default="anytls", choices=m.PROFILE_NODES)
    p.add_argument("--rounds", type=int, default=2)
    p.add_argument("--seconds", type=int, default=10)
    args = p.parse_args()
    engines = [e.partition("=")[::2] for e in args.engine]
    for rnd in range(1, args.rounds + 1):
        work = tempfile.mkdtemp(prefix="diag-")
        node = subprocess.Popen(m.pinned(args.node_cpus, [args.fakenode, "-dir", work]),
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            ports = json.loads(node.stdout.readline())
            env = dict(os.environ, SSL_CERT_FILE=ports["certificate"])
            for name, binary in engines:
                engine = m.Engine(binary, work, f"{name}-{rnd}", ["--tun=false", "--local-proxy=true"], env=env,
                                  wrap=lambda c: m.pinned(args.engine_cpus, c))
                try:
                    engine.call("apply-profile", {"profile": m.profile("perf#1", ports)})
                    engine.call("start")
                    time.sleep(3)
                    credential = engine.call("get-local-proxy-credential", {"node_id": args.node})
                    for direction in ("up", "down"):
                        for conns in COUNTS:
                            row = run(args, engine, credential, ports, direction, conns)
                            row.update(engine=name, round=rnd, direction=direction, conns=conns)
                            print("DIAG " + json.dumps(row), flush=True)
                            time.sleep(2)
                        tag = {"engine": name, "round": rnd, "direction": direction, "conns": 8}
                        print("SYSCALLS " + json.dumps({**tag, **syscalls(args, engine, credential, ports, direction)}),
                              flush=True)
                        print("WRITES " + json.dumps({**tag, **writes(args, engine, credential, ports, direction,
                                                                      int(credential["port"]))}), flush=True)
                finally:
                    engine.stop()
        finally:
            node.stdin.close()
            node.terminate()
            node.wait(10)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
