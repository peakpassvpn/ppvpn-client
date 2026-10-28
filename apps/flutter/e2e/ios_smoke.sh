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
mkdir -p build
log=build/ios-smoke.log

# A launch crash leaves flutter test waiting for the app's VM service with
# no output, so the test gets 12 minutes and a failure prints what the
# simulator saw: the end of flutter's verbose log, the app's own log and
# any crash report.
diagnose() {
  echo "--- flutter test -v, last 120 lines" >&2
  tail -120 "$log" >&2 || true
  echo "--- Runner log, last 15 minutes" >&2
  xcrun simctl spawn "$udid" log show --last 15m --style compact \
    --predicate 'process == "Runner"' 2>&1 | tail -200 >&2 || true
  for report in "$HOME"/Library/Logs/DiagnosticReports/Runner*; do
    [ -e "$report" ] || continue
    echo "--- $report" >&2
    head -120 "$report" >&2
  done
}

flutter test -v integration_test/mobile_smoke_test.dart -d "$udid" >"$log" 2>&1 &
test_pid=$!
( sleep 720 && kill "$test_pid" 2>/dev/null ) &
watchdog=$!
status=0
wait "$test_pid" || status=$?
kill "$watchdog" 2>/dev/null || true
if [ "$status" -ne 0 ]; then
  diagnose
  exit "$status"
fi
grep -E '^\[ *\+?[0-9]+ ms\] (All tests passed|[0-9]+:[0-9]+ \+)' "$log" | tail -5 || tail -20 "$log"
