#!/usr/bin/env bash
# Uses the builder's already installed SDK/JDK; downloads nothing.
set -euo pipefail
here=$(cd "$(dirname "$0")" && pwd)
out=${1:?usage: build-rival.sh OUTPUT_DIRECTORY}
sdk=${ANDROID_SDK_ROOT:-${ANDROID_HOME:?Android SDK is required}}
tools=$(find "$sdk/build-tools" -mindepth 1 -maxdepth 1 -type d | sort -V | tail -1)
jar=$(find "$sdk/platforms" -name android.jar | sort -V | tail -1)
test -f "$jar" && test -x "$tools/aapt2" && test -x "$tools/d8"
mkdir -p "$out/classes" "$out/dex"
out=$(cd "$out" && pwd)
"$tools/aapt2" link --manifest "$here/rival/AndroidManifest.xml" -I "$jar" -o "$out/rival-unsigned.apk"
javac -source 8 -target 8 -bootclasspath "$jar" -d "$out/classes" "$here"/rival/*.java
mapfile -t classes < <(find "$out/classes" -name '*.class')
"$tools/d8" --lib "$jar" --min-api 26 --output "$out/dex" "${classes[@]}"
(cd "$out/dex" && zip -q "$out/rival-unsigned.apk" classes.dex)
# Ephemeral fixture key, distinct from product/release signing credentials.
keytool -genkeypair -keystore "$out/rival.jks" -storepass android -keypass android \
  -alias fixture -dname CN=AcceptanceFixture -keyalg RSA -validity 2 -noprompt >/dev/null 2>&1
"$tools/zipalign" -f 4 "$out/rival-unsigned.apk" "$out/rival-aligned.apk"
"$tools/apksigner" sign --ks "$out/rival.jks" --ks-pass pass:android \
  --out "$out/rival.apk" "$out/rival-aligned.apk"
"$tools/apksigner" verify "$out/rival.apk"
