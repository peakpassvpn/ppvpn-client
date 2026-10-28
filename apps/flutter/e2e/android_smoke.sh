#!/usr/bin/env bash
# The Android smoke test (#448 steps 3 and 5) on a running emulator (ci.yml,
# job android, after ci-build.sh with FLUTTER_TARGET=android):
# integration_test/mobile_smoke_test.dart twice, its debug APK carrying the
# release bridge ci-build.sh put in jniLibs. First the tablet in landscape
# (1280 dp across: the desktop layout), then the screen set to a phone's,
# about 390 dp across in portrait (the phone layout below 600).
set -euo pipefail
cd "$(dirname "$0")/.."

adb wait-for-device
device=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
[ -n "$device" ] || { echo "no emulator" >&2; exit 1; }
adb shell settings put system accelerometer_rotation 0

# The rotation and the screen size apply some time after the settings
# change; a leg that starts before then lays out for the other shape (the
# desktop layout in a phone's height, a RenderFlex overflow). Waits until
# the app area the window manager reports is wider than tall (landscape)
# or taller than wide (portrait), up to 60 s, and goes on with a warning
# if the dump never says.
wait_for() { # landscape|portrait
  local size w h
  for _ in $(seq 60); do
    size=$(adb shell dumpsys window displays | grep -oE ' app=[0-9]+x[0-9]+' | head -1 | cut -d= -f2) || true
    if [ -n "$size" ] && { w=${size%x*} h=${size#*x}; if [ "$1" = landscape ]; then [ "$w" -gt "$h" ]; else [ "$h" -gt "$w" ]; fi; }; then
      echo "$1: app area $size"
      return
    fi
    sleep 1
  done
  echo "warning: the screen did not turn $1 in 60 s (app area '${size:-unknown}')" >&2
}

echo "--- tablet, landscape"
adb shell settings put system user_rotation 1
wait_for landscape
flutter test integration_test/mobile_smoke_test.dart -d "$device"

echo "--- phone, portrait"
adb shell settings put system user_rotation 0
adb shell wm size 1080x2340
adb shell wm density 440
# The emulator can be gone by the time this runs, after both legs passed.
trap 'adb shell wm size reset || true; adb shell wm density reset || true' EXIT
wait_for portrait
flutter test integration_test/mobile_smoke_test.dart -d "$device"
