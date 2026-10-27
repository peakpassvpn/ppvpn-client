#!/usr/bin/env bash
# Build a Release PPVPN.app for one architecture and wrap it in a DMG.
#
# Usage: apps/macos/scripts/package-dmg.sh arm64|x86_64
#   SKIP_BOOTSTRAP=1           reuse the existing PPVPNClient package / project
#   PPVPN_BINARIES_DIR         staged core + service helpers (see embed-binaries.sh)
#   PPVPN_API_BASE_DEFAULT     backend baked into the build (default: the project's)
#   PPVPN_UPDATE_FEED_URL      Sparkle appcast. Unset: the channel's feed on the update
#                              site, <PPVPN_UPDATE_SITE>/desktop/<channel>/appcast-<platform>.xml,
#                              when both the site and the channel are given;
#                              otherwise updates are disabled
#   PPVPN_UPDATE_SITE          base address of the update site
#   PPVPN_SPARKLE_PUBLIC_KEY   Sparkle EdDSA public key
#   SPARKLE_PRIVATE_KEY_FILE   when set, sign the DMG for Sparkle (sign_update)
#   PPVPN_CODESIGN_IDENTITY    signing identity in the keychain search list, e.g.
#                              "PPVPN Code Signing" (default: ad-hoc "-")
#   PPVPN_RELEASE_CHANNEL      dev | stable, recorded in release-meta (unset: null)
#   PPVPN_VERSION              x.y.z overriding the project's MARKETING_VERSION
#   PPVPN_BUILD_NUMBER         monotonic build counter (CI run number; default 0);
#                              CFBundleVersion / sparkle:version = <version>.<n>
#
# Everything in the app (our code, the shared PPVPNClientFFI framework,
# Sparkle and the service helpers) is thinned to the one
# architecture and re-signed inside out (scripts/package-app.sh).
#
# Output (dist/macos), <platform> = macos-arm64 | macos-x64:
#   PPVPN-<version>-<platform>.dmg   Sparkle's updates and manual installs
#   PPVPN-<version>-<platform>.pkg   first installs: Installer puts the app in
#                                    /Applications (packaging/postinstall)
#   release-meta-<platform>.json   what the update site's feed and pointer are written from;
#                                  file/length/sha256/ed_signature are the DMG's,
#                                  install_file/install_length/install_sha256 the pkg's
set -euo pipefail

ARCH="${1:-}"
case "$ARCH" in
  arm64) PLATFORM=macos-arm64 ;;
  x86_64) PLATFORM=macos-x64 ;;
  *) echo "usage: $0 arm64|x86_64" >&2; exit 2 ;;
esac
# Before the build: package-app.sh signs with it.
SIGN_IDENTITY="${PPVPN_CODESIGN_IDENTITY:--}"
if [[ "$SIGN_IDENTITY" != "-" ]] && ! security find-certificate -c "$SIGN_IDENTITY" >/dev/null 2>&1; then
  echo "error: no certificate for \"$SIGN_IDENTITY\" in the keychain search list" >&2; exit 2
fi

CHANNEL="${PPVPN_RELEASE_CHANNEL:-}"
case "$CHANNEL" in ''|dev|stable) ;; *) echo "error: PPVPN_RELEASE_CHANNEL must be dev or stable" >&2; exit 2 ;; esac
BUILD_NUMBER="${PPVPN_BUILD_NUMBER:-0}"
[[ "$BUILD_NUMBER" =~ ^[0-9]+$ ]] || { echo "error: PPVPN_BUILD_NUMBER must be an integer" >&2; exit 2; }
VERSION_OVERRIDE="${PPVPN_VERSION:-}"
[[ -z "$VERSION_OVERRIDE" || "$VERSION_OVERRIDE" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "error: PPVPN_VERSION must be x.y.z" >&2; exit 2; }

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
BUILD_DIR="$APP_DIR/build/release-$ARCH"
FEED_URL="${PPVPN_UPDATE_FEED_URL:-}"
if [[ -z "$FEED_URL" && -n "${PPVPN_UPDATE_SITE:-}" && -n "$CHANNEL" ]]; then
  FEED_URL="${PPVPN_UPDATE_SITE%/}/desktop/$CHANNEL/appcast-$PLATFORM.xml"
fi

if [[ "${SKIP_BOOTSTRAP:-}" != 1 ]]; then
  "$APP_DIR/scripts/bootstrap.sh" --release
fi

MARKETING_VERSION="${VERSION_OVERRIDE:-$(xcodebuild -project "$APP_DIR/PPVPN.xcodeproj" -scheme PPVPN \
  -configuration Release -showBuildSettings 2>/dev/null | awk -F' = ' '/ MARKETING_VERSION = /{print $2; exit}')}"

# Build settings on the command line: a target-level setting wins over the
# environment, so PPVPN_API_BASE_DEFAULT is passed explicitly.
SETTINGS=(
  ARCHS="$ARCH" ONLY_ACTIVE_ARCH=NO
  MARKETING_VERSION="$MARKETING_VERSION"
  CURRENT_PROJECT_VERSION="$MARKETING_VERSION.$BUILD_NUMBER"
  PPVPN_UPDATE_FEED_URL="$FEED_URL"
  PPVPN_SPARKLE_PUBLIC_KEY="${PPVPN_SPARKLE_PUBLIC_KEY:-}"
)
[[ -n "${PPVPN_API_BASE_DEFAULT:-}" ]] && SETTINGS+=(PPVPN_API_BASE_DEFAULT="$PPVPN_API_BASE_DEFAULT")
xcodebuild -project "$APP_DIR/PPVPN.xcodeproj" -scheme PPVPN -configuration Release \
  -derivedDataPath "$BUILD_DIR" -destination 'generic/platform=macOS' "${SETTINGS[@]}" build

APP="$BUILD_DIR/Build/Products/Release/PPVPN.app"

# Thinning, signing, the DMG, the .pkg and release-meta: shared with the
# Flutter app (apps/flutter/scripts/package-macos.sh).
PPVPN_SOURCE_PACKAGES="$BUILD_DIR/SourcePackages" exec "$APP_DIR/scripts/package-app.sh" "$APP" "$ARCH"
