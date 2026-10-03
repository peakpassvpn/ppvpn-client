#!/usr/bin/env python3
"""Reports where our Cargo.lock resolves the packages it shares with sail
differently from sail's own Cargo.lock at the sail commit we pin: another
version of a package, or the same version depending on other versions (as
when a lock regenerated for a new crate moves winapi-util from windows-sys
0.48 to 0.61). A dependency only one side has (a feature the other does
not enable) is not a difference. sail is tested with its lock; drifting
from it builds something sail never ran.

    tools/sail-lock-compare.py [SAIL_LOCK]

SAIL_LOCK is sail's Cargo.lock; by default the one in the sail checkout
`cargo metadata` names (run after `cargo fetch`). Only reports: prints a
GitHub warning per difference and exits 0, so CI shows drift without
failing on it. Packages sail does not have (our own crates and their
dependencies) are not compared.
"""

import json
import os
import subprocess
import sys
import tomllib

ROOT = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))


def packages(path):
    """(name, version) -> {dependency name: version}. A lock names a
    dependency bare when one version of it is locked, else as "name version"."""
    with open(path, "rb") as f:
        lock = tomllib.load(f)
    versions = {}
    for p in lock.get("package", []):
        versions.setdefault(p["name"], []).append(p["version"])
    out = {}
    for p in lock.get("package", []):
        deps = {}
        for dep in p.get("dependencies", []):
            name, _, version = dep.partition(" ")
            deps[name] = version or versions[name][0]
        out[(p["name"], p["version"])] = deps
    return out


def sail_lock():
    run = subprocess.run(
        ["cargo", "metadata", "--format-version", "1", "--locked", "--offline"],
        capture_output=True, text=True, cwd=ROOT,
    )
    if run.returncode != 0:
        sys.exit("sail-lock-compare: cargo metadata failed:\n" + run.stderr.strip())
    sail = next(p for p in json.loads(run.stdout)["packages"] if p["name"] == "sail")
    return os.path.join(os.path.dirname(os.path.dirname(sail["manifest_path"])), "Cargo.lock")


def main():
    theirs_path = sys.argv[1] if len(sys.argv) > 1 else sail_lock()
    ours, theirs = packages(os.path.join(ROOT, "Cargo.lock")), packages(theirs_path)
    their_names = {}
    for name, version in theirs:
        their_names.setdefault(name, set()).add(version)

    findings = []
    for (name, version), deps in sorted(ours.items()):
        if name not in their_names:
            continue
        if version not in their_names[name]:
            findings.append(f"{name} {version}: sail locks {', '.join(sorted(their_names[name]))}")
        else:
            # Dependencies only one side has come from features the other
            # does not enable; a dependency both have must be the same one.
            sails = theirs[(name, version)]
            for dep, v in sorted(deps.items()):
                if dep in sails and sails[dep] != v:
                    findings.append(f"{name} {version}: depends on {dep} {v} where sail's has {dep} {sails[dep]}")

    for finding in findings:
        print(f"::warning title=Cargo.lock differs from sail's::{finding}")
    shared = sum(1 for key in ours if key[0] in their_names)
    print(f"sail-lock-compare: {shared} shared packages, {len(findings)} resolved differently from sail's lock")


if __name__ == "__main__":
    main()
