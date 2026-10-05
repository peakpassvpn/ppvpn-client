#!/usr/bin/env python3
"""The 24-hour run (#214's G6), on Linux, as root: one engine under a
repeating load, sampled from outside.

    soak.py run --engine NAME=BIN --fakenode BIN --loadgen BIN --out DIR
                [--hours 24] [--cycle-seconds 600] [--sample-seconds 30]
                [--apply-seconds 3600]
                [--engine-cpus LIST --load-cpus LIST --node-cpus LIST]
    soak.py summarize DIR [DIR ...]

`run` starts the engine (`ppvpn-core serve`'s command line, as measure.py)
in a network namespace of its own, so that two runs side by side (Go and
Rust) do not share ports, and drives it through its local proxy against
measure.py's fake node. Each cycle (--cycle-seconds):
    stream     8 connections per node, 20 Mbit/s in total, echoed, 180 s
    churn      50 new connections a second per node, 4 KiB each way, 60 s
    pingpong   one connection per node, 30 s
    idle       the rest of the cycle
both nodes (Shadowsocks 2022, AnyTLS) at once. Every hour (--apply-seconds)
an apply-profile that switches kernels (the rules change). Every --sample-seconds a line in
DIR/samples.csv: the time, the phase, VmRSS, VmHWM, open descriptors,
threads, the engine's TCP sockets (established and all) and its CPU
ticks. Each load's failed and stalled connections go into DIR/loads.csv.
The run stops early, with a FAILED line, if the engine exits.

`summarize` reads DIRs and prints, per run, what a leak would show: the
idle samples of the second hour against those of the last hour (RSS,
descriptors, sockets), the largest values, and the failures. Its numbers
go into #214; nothing here runs in CI.
"""

import argparse
import csv
import json
import os
import statistics
import subprocess
import sys
import tempfile
import threading
import time

import measure

STREAM_SECONDS, CHURN_SECONDS, PINGPONG_SECONDS = 180, 60, 30


def sockets(pid):
    """The engine's TCP sockets: (established, all), by the inodes of its
    descriptors in the namespace's /proc/net/tcp and tcp6."""
    inodes = set()
    for fd in os.listdir(f"/proc/{pid}/fd"):
        try:
            target = os.readlink(f"/proc/{pid}/fd/{fd}")
        except FileNotFoundError:
            continue
        if target.startswith("socket:["):
            inodes.add(target[8:-1])
    established = total = 0
    for table in ("tcp", "tcp6"):
        with open(f"/proc/{pid}/net/{table}") as f:
            next(f)
            for line in f:
                fields = line.split()
                if fields[9] in inodes:
                    total += 1
                    established += fields[3] == "01"
    return established, total


def sample(engine):
    pid = engine.process.pid
    established, total = sockets(pid)
    return {"rss_kb": engine.status("VmRSS"), "hwm_kb": engine.status("VmHWM"),
            "fds": len(os.listdir(f"/proc/{pid}/fd")), "threads": engine.status("Threads"),
            "tcp_established": established, "tcp_all": total, "cpu_ticks": measure.cpu_ticks(pid)}


class Phase:
    """The phase the load is in, for the samples."""

    def __init__(self):
        self.name = "start"
        self.lock = threading.Lock()

    def set(self, name):
        with self.lock:
            self.name = name

    def get(self):
        with self.lock:
            return self.name


def sampler(engine, phase, path, every, stop):
    started = time.monotonic()
    with open(path, "w", newline="") as f:
        out = None
        while not stop.wait(0 if out is None else every):
            if engine.process.poll() is not None:
                return
            row = {"seconds": round(time.monotonic() - started), "phase": phase.get(), **sample(engine)}
            if out is None:
                out = csv.DictWriter(f, fieldnames=list(row))
                out.writeheader()
            out.writerow(row)
            f.flush()


def loads(args, credentials, ports, mode, seconds, *extra):
    """mode on every node at once, for seconds; the results by node."""
    results, threads = {}, []
    for node, credential in credentials.items():
        run = lambda node=node, credential=credential: results.__setitem__(node, measure.loadgen(
            args, credential, ports, mode, "-duration", f"{seconds}s", *extra, failures_ok=True))
        threads.append(threading.Thread(target=run))
    for t in threads:
        t.start()
    for t in threads:
        t.join()
    return results


def run(args):
    code = measure.netem_namespace()
    if code is not None:
        sys.exit(code)
    name, _, binary = args.engine.partition("=")
    os.makedirs(args.out, exist_ok=True)
    work = tempfile.mkdtemp(prefix="soak-")
    os.chmod(work, 0o700)
    fakenode = subprocess.Popen(measure.pinned(args.node_cpus, [args.fakenode, "-dir", work]),
                                stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
    ports = json.loads(fakenode.stdout.readline())
    env = dict(os.environ, SSL_CERT_FILE=ports["certificate"])
    engine = measure.Engine(binary, work, name, ["--tun=false", "--local-proxy=true"], env=env,
                            wrap=lambda c: measure.pinned(args.engine_cpus, c))
    phase, stop = Phase(), threading.Event()
    watcher = threading.Thread(target=sampler, args=(engine, phase, os.path.join(args.out, "samples.csv"),
                                                     args.sample_seconds, stop))
    status = "OK"
    try:
        revision = 1
        engine.call("apply-profile", {"profile": measure.profile(f"soak#{revision}", ports)})
        engine.call("start")
        credentials = {node: engine.call("get-local-proxy-credential", {"node_id": node})
                       for node in measure.PROFILE_NODES}
        watcher.start()
        started = time.monotonic()
        end = started + args.hours * 3600
        next_apply = started + args.apply_seconds
        with open(os.path.join(args.out, "loads.csv"), "w", newline="") as f:
            out = csv.writer(f)
            out.writerow(["seconds", "phase", "node", "connections", "failed", "stalled"])
            while time.monotonic() < end:
                cycle = time.monotonic()
                for name_, mode, seconds, extra in (
                        ("stream", "stream", STREAM_SECONDS, ("-conns", "8", "-rate-mbit", "10")),
                        ("churn", "churn", CHURN_SECONDS, ("-churn-rate", "50")),
                        ("pingpong", "pingpong", PINGPONG_SECONDS, ())):
                    phase.set(name_)
                    for node, result in loads(args, credentials, ports, mode, seconds, *extra).items():
                        out.writerow([round(time.monotonic() - started), name_, node, result["connections"],
                                      result["failed_connections"], result.get("stalled_connections", 0)])
                    f.flush()
                    if engine.process.poll() is not None:
                        raise RuntimeError(f"the engine exited ({engine.process.returncode})")
                if time.monotonic() >= next_apply:
                    phase.set("apply")
                    revision += 1
                    engine.call("apply-profile", {"profile": measure.profile(
                        f"soak#{revision}", ports, rules_variant=revision % 2)})
                    next_apply += args.apply_seconds
                phase.set("idle")
                rest = args.cycle_seconds - (time.monotonic() - cycle)
                if rest > 0:
                    time.sleep(min(rest, max(0, end - time.monotonic())))
                if engine.process.poll() is not None:
                    raise RuntimeError(f"the engine exited ({engine.process.returncode})")
    except Exception as e:
        status = f"FAILED {e}"
        raise
    finally:
        stop.set()
        if watcher.is_alive():
            watcher.join()
        with open(os.path.join(args.out, "status"), "w") as f:
            f.write(status + "\n")
        engine.stop()
        fakenode.stdin.close()
        fakenode.terminate()
        fakenode.wait(10)


def summarize(args):
    for out in args.dirs:
        with open(os.path.join(out, "samples.csv")) as f:
            samples = [{k: (v if k == "phase" else int(v)) for k, v in row.items()} for row in csv.DictReader(f)]
        with open(os.path.join(out, "loads.csv")) as f:
            load_rows = list(csv.DictReader(f))
        idle = [s for s in samples if s["phase"] == "idle"]
        last = max(s["seconds"] for s in samples)
        second_hour = [s for s in idle if 3600 <= s["seconds"] < 7200]
        last_hour = [s for s in idle if s["seconds"] >= last - 3600]
        med = lambda rows, key: statistics.median(r[key] for r in rows) if rows else None
        summary = {"run": os.path.basename(os.path.normpath(out)), "hours": round(last / 3600, 2),
                   "samples": len(samples), "loads": len(load_rows)}
        for key in ("rss_kb", "fds", "threads", "tcp_all", "tcp_established"):
            summary[f"idle_{key}_hour2"] = med(second_hour, key)
            summary[f"idle_{key}_last_hour"] = med(last_hour, key)
            summary[f"max_{key}"] = max(s[key] for s in samples)
        summary["hwm_kb"] = max(s["hwm_kb"] for s in samples)
        summary["connections"] = sum(int(r["connections"]) for r in load_rows)
        summary["failed"] = sum(int(r["failed"]) for r in load_rows)
        summary["stalled"] = sum(int(r["stalled"]) for r in load_rows)
        status = os.path.join(out, "status")
        summary["status"] = open(status).read().strip() if os.path.exists(status) else "running"
        print("SOAK " + json.dumps(summary, sort_keys=True))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    sub = parser.add_subparsers(dest="command", required=True)
    r = sub.add_parser("run")
    r.add_argument("--engine", required=True, metavar="NAME=BIN")
    r.add_argument("--fakenode", required=True)
    r.add_argument("--loadgen", required=True)
    r.add_argument("--out", required=True)
    r.add_argument("--hours", type=float, default=24)
    r.add_argument("--cycle-seconds", type=int, default=600)
    r.add_argument("--sample-seconds", type=int, default=30)
    r.add_argument("--apply-seconds", type=int, default=3600)
    r.add_argument("--engine-cpus")
    r.add_argument("--load-cpus")
    r.add_argument("--node-cpus")
    s = sub.add_parser("summarize")
    s.add_argument("dirs", nargs="+")
    args = parser.parse_args()
    if args.command == "run":
        if sys.platform != "linux":
            sys.exit("soak.py runs on Linux (/proc)")
        run(args)
    else:
        summarize(args)


if __name__ == "__main__":
    main()
