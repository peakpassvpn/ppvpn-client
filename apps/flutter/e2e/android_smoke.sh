#!/usr/bin/env bash
# The Android smoke test (#448 steps 3 and 5) on a running emulator (ci.yml,
# job android, after ci-build.sh with FLUTTER_TARGET=android):
# integration_test/android_smoke_test.dart twice, its debug APK carrying the
# release bridge ci-build.sh put in jniLibs. First the tablet in landscape
# (1280 dp across: the desktop layout), then the screen set to a phone's,
# about 390 dp across in portrait (the phone layout below 600).
set -euo pipefail
cd "$(dirname "$0")/.."

adb wait-for-device
device=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
[ -n "$device" ] || { echo "no emulator" >&2; exit 1; }
adb shell settings put system accelerometer_rotation 0

echo "--- tablet, landscape"
adb shell settings put system user_rotation 1
flutter test integration_test/android_smoke_test.dart -d "$device"

echo "--- phone, portrait"
adb shell settings put system user_rotation 0
adb shell wm size 1080x2340
adb shell wm density 440
trap 'adb shell wm size reset; adb shell wm density reset' EXIT
flutter test integration_test/android_smoke_test.dart -d "$device"
