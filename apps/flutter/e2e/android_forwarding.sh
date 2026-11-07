#!/usr/bin/env bash
# Builder hook AFTER normal Android ci-build.sh, on the existing Ubuntu/KVM
# emulator. Does not edit workflow/ci-build.sh or fetch any SDK/toolchain.
set -euo pipefail
cd "$(dirname "$0")/.."
[[ $(uname -s) == Linux ]] || { echo 'Use the disposable Ubuntu emulator builder' >&2; exit 2; }
serial=${ANDROID_SERIAL:?Select the existing emulator explicitly with ANDROID_SERIAL}
[[ "$serial" == emulator-* ]] || { echo 'Emulator serial required' >&2; exit 2; }
[[ $(adb -s "$serial" shell getprop ro.kernel.qemu | tr -d '\r') == 1 ]]
root=$(cd ../.. && pwd)
out=$PWD/build/android-forwarding
[[ ! -e "$out" ]] || { echo "$out already exists; preserve evidence and select a fresh checkout" >&2; exit 2; }
mkdir -p "$out"
node= controller= test_started=0 rival_installed=0 control_reverse=0
rival=com.peakpassvpn.ppvpn.acceptance.revoke
cleanup() {
  if [[ "$test_started" == 1 ]]; then
    timeout 20 adb -s "$serial" shell am force-stop com.peakpassvpn.ppvpn || true
  fi
  if [[ "$rival_installed" == 1 ]]; then
    timeout 20 adb -s "$serial" shell am force-stop "$rival" || true
    timeout 20 adb -s "$serial" uninstall "$rival" || true
  fi
  timeout 20 adb -s "$serial" logcat -d >"$out/logcat.txt" 2>&1 || true
  timeout 20 adb -s "$serial" shell ip addr show >"$out/final-addresses.txt" 2>&1 || true
  timeout 20 adb -s "$serial" shell dumpsys connectivity >"$out/final-connectivity.txt" 2>&1 || true
  if [[ "$control_reverse" == 1 ]]; then
    timeout 20 adb -s "$serial" reverse --remove tcp:18090 >"$out/control-reverse-cleanup.txt" 2>&1 || true
    timeout 20 adb -s "$serial" reverse --list >"$out/control-reverse-after.txt" 2>&1 || true
  fi
  [[ -z "$controller" ]] || kill "$controller" 2>/dev/null || true
  [[ -z "$node" ]] || kill "$node" 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT
adb -s "$serial" root
adb -s "$serial" wait-for-device
[[ $(adb -s "$serial" shell id -u | tr -d '\r') == 0 ]]
# Refuse to displace a VPN the runner did not start.
adb -s "$serial" shell ip -o link show >"$out/initial-links.txt"
if grep -Eq ' tun[0-9]+[:@]' "$out/initial-links.txt"; then
  echo 'Existing TUN: use a fresh emulator, do not take it over' >&2; exit 2
fi
if [[ -n $(adb -s "$serial" shell pm path com.peakpassvpn.ppvpn | tr -d '\r') ]]; then
  echo 'Product already installed: use a fresh emulator to preserve existing data' >&2; exit 2
fi
git -C "$root" rev-parse HEAD >"$out/source-head.txt"
adb -s "$serial" shell getprop >"$out/emulator-properties.txt"
# Production fixes these DoT resolvers; do not replace TLS or DNS policy with
# a permissive test transport. Diagnose runner egress before building fixtures.
python3 e2e/android_forwarding/dns_baseline.py --require-tcp \
  --output "$out/dns-egress.json"
"$PWD/e2e/android_forwarding/build-rival.sh" "$out/rival"
adb -s "$serial" install -r -t "$out/rival/rival.apk"
rival_installed=1
adb -s "$serial" shell pm clear "$rival"
mkdir -p "$root/build/test-tools"
# Honor all injected Go caches; no isolated cache or GOBIN install.
(cd "$root/test" && go build -trimpath -o "$root/build/test-tools/androidnode" ./androidnode)
"$root/build/test-tools/androidnode" >"$out/node.jsonl" 2>"$out/node-stderr.txt" & node=$!
python3 e2e/android_forwarding/host.py --serial "$serial" --directory "$out/evidence" \
  --node-log "$out/node.jsonl" >"$out/controller.log" 2>&1 & controller=$!
for _ in $(seq 100); do
  kill -0 "$node" && kill -0 "$controller"
  if grep -q '"event":"ready"' "$out/node.jsonl" && grep -q '^ready' "$out/controller.log"; then break; fi
  sleep .1
done
grep -q '"event":"ready"' "$out/node.jsonl"
grep -q '^ready' "$out/controller.log"
# Only fixture audit/control uses ADB reverse. TEST-NET TCP/UDP and DNS VIP
# probes remain normal App sockets through the real product TUN and SS node.
adb -s "$serial" reverse --list >"$out/control-reverse-before.txt"
if grep -Eq '(^|[[:space:]])tcp:18090([[:space:]]|$)' "$out/control-reverse-before.txt"; then
  echo 'Existing audit reverse mapping: use a fresh emulator' >&2; exit 2
fi
adb -s "$serial" reverse tcp:18090 tcp:18090
control_reverse=1
adb -s "$serial" reverse --list >"$out/control-reverse-active.txt"
# The controller grants the product VPN app-op once after installation, at
# the test's request; it never re-grants during system revoke.
test_started=1
timeout 600 flutter test integration_test/android_forwarding_test.dart -d "$serial" \
  --dart-define=PPVPN_ANDROID_FORWARDING_ACCEPTANCE=true \
  --file-reporter "json:$out/flutter.json" 2>&1 | tee "$out/flutter.log"
# Unlike smoke fallback: require Flutter exit=0 AND independent host verdict.
test -s "$out/evidence/PASS.json"
test ! -e "$out/evidence/failures.jsonl"
echo 'Android real forwarding acceptance passed (owner IPC; see scope document)'
