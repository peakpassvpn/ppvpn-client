#!/usr/bin/env bash
# The Android smoke test (#448 steps 3 and 5) on a running emulator (ci.yml,
# job android, after ci-build.sh with FLUTTER_TARGET=android):
# integration_test/mobile_smoke_test.dart twice, its debug APK carrying the
# release bridge and tunnel host ci-build.sh put in jniLibs. First a tablet's screen in
# landscape (1280 dp across: the desktop layout), then a phone's, about
# 390 dp across in portrait (the phone layout below 600).
set -euo pipefail
cd "$(dirname "$0")/.."

adb wait-for-device
device=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
[ -n "$device" ] || { echo "no emulator" >&2; exit 1; }
# Both shapes come from the screen size (wm size), at rotation 0: a
# rotation (user_rotation) applied a minute or more after the settings
# change, so the tablet leg ran in portrait and the rotation landed in the
# phone leg, which then had the desktop layout in a phone's height (run
# 40828057797).
adb shell settings put system accelerometer_rotation 0
adb shell settings put system user_rotation 0
# The test's system VPN, allowed as soon as flutter test has installed the
# app (it installs it for each leg and uninstalls it after), long before
# the test gets to it: no consent dialog, which the test cannot answer.
(
  while true; do
    adb shell appops set com.peakpassvpn.ppvpn ACTIVATE_VPN allow >/dev/null 2>&1 || true
    sleep 0.5
  done
) &
grant=$!
adb logcat -c || true
# The emulator can be gone by the time this runs, after both legs passed.
# TunnelService's lines (tag PPVPN), for a failed leg.
trap 'kill "$grant" 2>/dev/null || true; adb logcat -d -s PPVPN:V || true
  adb shell wm size reset || true; adb shell wm density reset || true' EXIT

# A size applies some time after wm size; a leg that started before then
# would lay out for the other shape. Waits until the app area the window
# manager reports is wider than tall (landscape) or taller than wide
# (portrait) on three reads in a row, up to 60 s.
wait_for() { # landscape|portrait
  local size w h seen=0
  for _ in $(seq 60); do
    size=$(adb shell dumpsys window displays | grep -oE ' app=[0-9]+x[0-9]+' | head -1 | cut -d= -f2) || true
    if [ -n "$size" ] && { w=${size%x*} h=${size#*x}; if [ "$1" = landscape ]; then [ "$w" -gt "$h" ]; else [ "$h" -gt "$w" ]; fi; }; then
      seen=$((seen + 1))
      if [ "$seen" -ge 3 ]; then
        echo "$1: app area $size"
        return
      fi
    else
      seen=0
    fi
    sleep 1
  done
  echo "the screen did not turn $1 in 60 s (app area '${size:-unknown}')" >&2
  exit 1
}

echo "--- tablet, landscape"
adb shell wm size 2560x1800
adb shell wm density 320
wait_for landscape
flutter test integration_test/mobile_smoke_test.dart -d "$device"

echo "--- phone, portrait"
adb shell wm size 1080x2340
adb shell wm density 440
wait_for portrait
flutter test integration_test/mobile_smoke_test.dart -d "$device"
