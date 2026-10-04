#!/usr/bin/env python3
"""The engine's memory after apply and start, idle, per build, instance and
profile size (#193): how much a sail commit's memory change weighs for our
configurations. Not a tier B metric. On Linux, as root for the TUN instance.

    diag_rss.py --engine NAME=BIN [--engine NAME=BIN ...] --fakenode BIN
                [--rules 0,3000] [--rounds 3] [--skip-tun]

For each round, each profile size (the measured profile plus that many
inline rules, one domain suffix each, all distinct), each instance
(standard: local proxy; tun: TUN in a namespace of its own) and each engine
in turn: apply, start, 10 s idle, then a MEM line with VmRSS and the
anonymous memory (/proc/<pid>/smaps_rollup Anonymous: the heap, roughly),
and the inline route rules the profile had.
"""

import argparse
import json
import os
import shutil
import subprocess
import sys
import tempfile
import time

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import measure as m  # noqa: E402


def anonymous_kb(pid):
    with open(f"/proc/{pid}/smaps_rollup") as f:
        for line in f:
            if line.startswith("Anonymous:"):
                return int(line.split()[1])
    return None


def profile(ports, extra):
    p = m.profile("perf#1", ports)
    p["routing"]["rules"] = [
        {"id": f"bulk-{i}", "match": {"domain_suffixes": [f"bulk{i}.example"]}, "action": {"type": "direct"}}
        for i in range(extra)
    ] + p["routing"]["rules"]
    return p


def measure_one(binary, work, name, kind, ports, extra, env, idle):
    netns = None
    if kind == "tun":
        netns = f"rss{os.getpid()}"
        for command in (["ip", "netns", "add", netns],
                        ["ip", "netns", "exec", netns, "ip", "link", "set", "lo", "up"],
                        ["ip", "netns", "exec", netns, "ip", "link", "add", "dummy0", "type", "dummy"],
                        ["ip", "netns", "exec", netns, "ip", "addr", "add", "10.200.0.2/24", "dev", "dummy0"],
                        ["ip", "netns", "exec", netns, "ip", "link", "set", "dummy0", "up"],
                        ["ip", "netns", "exec", netns, "ip", "route", "add", "default", "via", "10.200.0.1"]):
            subprocess.run(command, check=True)
    flags = ["--tun", "--local-proxy=false"] if kind == "tun" else ["--tun=false", "--local-proxy=true"]
    try:
        engine = m.Engine(binary, work, name, flags, netns=netns, env=env)
        try:
            engine.call("apply-profile", {"profile": profile(ports, extra)})
            engine.call("start")
            time.sleep(idle)
            return engine.status("VmRSS"), anonymous_kb(engine.process.pid)
        finally:
            engine.stop()
    finally:
        if netns:
            subprocess.run(["ip", "netns", "del", netns], check=False)


def main():
    p = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    p.add_argument("--engine", action="append", required=True, metavar="NAME=BIN")
    p.add_argument("--fakenode", required=True)
    p.add_argument("--rules", default="0,3000", help="extra inline rules per profile, comma separated")
    p.add_argument("--rounds", type=int, default=3)
    p.add_argument("--idle-seconds", type=float, default=10)
    p.add_argument("--skip-tun", action="store_true")
    args = p.parse_args()
    engines = [e.partition("=")[::2] for e in args.engine]
    kinds = ["standard"] + ([] if args.skip_tun or os.geteuid() != 0 else ["tun"])
    for rnd in range(1, args.rounds + 1):
        work = tempfile.mkdtemp(prefix="diag-rss-")
        node = subprocess.Popen([args.fakenode, "-dir", work], stdin=subprocess.PIPE, stdout=subprocess.PIPE, text=True)
        try:
            ports = json.loads(node.stdout.readline())
            env = dict(os.environ, SSL_CERT_FILE=ports["certificate"])
            for extra in (int(x) for x in args.rules.split(",")):
                for kind in kinds:
                    order = engines if rnd % 2 else engines[::-1]
                    for name, binary in order:
                        rss, anon = measure_one(binary, work, f"{name}-{kind}-{extra}-{rnd}", kind, ports, extra,
                                                env, args.idle_seconds)
                        print("MEM " + json.dumps({"engine": name, "round": rnd, "instance": kind, "extra_rules": extra,
                                                   "profile_rules": extra + 2, "rss_kb": rss, "anon_kb": anon}), flush=True)
        finally:
            node.stdin.close()
            node.terminate()
            node.wait(10)
            shutil.rmtree(work, ignore_errors=True)


if __name__ == "__main__":
    main()
