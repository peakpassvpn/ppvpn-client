#!/usr/bin/env bash
# The iOS smoke test (#448) on a simulator (ci.yml, job ios, after
# ci-build.sh with FLUTTER_TARGET=ios IOS_SDK=simulator):
# integration_test/mobile_smoke_test.dart on the newest available iPhone,
# its debug app linking the bridge ci-build.sh built for the simulator.
set -euo pipefail
cd "$(dirname "$0")/.."

udid=$(xcrun simctl list devices available --json | python3 -c '
import json, sys
runtimes = json.load(sys.stdin)["devices"]
phones = [(runtime, device) for runtime, devices in runtimes.items() if ".iOS-" in runtime
          for device in devices if device["name"].startswith("iPhone")]
if not phones:
    sys.exit("no iPhone simulator")
print(max(phones, key=lambda pair: [int(part) for part in pair[0].rsplit(".iOS-", 1)[1].split("-")])[1]["udid"])
')
xcrun simctl boot "$udid" 2>/dev/null || true
xcrun simctl bootstatus "$udid" -b
trap 'xcrun simctl shutdown "$udid" || true' EXIT
flutter test integration_test/mobile_smoke_test.dart -d "$udid"
