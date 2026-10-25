#!/usr/bin/env bash
# The Android smoke test (#448 step 3) on a running emulator (ci.yml, job
# android, after ci-build.sh with FLUTTER_TARGET=android): the screen held
# in landscape, then integration_test/android_smoke_test.dart, whose debug
# APK carries the release bridge ci-build.sh put in jniLibs.
set -euo pipefail
cd "$(dirname "$0")/.."

adb wait-for-device
adb shell settings put system accelerometer_rotation 0
adb shell settings put system user_rotation 1
device=$(adb devices | awk 'NR > 1 && $2 == "device" { print $1; exit }')
[ -n "$device" ] || { echo "no emulator" >&2; exit 1; }
flutter test integration_test/android_smoke_test.dart -d "$device"
