#!/usr/bin/env bash
# Native standard-driver integration, not a full macOS A-to-B installer test.
set -euo pipefail
root=$(cd "$(dirname "$0")/../../.." && pwd)
work="$root/build/macos-update-qa"
mkdir -p "$work"
arch=$(uname -m)
"$root/tools/desktop/macos/build-sparkle.sh" "$work/sparkle" "$arch"
framework=$(cat "$work/sparkle/framework.path")
# The fixture needs runnable local helpers. Shipping packaging independently
# applies its existing signing identities and verification inside-out.
codesign --force --deep --sign - "$framework"
codesign --verify --deep --strict "$framework"
python3 - "$root" "$work" <<'PY'
from pathlib import Path
import sys,plistlib
root,work=map(Path,sys.argv[1:]);source=(root/'apps/flutter/runner/macos/Runner/MainFlutterWindow.swift').read_text()
policy=source.split('// BEGIN shared update presentation policy (also compiled by native QA).',1)[1].split('// END shared update presentation policy.',1)[0]
qa=(root/'tools/desktop/macos/update-prompt-qa.swift').read_text()
(work/'probe.swift').write_text(qa.replace('import os','import os\n'+policy,1))
for role in ['host','other']:
 app=work/f'{role}.app/Contents';(app/'MacOS').mkdir(parents=True,exist_ok=True)
 info={'CFBundleIdentifier':f'com.peakpassvpn.update-qa.{role}','CFBundleName':f'PPVPN Update QA {role}','CFBundleExecutable':'probe','CFBundlePackageType':'APPL','CFBundleVersion':'0.7.5.1','CFBundleShortVersionString':'0.7.5','SUEnableAutomaticChecks':False,'SUFeedURL':'http://127.0.0.1:18763/feed.xml','SUPublicEDKey':'AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA=','NSAppTransportSecurity':{'NSAllowsLocalNetworking':True,'NSAllowsArbitraryLoads':True}}
 (app/'Info.plist').write_bytes(plistlib.dumps(info))
PY
swiftc -parse-as-library "$work/probe.swift" -F "$(dirname "$framework")" -framework Sparkle -o "$work/host.app/Contents/MacOS/probe" -Xlinker -rpath -Xlinker "$(dirname "$framework")"
cp "$work/host.app/Contents/MacOS/probe" "$work/other.app/Contents/MacOS/probe"
for role in host other; do codesign --force --sign - "$work/$role.app"; done
python3 "$root/tools/desktop/windows/update-feed-fixture.py" --macos > "$work/feed.stdout" 2> "$work/feed.stderr" &
server=$!
probe=''
cleanup() {
  if [ -n "$probe" ] && kill -0 "$probe" 2>/dev/null; then kill "$probe"; fi
  if [ -f "$work/other-pid.txt" ]; then
    other_pid=$(cat "$work/other-pid.txt")
    case "$other_pid" in *[!0-9]*|'') ;; *)
      command=$(ps -p "$other_pid" -o command= || true)
      case "$command" in "$work/other.app/Contents/MacOS/probe"*) kill "$other_pid" 2>/dev/null || true;; esac
    ;; esac
  fi
  if [ -f "$work/client-pid.txt" ]; then
    client_pid=$(cat "$work/client-pid.txt")
    case "$client_pid" in *[!0-9]*|'') ;; *)
      command=$(ps -p "$client_pid" -o command= || true)
      case "$command" in "$work/first-show/PPVPN.app/Contents/MacOS/PPVPN"*) kill "$client_pid" 2>/dev/null || true;; esac
    ;; esac
  fi
  kill "$server" 2>/dev/null || true
}
trap cleanup EXIT
rm -f "$work/mode" "$work/other-ready"
extra=()
if [ -n "${1:-}" ]; then
  mkdir -p "$work/first-show"
  ditto "$1" "$work/first-show/PPVPN.app"
  extra+=("$work/first-show/PPVPN.app")
  shasum -a 256 "$work/first-show/PPVPN.app/Contents/MacOS/PPVPN" > "$work/actual-client-hash.txt"
fi
"$work/host.app/Contents/MacOS/probe" host "$work" "$work/other.app" "${extra[@]}" > "$work/raw.txt" 2> "$work/errors.txt" &
probe=$!
for _ in $(seq 1 110); do
  if ! kill -0 "$probe" 2>/dev/null; then break; fi
  sleep 1
done
if kill -0 "$probe" 2>/dev/null; then
  screencapture -x "$work/timeout.png" || true
  echo 'FAIL native UI integration timed out' >&2
  exit 1
fi
set +e
wait "$probe"
status=$?
set -e
cat "$work/raw.txt" "$work/errors.txt"
printf '%s\n' "$status" > "$work/exitcode.txt"
git -C "$root" rev-parse HEAD > "$work/source.txt"
git -C "$work/sparkle/source" rev-parse HEAD > "$work/sparkle-source.txt"
(cd "$work/sparkle/source" && shasum -a 256 Sparkle/SPUStandardUserDriver.m Sparkle/InstallerProgress/InstallerProgressAppController.m) > "$work/sparkle-source-hashes.txt"
shasum -a 256 "$framework/Versions/B/Sparkle" "$work/host.app/Contents/MacOS/probe" > "$work/hashes.txt"
exit "$status"
