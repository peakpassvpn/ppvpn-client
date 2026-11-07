#!/usr/bin/env python3
"""Inventory actual serial Android builds, refusing unlike signing/toolchains."""
import argparse
import json
from pathlib import Path

from measure_size import compare, inventory


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output", type=Path)
    args = parser.parse_args()
    root = args.output
    builds = {side: json.loads((root / side / "build-provenance.json").read_text())
              for side in ("baseline", "candidate")}
    baseline = builds["baseline"]["source_commit"]
    layouts = {
        "app-release.apk": ("apk", "universal", ["arm64-v8a", "x86_64"]),
        "app-arm64-v8a-release.apk": ("apk", "abi-split", ["arm64-v8a"]),
        "app-x86_64-release.apk": ("apk", "abi-split", ["x86_64"]),
        "app-release.aab": ("aab", "bundle", ["arm64-v8a", "x86_64"]),
    }
    results = {}
    for name, (kind, variant, abis) in layouts.items():
        measured = {}
        for side, provenance in builds.items():
            certificates = provenance["verified_signing_certificate_sha256"][name]
            if not certificates:
                raise ValueError("missing verified signing certificate")
            conditions = {
                "optimization": "Rust bridge release; tunnel mobile thin LTO, one codegen unit; Flutter/Gradle release",
                "strip": "shipping Gradle native library processing; tunnel llvm-strip --strip-unneeded",
                "signing": {"mode": provenance["signing"], "certificates_sha256": certificates},
                "toolchain": {key: provenance[key] for key in (
                    "rust", "flutter", "jdk_version", "ndk_revision",
                    "sdk_platforms_available", "sdk_build_tools_available")},
                "build_number": provenance["build_number"],
                "backend_origin_sha256": provenance["backend_origin_sha256"],
            }
            removals = (["mobile-local-proxy-server", "mobile-proxy-settings-ui", "app-standard-forwarder"]
                        if side == "candidate" else [])
            measured[side] = inventory(root / side / name, kind, variant, abis,
                                      provenance["source_commit"], baseline, conditions, removals)
            (root / side / (name + ".json")).write_text(json.dumps(measured[side], indent=2) + "\n")
        results[name] = compare(measured["baseline"], measured["candidate"])
    (root / "comparison.json").write_text(json.dumps(results, indent=2) + "\n")
    print(json.dumps({name: {key: row[key] for key in ("saved_bytes", "saved_MB", "saved_MiB")}
                      for name, row in results.items()}, indent=2))


if __name__ == "__main__":
    main()
