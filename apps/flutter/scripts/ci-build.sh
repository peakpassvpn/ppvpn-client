#!/usr/bin/env bash
# Builds the Flutter app for the runner's platform (ci.yml, job flutter;
# release.yml's installer jobs): the platform runner (flutter create, with
# our files from runner/<os> over it and the native app's identity), the
# bridge glue (flutter_rust_bridge_codegen), analysis, the Rust library
# (release), the app (release), the library put into the bundle. The
# bundle's path goes to $GITHUB_OUTPUT as `bundle`.
# Nothing generated here is committed (see apps/flutter/.gitignore).
#
# Optional, as release.yml passes them:
#   PPVPN_VERSION        x.y.z (default: the workspace's, Cargo.toml)
#   PPVPN_BUILD_NUMBER   the release's one counter, DESKTOP_BUILD_OFFSET plus
#                        its run number, 0-65535 (default 0): the fourth part
#                        of the Windows file version, and CFBundleVersion
#                        <version>.<build> on macOS, as for the native apps
#   MACOS_ARCHS          macOS: "arm64 x86_64" builds the library for both and
#                        joins them, for the release's two DMGs (default: the
#                        runner's architecture only)
#   and what rust/src/update.rs and bridge.rs bake into the library:
#   PPVPN_UPDATE_SITE, PPVPN_RELEASE_CHANNEL, PPVPN_SPARKLE_PUBLIC_KEY,
#   PPVPN_API_BASE_DEFAULT
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

version=${PPVPN_VERSION:-$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml")}
build=${PPVPN_BUILD_NUMBER:-0}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "version must be x.y.z, got '$version'" >&2; exit 1; }
[[ $build =~ ^[0-9]+$ ]] && [ "$build" -le 65535 ] ||
  { echo "build number must be 0-65535, got '$build'" >&2; exit 1; }
# A build for a channel updates from that channel's feed (as the native
# builds require it): without the site its updates would silently be off.
if [ -n "${PPVPN_RELEASE_CHANNEL:-}" ] && [ -z "${PPVPN_UPDATE_SITE:-}" ]; then
  echo "PPVPN_UPDATE_SITE is required for a channel build" >&2
  exit 1
fi
if [ -n "${PPVPN_UPDATE_SITE:-}" ] && [[ ! $PPVPN_UPDATE_SITE =~ ^https://[^/[:space:]]+(/[^[:space:]]*)?$ ]]; then
  echo "PPVPN_UPDATE_SITE must be an https URL" >&2
  exit 1
fi

# Replaces the template's $2 (a basic regex) with $3 in $1. A template
# without it fails the build rather than keeping the generated value.
replace() {
  grep -q -- "$2" "$1" || { echo "$1: no '$2' to replace" >&2; exit 1; }
  sed -i.orig "s|$2|$3|" "$1"
  rm "$1.orig"
}

# The runner for this platform, then without the counter sample's test.
flutter create --platforms="$os" --org com.peakpassvpn --project-name ppvpn_flutter . >/dev/null
rm -rf test
# The native app's icon replaces the template's (other file names).
if [ "$os" = macos ]; then
  rm -rf macos/Runner/Assets.xcassets/AppIcon.appiconset
fi
# Our changes to the generated runner (macOS: the activation channel; the
# icons, from apps/macos and apps/windows).
if [ -d "runner/$os" ]; then
  cp -R "runner/$os/." "$os/"
fi
flutter pub get
cc=linux/runner/my_application.cc
[ -f "$cc" ] || cc=linux/my_application.cc

# The native apps' identity (#401 step 5): what the installers, updates,
# the service, login items and the push agent know the app by.
case $os in
  macos)
    # PPVPN.app, as apps/macos/project.yml (target PPVPN).
    f=macos/Runner/Configs/AppInfo.xcconfig
    replace "$f" '^PRODUCT_NAME = .*' 'PRODUCT_NAME = PPVPN'
    replace "$f" '^PRODUCT_BUNDLE_IDENTIFIER = .*' 'PRODUCT_BUNDLE_IDENTIFIER = com.peakpassvpn.ppvpn.desktop'
    replace "$f" '^PRODUCT_COPYRIGHT = .*' 'PRODUCT_COPYRIGHT = © 2026 PeakPass Labs LLC'
    # LSMinimumSystemVersion is the deployment target: macOS 13, as native.
    replace macos/Runner.xcodeproj/project.pbxproj \
      'MACOSX_DEPLOYMENT_TARGET = [0-9.]*;' 'MACOSX_DEPLOYMENT_TARGET = 13.0;'
    # The application menu's "About", "Hide" and "Quit" items.
    xib=macos/Runner/Base.lproj/MainMenu.xib
    grep -qE 'APP_NAME|ppvpn_flutter' "$xib" || { echo "$xib: no app name to replace" >&2; exit 1; }
    sed -i.orig -e '/title=/s/APP_NAME/PPVPN/g' -e '/title=/s/ppvpn_flutter/PPVPN/g' "$xib"
    rm "$xib.orig"
    plist=macos/Runner/Info.plist
    for key in CFBundleDisplayName LSApplicationCategoryType LSUIElement CFBundleLocalizations; do
      /usr/libexec/PlistBuddy -c "Delete :$key" "$plist" 2>/dev/null || true
    done
    /usr/libexec/PlistBuddy \
      -c 'Add :CFBundleDisplayName string PPVPN' \
      -c 'Add :LSApplicationCategoryType string public.app-category.utilities' \
      -c 'Add :LSUIElement bool false' \
      -c 'Add :CFBundleLocalizations array' \
      -c 'Add :CFBundleLocalizations:0 string zh-Hans' \
      -c 'Add :CFBundleLocalizations:1 string en' \
      -c 'Set :CFBundleVersion $(FLUTTER_BUILD_NAME).$(FLUTTER_BUILD_NUMBER)' \
      "$plist"
    # The prototype writes the system proxy and its own files: no App Sandbox
    # (the native app has none either).
    for f in macos/Runner/DebugProfile.entitlements macos/Runner/Release.entitlements; do
      /usr/libexec/PlistBuddy -c 'Set :com.apple.security.app-sandbox false' "$f"
    done
    # The ppvpn:// scheme: the push agent opens ppvpn://push/<id> for a click
    # on a notification (as for the native app, apps/macos/PPVPN/Info.plist).
    /usr/libexec/PlistBuddy -c 'Delete :CFBundleURLTypes' "$plist" 2>/dev/null || true
    /usr/libexec/PlistBuddy \
      -c 'Add :CFBundleURLTypes array' \
      -c 'Add :CFBundleURLTypes:0 dict' \
      -c 'Add :CFBundleURLTypes:0:CFBundleURLName string com.peakpassvpn.ppvpn.desktop' \
      -c 'Add :CFBundleURLTypes:0:CFBundleURLSchemes array' \
      -c 'Add :CFBundleURLTypes:0:CFBundleURLSchemes:0 string ppvpn' \
      "$plist" ;;
  windows)
    # ppvpn.exe and its version resource, as apps/windows
    # (PPVPN.Windows.csproj, Directory.Build.props).
    replace windows/CMakeLists.txt '^set(BINARY_NAME ".*")' 'set(BINARY_NAME "ppvpn")'
    replace windows/runner/main.cpp 'window.Create(L"[^"]*"' 'window.Create(L"PPVPN"'
    rc=windows/runner/Runner.rc
    # UTF-8 for the copyright sign.
    if ! grep -qF '#pragma code_page(65001)' "$rc"; then
      { printf '#pragma code_page(65001)\n'; cat "$rc"; } >"$rc.new"
      mv "$rc.new" "$rc"
    fi
    for kv in 'CompanyName=PeakPass Labs LLC' 'FileDescription=PPVPN' 'InternalName=ppvpn' \
        'LegalCopyright=© PeakPass Labs LLC' 'OriginalFilename=ppvpn.exe' 'ProductName=PPVPN'; do
      replace "$rc" "VALUE \"${kv%%=*}\", \"[^\"]*\"" "VALUE \"${kv%%=*}\", \"${kv#*=}\""
    done ;;
  linux)
    # /usr/lib/ppvpn/ppvpn and the GApplication id, as apps/linux
    # (PPVPNApplication.Id): the desktop entry and the window's class.
    replace linux/CMakeLists.txt '^set(BINARY_NAME ".*")' 'set(BINARY_NAME "ppvpn")'
    replace linux/CMakeLists.txt '^set(APPLICATION_ID ".*")' 'set(APPLICATION_ID "com.peakpassvpn.ppvpn.desktop")'
    replace "$cc" 'gtk_header_bar_set_title(header_bar, "[^"]*");' 'gtk_header_bar_set_title(header_bar, "PPVPN");'
    replace "$cc" 'gtk_window_set_title(window, "[^"]*");' 'gtk_window_set_title(window, "PPVPN");'
    # The program name, so the X11 WM_CLASS, is the application id: the
    # desktop entry's StartupWMClass (packaging/linux).
    grep -qF 'g_set_prgname(APPLICATION_ID);' "$cc" ||
      { echo "$cc: the program name is no longer the application id" >&2; exit 1; } ;;
esac

# The Windows and Linux runners show the window on Flutter's first frame;
# here it stays hidden until main.dart shows it, so that a `--background`
# start (launch at login) keeps to the tray. A template without the line
# fails the build rather than showing the window after all.
case $os in
  windows) replace windows/runner/flutter_window.cpp 'this->Show();' '' ;;
  linux)
    if grep -qF 'first_frame_cb' "$cc"; then
      replace "$cc" 'gtk_widget_show(gtk_widget_get_toplevel(GTK_WIDGET(view)));' ''
    else
      replace "$cc" 'gtk_widget_show(GTK_WIDGET(window));' 'gtk_widget_realize(GTK_WIDGET(window));'
    fi ;;
esac

flutter_rust_bridge_codegen generate
grep -q '^mod frb_generated;' rust/src/lib.rs || echo 'mod frb_generated;' >> rust/src/lib.rs
flutter analyze --no-fatal-infos

lib="$root/target/release"
if [ "$os" = macos ] && [ -n "${MACOS_ARCHS:-}" ]; then
  # Flutter's release app is universal; package-app.sh thins each DMG's copy.
  slices=()
  for arch in $MACOS_ARCHS; do
    case $arch in
      arm64) triple=aarch64-apple-darwin ;;
      x86_64) triple=x86_64-apple-darwin ;;
      *) echo "unsupported arch in MACOS_ARCHS: $arch" >&2; exit 1 ;;
    esac
    (cd "$root" && cargo build --release --locked -p ppvpn-flutter --target "$triple")
    slices+=("$root/target/$triple/release/libppvpn_flutter.dylib")
  done
  lib="$root/target/universal-apple-darwin/release"
  mkdir -p "$lib"
  lipo -create "${slices[@]}" -output "$lib/libppvpn_flutter.dylib"
else
  (cd "$root" && cargo build --release --locked -p ppvpn-flutter)
fi
flutter build "$os" --release --build-name "$version" --build-number "$build"

case $os in
  linux)
    bundle=build/linux/x64/release/bundle
    test -x "$bundle/ppvpn"
    cp "$lib/libppvpn_flutter.so" "$bundle/lib/" ;;
  windows)
    bundle=build/windows/x64/runner/Release
    test -f "$bundle/ppvpn.exe"
    cp "$lib/ppvpn_flutter.dll" "$bundle/" ;;
  macos)
    bundle=build/macos/Build/Products/Release/PPVPN.app
    test -x "$bundle/Contents/MacOS/PPVPN"
    # What the edits above should have given the bundle.
    for kv in CFBundleIdentifier=com.peakpassvpn.ppvpn.desktop LSMinimumSystemVersion=13.0 \
        CFBundleShortVersionString="$version" CFBundleVersion="$version.$build"; do
      got=$(/usr/libexec/PlistBuddy -c "Print :${kv%%=*}" "$bundle/Contents/Info.plist")
      [ "$got" = "${kv#*=}" ] || { echo "${kv%%=*} is '$got', not '${kv#*=}'" >&2; exit 1; }
    done
    cp "$lib/libppvpn_flutter.dylib" "$bundle/Contents/Frameworks/"
    codesign --force --deep --sign - "$bundle" ;;
esac
echo "bundle=$app/$bundle" >> "${GITHUB_OUTPUT:-/dev/stdout}"
