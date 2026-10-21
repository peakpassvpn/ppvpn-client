#!/usr/bin/env bash
# Packages the Flutter app (#401 step 5) as the native macOS release does:
# the same PPVPN.app layout, signing, DMG, .pkg and release-meta, so that
# release.yml can take the macOS artifacts from here instead.
#
# Usage: apps/flutter/scripts/package-macos.sh <PPVPN.app> arm64|x86_64
#   <PPVPN.app> is what scripts/ci-build.sh built; it is copied, not changed.
#   SKIP_BOOTSTRAP=1           reuse the PPVPNClient package and the generated
#                              apps/macos project (apps/macos/scripts/bootstrap.sh)
#   PPVPN_BINARIES_DIR         the service helpers, <bin>-universal-apple-darwin or
#                              <bin>-<triple> (default build/binaries, as
#                              tools/desktop/build-service.sh stages them)
#   and package-app.sh's: PPVPN_CODESIGN_IDENTITY, PPVPN_RELEASE_CHANNEL,
#   SPARKLE_PRIVATE_KEY_FILE.
#
# Into the copy, at the native app's paths (apps/macos/project.yml):
#   Contents/Library/LoginItems/PPVPN Agent.app   the Swift push agent, built
#       from apps/macos (target PPVPNAgent); the app registers it
#       (runner/macos/Runner/MainFlutterWindow.swift, PushAgentService)
#   Contents/Library/LaunchAgents/com.peakpassvpn.ppvpn.desktop.agent.plist
#   Contents/Frameworks/PPVPNClientFFI.framework  the agent's client
#   Contents/MacOS/ppvpn-service{,-install,-uninstall}  what rust/src/hooks.rs
#       runs for enhanced mode
# then apps/macos/scripts/package-app.sh: thinning, inside-out signing with
# the native app's entitlements, and dist/macos/PPVPN-<version>-<platform>.dmg,
# .pkg and release-meta-<platform>.json.
set -euo pipefail

src="${1:-}"
arch="${2:-}"
case "$arch" in
  arm64) triple=aarch64-apple-darwin ;;
  x86_64) triple=x86_64-apple-darwin ;;
  *) echo "usage: $0 <PPVPN.app> arm64|x86_64" >&2; exit 2 ;;
esac
[ -x "$src/Contents/MacOS/PPVPN" ] || { echo "no PPVPN.app at '$src'" >&2; exit 2; }

app_dir=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$app_dir/../.." && pwd)
macos="$root/apps/macos"
bins="${PPVPN_BINARIES_DIR:-$root/build/binaries}"
build="$app_dir/build/macos-package-$arch"

plist() { /usr/libexec/PlistBuddy -c "Print :$1" "$src/Contents/Info.plist"; }
[ "$(plist CFBundleIdentifier)" = com.peakpassvpn.ppvpn.desktop ] ||
  { echo "$src is not com.peakpassvpn.ppvpn.desktop" >&2; exit 2; }
version=$(plist CFBundleShortVersionString)
bundle_version=$(plist CFBundleVersion)

# The push agent, built as package-dmg.sh builds it within the native app,
# with the Flutter app's version.
if [ "${SKIP_BOOTSTRAP:-}" != 1 ]; then
  command -v xcodegen >/dev/null || brew install xcodegen
  "$macos/scripts/bootstrap.sh" --release
fi
xcodebuild -project "$macos/PPVPN.xcodeproj" -scheme PPVPNAgent -configuration Release \
  -derivedDataPath "$build/xcode" -destination 'generic/platform=macOS' \
  ARCHS="$arch" ONLY_ACTIVE_ARCH=NO \
  MARKETING_VERSION="$version" CURRENT_PROJECT_VERSION="$bundle_version" build
agent="$build/xcode/Build/Products/Release/PPVPN Agent.app"
# The one slice the agent links: the xcframework's macOS framework.
ffi=("$macos"/PPVPNClient/PPVPNClientFFI.xcframework/macos-*/PPVPNClientFFI.framework)
[ ${#ffi[@]} = 1 ] && [ -d "${ffi[0]}" ] ||
  { echo "expected one macOS PPVPNClientFFI.framework, found: ${ffi[*]}" >&2; exit 1; }

app="$build/PPVPN.app"
rm -rf "$app"
ditto "$src" "$app"
contents="$app/Contents"
mkdir -p "$contents/Library/LoginItems" "$contents/Library/LaunchAgents" "$contents/Frameworks"
ditto "$agent" "$contents/Library/LoginItems/PPVPN Agent.app"
# Shared from Contents/Frameworks (the agent's rpath), as
# apps/macos/scripts/share-client-framework.sh leaves the native app.
rm -rf "$contents/Library/LoginItems/PPVPN Agent.app/Contents/Frameworks/PPVPNClientFFI.framework"
rmdir "$contents/Library/LoginItems/PPVPN Agent.app/Contents/Frameworks" 2>/dev/null || true
rm -rf "$contents/Frameworks/PPVPNClientFFI.framework"
ditto "${ffi[0]}" "$contents/Frameworks/PPVPNClientFFI.framework"
install -m 644 "$macos/LaunchAgents/com.peakpassvpn.ppvpn.desktop.agent.plist" "$contents/Library/LaunchAgents/"
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  file="$bins/$bin-universal-apple-darwin"
  [ -f "$file" ] || file="$bins/$bin-$triple"
  [ -f "$file" ] || { echo "$bin missing in $bins (tools/desktop/build-service.sh macos)" >&2; exit 1; }
  install -m 755 "$file" "$contents/MacOS/$bin"
done

PPVPN_APP_ENTITLEMENTS="$macos/PPVPN/PPVPN.entitlements" \
PPVPN_SOURCE_PACKAGES="$build/xcode/SourcePackages" \
  "$macos/scripts/package-app.sh" "$app" "$arch"

# What the native app holds besides its own code (package-app.sh checked the
# signatures, the helpers' identifiers and the agent's link).
for path in "Library/LoginItems/PPVPN Agent.app/Contents/MacOS/PPVPN Agent" \
    Library/LaunchAgents/com.peakpassvpn.ppvpn.desktop.agent.plist \
    Frameworks/PPVPNClientFFI.framework/Versions/A/PPVPNClientFFI \
    Frameworks/libppvpn_flutter.dylib MacOS/ppvpn-service MacOS/ppvpn-service-install \
    MacOS/ppvpn-service-uninstall; do
  [ -e "$contents/$path" ] || { echo "$path missing in $app" >&2; exit 1; }
done
[ "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' \
  "$contents/Library/LoginItems/PPVPN Agent.app/Contents/Info.plist")" = com.peakpassvpn.ppvpn.desktop.agent ] ||
  { echo "the push agent is not com.peakpassvpn.ppvpn.desktop.agent" >&2; exit 1; }
echo "app=$app" >> "${GITHUB_OUTPUT:-/dev/stdout}"
