#!/usr/bin/env python3
"""Gathers and compares the performance checks' tier A numbers.

Adapted from peakpassvpn/sail tools/perf/report.py (Apache-2.0).

    report.py collect --sha SHA --engine ENGINE [--size NAME=FILE ...] LOG... > perf.json
        Takes the `PERF {...}` lines of measure.py and of the engine's
        allocation test from LOGs, and the sizes of FILEs, into one file of
        numbers: the median of each over its rounds, and the rounds.

    report.py merge [--env KEY=VALUE ...] PERF.json... > perf.json
        One file of the files of one commit's jobs, with the measurement
        environment (runner, Go version, allocator, ...).

    report.py check PERF.json
        Fails unless the file is numbers only: the schema, a commit, the
        engine, metric names of [a-z0-9_.-] and numbers. Run before it is
        published.

    report.py compare NEW [--main MAIN] [--baseline BASELINE]
        Compares NEW with MAIN (the last main run) and BASELINE (Go 0.5.21,
        testdata/perf) by the thresholds below; writes a Markdown table, and
        exits 1 if any metric crossed one.

The file holds numbers, metric names, the engine and the commit only: it is
published as a workflow artifact of a public repository.
"""

import argparse
import json
import os
import re
import statistics
import sys

# metric suffix -> (largest growth against the last main run, against the
# Go 0.5.21 baseline, only comparable within one engine, source). None: no
# threshold, the number is recorded only. The most specific suffix first.
# Calibration (2026-10-03, testdata/perf/baseline-go-0.5.21.json): ten
# single-round runners plus three; "spread" is (max - min) / median of the
# rounds. A compared number is a median of three, so it varies less.
THRESHOLDS = [
    # Reproducible (spread 0). Against Go a trend only: Rust links the
    # engine into the host, whose installer Desktop measures (#45).
    ("size_bytes", 0.02, None, False, "spread 0; Go vs Rust: trend only"),
    # Same engine only: Go's GC and a Rust allocator count differently.
    ("allocations_per_mib", 0.03, 0.03, True, "spread <=1.6%"),
    ("allocations_per_connection", 0.05, 0.05, True, "spread <=3.6%"),
    ("allocated_bytes_per_mib", 0.10, 0.10, True, "spread <=5.3%"),
    # After a load the Go runtime returns memory on its own schedule.
    ("after_rss_kb", None, None, False, "record only: depends on the GC's scavenger timing"),
    ("rss_kb", 0.10, 0.20, False, "spread <=8.5%; against Go: Desktop's 120%"),
    # A few milliseconds at a resolution of one: spread 75-100%. Desktop's
    # "not worse than Go" for apply and kernel_switch_ms is judged on the
    # lab hosts and real machines instead (#45).
    ("_ms", None, None, False, "record only: spread 75-100% at 1 ms resolution"),
]

NAME = re.compile(r"^[a-z0-9_.-]+$")


def threshold(metric):
    for suffix, main, baseline, same_engine, source in THRESHOLDS:
        if metric.endswith(suffix):
            return main, baseline, same_engine, source
    return None


def collect(args):
    rounds = {}
    for path in args.logs:
        with open(path) as f:
            for line in f:
                if not line.startswith("PERF "):
                    continue
                row = json.loads(line[5:])
                prefix = row.pop("profile")
                row.pop("round", None)
                for key, value in row.items():
                    if isinstance(value, (int, float)) and not isinstance(value, bool):
                        rounds.setdefault(f"{prefix}.{key}", []).append(value)
    for spec in args.size or []:
        name, _, path = spec.partition("=")
        rounds[f"size.{name}.size_bytes"] = [os.path.getsize(path)]
    for name in rounds:
        if not NAME.match(name):
            sys.exit(f"{name}: not a metric name")
    dump({"schema": 1, "sha": args.sha, "engine": args.engine,
          "metrics": {name: {"median": statistics.median(values), "rounds": values}
                      for name, values in sorted(rounds.items())}})


def merge(args):
    files = [load(p) for p in args.files]
    keys = {(f["sha"], f["engine"]) for f in files}
    if len(keys) != 1:
        sys.exit(f"files of different commits or engines: {sorted(keys)}")
    metrics = {}
    for f in files:
        for name, value in f["metrics"].items():
            if name in metrics:
                rounds = metrics[name]["rounds"] + value["rounds"]
                metrics[name] = {"median": statistics.median(rounds), "rounds": rounds}
            else:
                metrics[name] = value
    sha, engine = keys.pop()
    out = {"schema": 1, "sha": sha, "engine": engine, "metrics": dict(sorted(metrics.items()))}
    if args.env:
        out["environment"] = dict(spec.partition("=")[::2] for spec in args.env)
    dump(out)


def check(args):
    data = load(args.file)
    problems = []
    if set(data) - {"environment"} != {"schema", "sha", "engine", "metrics"} or data["schema"] != 1:
        problems.append("keys other than schema, sha, engine, metrics (and environment)")
    if not re.fullmatch(r"[0-9a-f]{7,40}", str(data.get("sha", ""))):
        problems.append("sha is not a commit")
    if not NAME.match(str(data.get("engine", ""))):
        problems.append("engine is not a name")
    for name, value in data.get("metrics", {}).items():
        if not NAME.match(name):
            problems.append(f"{name!r}: not a metric name")
        if not isinstance(value, dict) or set(value) != {"median", "rounds"}:
            problems.append(f"{name}: not median and rounds")
            continue
        numbers = [value["median"], *value["rounds"]]
        if not all(isinstance(n, (int, float)) and not isinstance(n, bool) for n in numbers):
            problems.append(f"{name}: not numbers")
    for key, value in data.get("environment", {}).items():
        if not NAME.match(key) or not re.fullmatch(r"[A-Za-z0-9 ._:/+=,()-]{0,120}", str(value)):
            problems.append(f"environment {key!r}: not a plain value")
    if problems:
        sys.exit("\n".join(f"{args.file}: {p}" for p in problems))


def load(path):
    with open(path) as f:
        return json.load(f)


def dump(data):
    json.dump(data, sys.stdout, indent=1)
    print()


def compare(args):
    new = load(args.new)
    against = []
    if args.main:
        against.append(("main", load(args.main)))
    if args.baseline:
        against.append(("baseline", load(args.baseline)))
    crossed = []
    header = " | ".join(f"{n} ({b['engine']} {b['sha'][:8]})" for n, b in against)
    lines = [f"Engine {new['engine']} at {new['sha'][:8]}.", "",
             f"| metric | new | {header} | limit |",
             "| --- | --- | " + " | ".join("---" for _ in against) + " | --- |"]
    for metric, now in sorted(new["metrics"].items()):
        limits = threshold(metric)
        cells = []
        for which, base in against:
            old = base["metrics"].get(metric)
            if old is None or not old["median"]:
                cells.append("—")
                continue
            if limits and limits[2] and base["engine"] != new["engine"]:
                cells.append("(same engine only)")
                continue
            growth = now["median"] / old["median"] - 1
            limit = limits and (limits[0] if which == "main" else limits[1])
            over = limit is not None and growth > limit
            if over:
                crossed.append(f"{metric}: +{growth:.1%} against {which} (limit {limit:.0%})")
            cells.append(f"{'**' if over else ''}{growth:+.1%}{'**' if over else ''}")
        if limits:
            limit_text = " / ".join("—" if v is None else f"{v:.0%}" for v in limits[:2])
        else:
            limit_text = "—"
        lines.append(f"| {metric} | {now['median']:.6g} | " + " | ".join(cells) + f" | {limit_text} |")
    print("\n".join(lines))
    print("\nLimits: against main / against the Go 0.5.21 baseline; — records only. "
          "RSS is the engine process's (the lab binary for Rust); the host process's total RSS "
          "and installer size are Desktop's measurements.")
    if crossed:
        print("\nOver the limit:\n" + "\n".join(f"- {c}" for c in crossed))
        sys.exit(1)


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="command", required=True)
    c = sub.add_parser("collect")
    c.add_argument("--sha", required=True)
    c.add_argument("--engine", required=True)
    c.add_argument("--size", action="append", metavar="NAME=FILE")
    c.add_argument("logs", nargs="*")
    m = sub.add_parser("merge")
    m.add_argument("--env", action="append", metavar="KEY=VALUE", help="measurement environment, recorded with the numbers")
    m.add_argument("files", nargs="+")
    h = sub.add_parser("check")
    h.add_argument("file")
    k = sub.add_parser("compare")
    k.add_argument("new")
    k.add_argument("--main")
    k.add_argument("--baseline")
    args = parser.parse_args()
    {"collect": collect, "merge": merge, "check": check, "compare": compare}[args.command](args)


if __name__ == "__main__":
    main()
