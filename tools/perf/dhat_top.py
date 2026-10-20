#!/usr/bin/env python3
"""The heap at its peak, by allocation site, from a dhat heap profile
(ne-mem's --dhat-file, tools/perf/ne_mem.py --dhat): the bytes live at
t-gmax summed by the first source line past the standard library and the
allocator, the caller in our code (sail's, ppvpn's) after it when that
line is a dependency's (`bytes-1.12.1/src/bytes_mut.rs:157 <- transport/
muxcore/session.rs:736`), then the largest allocation points' stacks.

    tools/perf/dhat_top.py dhat-heap.json [--top 25] [--depth 8]

Frames are those of a line-tables build: short function names, and paths
relative to the crate's source (a dependency's begins with its directory,
`name-1.2.3/`). Prints Markdown.
"""

import argparse
import collections
import json
import re

# The standard library's files (as rustc remaps them) and dhat's.
STD = re.compile(r"^(alloc|core|std)/src/|^src/(raw_vec|vec|boxed|slice|convert|sys|thread|panic|ops|string|"
                 r"collections|sync|fmt|io|alloc)/|^sys/|^dhat-\d")
DEPENDENCY = re.compile(r"^[A-Za-z0-9_\-]+-\d+\.\d+\.\d+[^/]*/")


def location(frame):
    """`0x1: name (path:line:col)` -> (path, `path:line`), or ("", frame);
    the global allocator's shims (`__rust_alloc_zeroed`, at the static's
    line) count as std."""
    if re.search(r": __r(ust|dl|g)_", frame):
        return ("alloc/src/", frame)
    m = re.search(r"\(([^()]*):(\d+):\d+\)$", frame)
    return (m.group(1), f"{m.group(1)}:{m.group(2)}") if m else ("", frame)


def site(frames):
    """The first frame past std, and our caller when that is a dependency's."""
    first = ours = None
    for frame in frames:
        path, where = location(frame)
        if not path or STD.search(path):
            continue
        if first is None:
            first = where
        if not DEPENDENCY.search(path):
            ours = where
            break
    if first is None:
        return "?"
    return first if ours in (None, first) else f"{first} <- {ours}"


def main():
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("profile")
    parser.add_argument("--top", type=int, default=25)
    parser.add_argument("--depth", type=int, default=8)
    args = parser.parse_args()
    with open(args.profile) as f:
        d = json.load(f)
    ftbl = d["ftbl"]
    pps = [pp for pp in d["pps"] if pp["gb"]]
    total = sum(pp["gb"] for pp in pps)
    by_site = collections.Counter()
    blocks = collections.Counter()
    for pp in pps:
        key = site([ftbl[i] for i in pp["fs"]])
        by_site[key] += pp["gb"]
        blocks[key] += pp["gbk"]

    mib = lambda b: f"{b / 1048576:.2f}"
    print(f"Live at t-gmax: {mib(total)} MiB in {sum(pp['gbk'] for pp in pps)} blocks.\n")
    print("| site | MiB | blocks |\n| --- | --- | --- |")
    for key, b in by_site.most_common(args.top):
        print(f"| `{key}` | {mib(b)} | {blocks[key]} |")
    print("\nThe largest allocation points, their frames past std:\n")
    for pp in sorted(pps, key=lambda pp: -pp["gb"])[:8]:
        print(f"- {mib(pp['gb'])} MiB in {pp['gbk']} blocks")
        frames = [ftbl[i] for i in pp["fs"] if not STD.search(location(ftbl[i])[0])]
        for frame in frames[: args.depth]:
            print(f"  - `{location(frame)[1]}`")


if __name__ == "__main__":
    main()
