#!/usr/bin/env python3
"""Gathers and compares the performance checks' tier A and B numbers.

Adapted from peakpassvpn/sail tools/perf/report.py (Apache-2.0).

    report.py collect --sha SHA --engine ENGINE [--size NAME=FILE ...] LOG... > perf.json
        Takes the `PERF {...}` lines of measure.py and of the engine's
        allocation test from LOGs, and the sizes of FILEs, into one file of
        numbers: the median of each over its rounds, and the rounds. Lines
        that name another engine ("engine", tier B's) are left out.

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

# metric suffix -> (largest change against the last main run, against the
# Go 0.5.21 baseline, only comparable within one engine, source[, how]).
# None: no threshold, the number is recorded only. The most specific suffix
# first. how: "up" (the default) crosses when the number grows by more than
# the limit, "down" when it falls by more (throughput), "abs" when it grows
# by more than the limit in its own unit.
# Calibration (2026-10-03, testdata/perf/baseline-go-0.5.21.json): ten
# single-round runners plus three; "spread" is (max - min) / median of the
# rounds. A compared number is a median of three, so it varies less.
THRESHOLDS = [
    # Tier B (#214's thresholds, against Go measured as a pair on one
    # machine on one day; never against main: tier B runs by hand).
    ("_mbit", None, 0.05, False, "#214: throughput at least 95% of Go's", "down"),
    ("extra_p50_us", None, 2000, False, "#214: p50 at most Go + 2 ms", "abs"),
    ("extra_p99_us", None, 0.10, False, "#214: p99 at most 110% of Go's"),
    ("connect_p50_us", None, 0.10, False, "#214: first packet p50 at most 110% of Go's"),
    ("cpu100_pct", None, 0.10, False, "#214: CPU at 100 Mbit/s at most 110% of Go's"),
    ("idle_cpu_pct", None, 0.0, False, "#214: idle CPU not above Go's"),
    ("idle_switches_per_s", None, None, False, "record only: wakeups, roughly"),
    ("_cpu_pct", None, None, False, "record only: where the throughput's bottleneck was"),
    ("_load_limited", None, None, False, "1: the throughput beside it is the load side's"),
    ("_us", None, None, False, "record only"),
    # Reproducible (spread 0). Against Go a trend only: Rust links the
    # engine into the host, whose installer Desktop measures (#214).
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
    # lab hosts and real machines instead (#214).
    ("_ms", None, None, False, "record only: spread 75-100% at 1 ms resolution"),
]

NAME = re.compile(r"^[a-z0-9_.-]+$")


def threshold(metric):
    for suffix, main, baseline, same_engine, source, *how in THRESHOLDS:
        if metric.endswith(suffix):
            return main, baseline, same_engine, source, (how or ["up"])[0]
    return None


def crosses(now, old, limit, how):
    """Whether now, against old, crosses limit; and the change as shown."""
    if how == "abs":
        delta = now - old
        return delta > limit, f"{delta:+.6g}"
    growth = now / old - 1
    over = growth < -limit if how == "down" else growth > limit
    return over, f"{growth:+.1%}"


def collect(args):
    rounds = {}
    for path in args.logs:
        with open(path) as f:
            for line in f:
                if not line.startswith("PERF "):
                    continue
                row = json.loads(line[5:])
                if row.pop("engine", args.engine) != args.engine:
                    continue
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


def change_of(now, old):
    return f"{now / old - 1:+.1%}" if old else "—"


def limit_text_of(limit, how):
    if how == "abs":
        return f"+{limit:g}"
    return f"{'-' if how == 'down' else ''}{limit:.0%}"


def throughput_breakdown(new, against):
    """Tier B's throughputs, each with the CPU of the engine, loadgen and
    fakenode (100 = one CPU) and whether the load side capped it, for the new
    file and each it is compared with: where the bottleneck was is part of
    the result."""
    files = [("new", new)] + list(against)
    keys = sorted(k.removesuffix("_mbit") for k in new["metrics"] if k.startswith("tierb.") and k.endswith("_mbit"))
    if not keys:
        return []
    med = lambda f, k: (f["metrics"].get(k) or {}).get("median")
    lines = ["Throughput, with each process's CPU (engine / loadgen / fakenode) and whether the load side capped it:", "",
             "| metric | " + " | ".join(f"{n} ({f['engine']})" for n, f in files) + " |",
             "| --- | " + " | ".join("---" for _ in files) + " |"]
    for key in keys:
        cells = []
        for _, f in files:
            mbit = med(f, f"{key}_mbit")
            if mbit is None:
                cells.append("—")
                continue
            cpus = " / ".join(f"{med(f, f'{key}_{p}_cpu_pct'):.0f}" if med(f, f"{key}_{p}_cpu_pct") is not None else "?"
                              for p in ("engine", "load", "node"))
            capped = " **load-limited**" if (med(f, f"{key}_load_limited") or 0) >= 0.5 else ""
            cells.append(f"{mbit:.6g} Mbit/s ({cpus}){capped}")
        lines.append(f"| {key.removeprefix('tierb.')} | " + " | ".join(cells) + " |")
    return lines


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
            how = limits[4] if limits else "up"
            if old is None or (how != "abs" and not old["median"]):
                cells.append("—")
                continue
            if limits and limits[2] and base["engine"] != new["engine"]:
                cells.append("(same engine only)")
                continue
            # A throughput the load side capped, here or there, is not judged,
            # unless the new one is capped and still not lower: a lower
            # bound that passes.
            limited = metric.removesuffix("_mbit") + "_load_limited"
            capped = lambda f: (f["metrics"].get(limited) or {}).get("median", 0) >= 0.5
            if metric.endswith("_mbit") and (capped(base) or (capped(new) and now["median"] < old["median"])):
                cells.append(f"{change_of(now['median'], old['median'])} (load-limited, not judged)")
                continue
            limit = limits and (limits[0] if which == "main" else limits[1])
            over, change = crosses(now["median"], old["median"], limit if limit is not None else 0, how)
            over = over and limit is not None
            if over:
                crossed.append(f"{metric}: {change} against {which} (limit {limit_text_of(limit, how)})")
            cells.append(f"{'**' if over else ''}{change}{'**' if over else ''}")
        if limits:
            limit_text = " / ".join("—" if v is None else limit_text_of(v, limits[4]) for v in limits[:2])
        else:
            limit_text = "—"
        lines.append(f"| {metric} | {now['median']:.6g} | " + " | ".join(cells) + f" | {limit_text} |")
    print("\n".join(lines))
    breakdown = throughput_breakdown(new, against)
    if breakdown:
        print("\n" + "\n".join(breakdown))
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
