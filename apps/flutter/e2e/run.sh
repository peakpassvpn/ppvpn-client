#!/usr/bin/env bash
# The signed-in flow end to end (#401 step 1): starts test/fakenode and
# mock_backend.py on loopback, then runs integration_test/signed_in_test.dart
# on this platform's desktop device against them, with the release library
# ci-build.sh built (PPVPN_FLUTTER_LIB) and an empty profile of its own.
# After the test, the fakenode must have seen the request through the node.
# ci.yml (job flutter) runs it after ci-build.sh; Linux runs it inside
# dbus-run-session and Xvfb (the system proxy goes through gsettings and
# dconf). Needs Go (test/go.mod) and Python 3.
set -euo pipefail

app=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$app/../.." && pwd)
work=$(mktemp -d)
trap 'kill $(jobs -p) 2>/dev/null || true' EXIT

case "$(uname -s)" in
  Linux) device=linux lib=$root/target/release/libppvpn_flutter.so exe= ;;
  Darwin) device=macos lib=$root/target/release/libppvpn_flutter.dylib exe= ;;
  *) device=windows lib=$root/target/release/ppvpn_flutter.dll exe=.exe ;;
esac
win() { if command -v cygpath >/dev/null; then cygpath -w "$1"; else echo "$1"; fi; }

ss_port=18388 http_port=18080 api_port=18090
(cd "$root/test" && CGO_ENABLED=0 go build -o "$work/fakenode$exe" ./fakenode)
"$work/fakenode$exe" -ss "127.0.0.1:$ss_port" -key AAAAAAAAAAAAAAAAAAAAAA== \
  -http "127.0.0.1:$http_port" >"$work/fakenode.log" 2>&1 &
python=$(command -v python3 || command -v python)
"$python" "$app/e2e/mock_backend.py" --port "$api_port" --node-port "$ss_port" \
  --log "$work/backend.log" >"$work/backend.out" 2>&1 &
for f in fakenode.log backend.out; do
  for _ in $(seq 100); do grep -q '^ready' "$work/$f" && break; sleep 0.1; done
  grep -q '^ready' "$work/$f" || { echo "::error::$f did not get ready"; cat "$work/$f"; exit 1; }
done

home=$work/home
mkdir -p "$home"
export PPVPN_API_BASE=http://127.0.0.1:$api_port PPVPN_TEST_TRUST_LOCAL_BACKEND=1 \
  PPVPN_FLUTTER_LIB=$(win "$lib") PPVPN_FLUTTER_HOME=$(win "$home") \
  PPVPN_E2E_TARGET=http://127.0.0.1:$http_port/

status=0
(cd "$app" && flutter test integration_test/signed_in_test.dart -d "$device") || status=$?
echo "--- backend requests"; cat "$work/backend.log" || true
echo "--- fakenode"; cat "$work/fakenode.log" || true
if [ $status -ne 0 ]; then
  echo "--- client logs"; find "$home" -name '*.log' -exec tail -n 40 {} + 2>/dev/null || true
  exit $status
fi
grep -q "conn 127.0.0.1:$http_port" "$work/fakenode.log" || {
  echo "::error::the request did not go through the fakenode"; exit 1; }
echo "e2e: ok"
