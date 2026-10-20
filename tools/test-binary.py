#!/usr/bin/env python3
"""The test binary cargo built for a target, from `cargo test --no-run
--message-format=json` output: ci.yml's macOS tests build several packages'
tests in one cargo invocation (one feature resolution, so sail is compiled
once) and run each binary with its own filters.

    tools/test-binary.py CARGO_JSON TARGET

TARGET is the target's name (`ppvpn_core`, `ppvpn-service`, `keystore`).
Prints the binary's path; exits 1 when none was built.
"""

import json
import sys


def main():
    path, target = sys.argv[1], sys.argv[2]
    found = []
    with open(path) as f:
        for line in f:
            try:
                msg = json.loads(line)
            except json.JSONDecodeError:
                continue
            if (msg.get("reason") == "compiler-artifact" and msg["profile"]["test"]
                    and msg.get("executable") and msg["target"]["name"] == target):
                found.append(msg["executable"])
    if len(found) != 1:
        sys.exit(f"test-binary: {len(found)} test binaries for target {target!r} in {path}")
    print(found[0])


if __name__ == "__main__":
    main()
