#!/usr/bin/env python3
"""The heap at its peak, by allocation site, from a dhat heap profile
(ne-mem's --dhat-file, tools/perf/ne_mem.py --dhat): the bytes live at
t-gmax summed by the first frame of ours or a dependency's (allocator,
std, core and dhat frames skipped), then the largest sites with their
stacks.

    tools/perf/dhat_top.py dhat-heap.json [--top 25] [--depth 8]

Prints Markdown.
"""

import argparse
import collections
import json
import re

# Frames that only allocate: skipped to reach the code that asked.
PLUMBING = re.compile(r"^(<?)(alloc|core|std|dhat|hashbrown|__rust|rust_|_?malloc|<alloc|<core|<std)::|"
                      r"^(<?alloc::|<?core::|<?std::)|__rdl_|__rg_|^\[root\]$")


def name(frame):
    """`0x1234: crate::path::fn (file:line)` -> `crate::path::fn (file:line)`."""
    return frame.split(": ", 1)[1] if re.match(r"^0x[0-9a-f]+: ", frame) else frame


def function(frame):
    return re.sub(r" \(.*\)$", "", name(frame))


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("profile")
    parser.add_argument("--top", type=int, default=25)
    parser.add_argument("--depth", type=int, default=8)
    args = parser.parse_args()
    with open(args.profile) as f:
        d = json.load(f)
    ftbl = d["ftbl"]
    total = sum(pp["gb"] for pp in d["pps"])
    blocks = sum(pp["gbk"] for pp in d["pps"])

    def ours(pp):
        frames = [ftbl[i] for i in pp["fs"]]
        kept = [f for f in frames if not PLUMBING.search(function(f))]
        return kept or frames

    by_site = collections.Counter()
    by_crate = collections.Counter()
    for pp in d["pps"]:
        if not pp["gb"]:
            continue
        frames = ours(pp)
        by_site[function(frames[0])] += pp["gb"]
        crate = re.match(r"^<?([a-z0-9_]+)", function(frames[0]))
        by_crate[crate.group(1) if crate else "?"] += pp["gb"]

    mib = lambda b: f"{b / 1048576:.2f}"
    print(f"Live at t-gmax: {mib(total)} MiB in {blocks} blocks.\n")
    print("| crate | MiB |\n| --- | --- |")
    for crate, b in by_crate.most_common(12):
        print(f"| {crate} | {mib(b)} |")
    print("\n| first frame of ours | MiB |\n| --- | --- |")
    for site, b in by_site.most_common(args.top):
        print(f"| `{site}` | {mib(b)} |")
    print("\nThe largest allocation points with their stacks:\n")
    for pp in sorted(d["pps"], key=lambda pp: -pp["gb"])[:10]:
        if not pp["gb"]:
            break
        print(f"- {mib(pp['gb'])} MiB in {pp['gbk']} blocks")
        for f in ours(pp)[: args.depth]:
            print(f"  - `{name(f)}`")


if __name__ == "__main__":
    main()
