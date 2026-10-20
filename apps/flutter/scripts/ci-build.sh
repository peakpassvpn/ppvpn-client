#!/usr/bin/env bash
# Builds the Flutter prototype for the runner's platform (ci.yml, job
# flutter): the platform runner (flutter create), the bridge glue
# (flutter_rust_bridge_codegen), analysis, the Rust library (release), the
# app (release), the library put into the bundle. The bundle's path goes to
# $GITHUB_OUTPUT as `bundle`.
# Nothing generated here is committed (see apps/flutter/.gitignore).
set -euo pipefail

app=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$app/../.." && pwd)
cd "$app"

case "$(uname -s)" in
  Linux) os=linux ;;
  Darwin) os=macos ;;
  MINGW* | MSYS*) os=windows ;;
  *) echo "unsupported: $(uname -s)" >&2; exit 1 ;;
esac

# The runner for this platform, then without the counter sample's test.
flutter create --platforms="$os" --org com.peakpassvpn --project-name ppvpn_flutter . >/dev/null
rm -rf test
flutter pub get

if [ "$os" = macos ]; then
  # The prototype writes the system proxy and its own files: no App Sandbox
  # (the native app has none either).
  for f in macos/Runner/DebugProfile.entitlements macos/Runner/Release.entitlements; do
    /usr/libexec/PlistBuddy -c 'Set :com.apple.security.app-sandbox false' "$f"
  done
fi

flutter_rust_bridge_codegen generate
grep -q '^mod frb_generated;' rust/src/lib.rs || echo 'mod frb_generated;' >> rust/src/lib.rs
flutter analyze --no-fatal-infos

(cd "$root" && cargo build --release --locked -p ppvpn-flutter)
flutter build "$os" --release

lib="$root/target/release"
case $os in
  linux)
    bundle=build/linux/x64/release/bundle
    cp "$lib/libppvpn_flutter.so" "$bundle/lib/" ;;
  windows)
    bundle=build/windows/x64/runner/Release
    cp "$lib/ppvpn_flutter.dll" "$bundle/" ;;
  macos)
    bundle=build/macos/Build/Products/Release/ppvpn_flutter.app
    cp "$lib/libppvpn_flutter.dylib" "$bundle/Contents/Frameworks/"
    codesign --force --deep --sign - "$bundle" ;;
esac
echo "bundle=$app/$bundle" >> "${GITHUB_OUTPUT:-/dev/stdout}"
