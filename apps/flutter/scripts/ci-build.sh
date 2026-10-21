#!/usr/bin/env bash
# Builds the Flutter prototype for the runner's platform (ci.yml, job
# flutter): the platform runner (flutter create, with our files from
# runner/<os> over it), the bridge glue
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
# Our changes to the generated runner (macOS: the activation channel).
if [ -d "runner/$os" ]; then
  cp -R "runner/$os/." "$os/"
fi
flutter pub get

if [ "$os" = macos ]; then
  # The prototype writes the system proxy and its own files: no App Sandbox
  # (the native app has none either).
  for f in macos/Runner/DebugProfile.entitlements macos/Runner/Release.entitlements; do
    /usr/libexec/PlistBuddy -c 'Set :com.apple.security.app-sandbox false' "$f"
  done
  # The ppvpn:// scheme: the push agent opens ppvpn://push/<id> for a click
  # on a notification (as for the native app, apps/macos/PPVPN/Info.plist).
  plist=macos/Runner/Info.plist
  /usr/libexec/PlistBuddy -c 'Delete :CFBundleURLTypes' "$plist" 2>/dev/null || true
  /usr/libexec/PlistBuddy \
    -c 'Add :CFBundleURLTypes array' \
    -c 'Add :CFBundleURLTypes:0 dict' \
    -c 'Add :CFBundleURLTypes:0:CFBundleURLName string com.peakpassvpn.ppvpn.desktop' \
    -c 'Add :CFBundleURLTypes:0:CFBundleURLSchemes array' \
    -c 'Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string ppvpn' \
    "$plist"
fi

# The Windows and Linux runners show the window on Flutter's first frame;
# here it stays hidden until main.dart shows it, so that a `--background`
# start (launch at login) keeps to the tray. A template without the line
# fails the build rather than showing the window after all.
unshow() {
  grep -qF "$2" "$1" || { echo "$1: no '$2' to remove" >&2; exit 1; }
  sed -i "s|$2|$3|" "$1"
}
case $os in
  windows) unshow windows/runner/flutter_window.cpp 'this->Show();' '' ;;
  linux)
    f=linux/runner/my_application.cc
    [ -f "$f" ] || f=linux/my_application.cc
    if grep -qF 'first_frame_cb' "$f"; then
      unshow "$f" 'gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));' ''
    else
      unshow "$f" 'gtk_widget_show(GTK_WIDGET(window));' 'gtk_widget_realize(GTK_WIDGET(window));'
    fi ;;
esac

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
