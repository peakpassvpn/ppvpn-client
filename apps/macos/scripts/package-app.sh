#!/usr/bin/env bash
# Thin a built PPVPN.app to one architecture, re-sign it inside out, and wrap
# it in the DMG, the .pkg and the release metadata. The second half of
# package-dmg.sh (the native app), shared with the Flutter app's
# apps/flutter/scripts/package-macos.sh: both ship the same artifacts.
#
# Usage: apps/macos/scripts/package-app.sh <PPVPN.app> arm64|x86_64
#   PPVPN_CODESIGN_IDENTITY    signing identity in the keychain search list, e.g.
#                              "PPVPN Code Signing" (default: ad-hoc "-")
#   PPVPN_APP_ENTITLEMENTS     sign the app itself with this file (default: keep
#                              the entitlements Xcode signed it with)
#   PPVPN_RELEASE_CHANNEL      dev | stable, recorded in release-meta (unset: null)
#   SPARKLE_PRIVATE_KEY_FILE   when set, sign the DMG for Sparkle (sign_update)
#   PPVPN_SOURCE_PACKAGES      the Xcode build's SourcePackages, where Sparkle's
#                              sign_update is (needed with SPARKLE_PRIVATE_KEY_FILE)
#
# The app is changed in place. It must hold the push agent
# (Contents/Library/LoginItems/PPVPN Agent.app), the one shared
# PPVPNClientFFI.framework and the service helpers in Contents/MacOS.
#
# Output (dist/macos), <platform> = macos-arm64 | macos-x64:
#   PPVPN-<version>-<platform>.dmg   Sparkle's updates and manual installs
#   PPVPN-<version>-<platform>.pkg   first installs: Installer puts the app in
#                                    /Applications (packaging/postinstall)
#   release-meta-<platform>.json   what the update site's feed and pointer are written from;
#                                  file/length/sha256/ed_signature are the DMG's,
#                                  install_file/install_length/install_sha256 the pkg's
set -Eeuo pipefail
# Name the failing command: some (a pipeline in an assignment) fail silently.
trap 'echo "error: ${BASH_SOURCE[0]}:$LINENO: \"$BASH_COMMAND\" exited $?" >&2' ERR

APP="${1:-}"
ARCH="${2:-}"
case "$ARCH" in
  arm64) PLATFORM=macos-arm64 ;;
  x86_64) PLATFORM=macos-x64 ;;
  *) echo "usage: $0 <PPVPN.app> arm64|x86_64" >&2; exit 2 ;;
esac
[[ -f "$APP/Contents/Info.plist" ]] || { echo "error: no app at $APP" >&2; exit 2; }
APP="$(cd "$APP" && pwd)"
SIGN_IDENTITY="${PPVPN_CODESIGN_IDENTITY:--}"
CERT_SHA1=""
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  CERT_SHA1="$(security find-certificate -c "$SIGN_IDENTITY" -Z | awk '/^SHA-1 hash:/ { print $NF; exit }')"
  [[ -n "$CERT_SHA1" ]] || { echo "error: no certificate for \"$SIGN_IDENTITY\" in the keychain search list" >&2; exit 2; }
fi
CHANNEL="${PPVPN_RELEASE_CHANNEL:-}"
case "$CHANNEL" in ''|dev|stable) ;; *) echo "error: PPVPN_RELEASE_CHANNEL must be dev or stable" >&2; exit 2 ;; esac
ENTITLEMENTS="${PPVPN_APP_ENTITLEMENTS:-}"
[[ -z "$ENTITLEMENTS" || -f "$ENTITLEMENTS" ]] || { echo "error: no entitlements at $ENTITLEMENTS" >&2; exit 2; }

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$APP_DIR/../.." && pwd)"
OUT_DIR="$REPO_DIR/dist/macos"

# --- Thin every Mach-O to $ARCH -------------------------------------------
MACHO="$(mktemp)"
trap 'rm -f "$MACHO"' EXIT
find "$APP" -type f -print0 | while IFS= read -r -d '' file; do
  # Not Mach-O: no archs (and no failing command for the ERR trap to report).
  archs="$(lipo -archs "$file" 2>/dev/null || true)"
  [[ -n "$archs" ]] || continue
  echo "$file" >> "$MACHO"
  [[ "$archs" == "$ARCH" ]] && continue
  [[ " $archs " == *" $ARCH "* ]] || { echo "error: $file lacks $ARCH ($archs)" >&2; exit 1; }
  lipo -thin "$ARCH" "$file" -output "$file.thin"
  chmod "$(stat -f %Lp "$file")" "$file.thin"
  mv "$file.thin" "$file"
done

# --- Re-sign inside out ----------------------------------------------------
# Bundles (deepest first) and the main executable each one owns.
BUNDLES="$(find "$APP/Contents" -type d \( -name '*.framework' -o -name '*.xpc' -o -name '*.app' \) \
  | awk -F/ '{ print NF "\t" $0 }' | sort -rn | cut -f2-)"
MAINS=""
while IFS= read -r bundle; do
  [[ -n "$bundle" ]] || continue
  if [[ "$bundle" == *.framework ]]; then
    name="$(basename "$bundle" .framework)"
    MAINS+="$(cd "$bundle/Versions/Current" && pwd -P)/$name"$'\n'
  else
    MAINS+="$bundle/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print CFBundleExecutable' "$bundle/Contents/Info.plist")"$'\n'
  fi
done <<< "$BUNDLES"
# With a certificate, each designated requirement is pinned to the identifier
# and that certificate: a self-signed certificate has no Team ID, and the
# default requirement would otherwise not carry across builds (keychain ACLs,
# SMAppService, notification permission).
# The service helpers get fixed identifiers (the linker's default carries a
# hash): the app and ppvpn-service-install check them before running them as
# root (MacPlatformHooks.swift, install.rs).
sign() {
  local id="" preserve=identifier,entitlements identifier=() entitlements=()
  case "$1" in
    */Contents/MacOS/ppvpn-service | */Contents/MacOS/ppvpn-service-install | */Contents/MacOS/ppvpn-service-uninstall)
      id="com.peakpassvpn.ppvpn.${1##*/ppvpn-}"
      identifier=(--identifier "$id")
      preserve=entitlements
      ;;
  esac
  if [[ "$1" == "$APP" && -n "$ENTITLEMENTS" ]]; then
    entitlements=(--entitlements "$ENTITLEMENTS")
    preserve=identifier
  fi
  codesign --force --sign "$SIGN_IDENTITY" --options runtime --timestamp=none \
    --preserve-metadata=$preserve ${identifier[@]+"${identifier[@]}"} ${entitlements[@]+"${entitlements[@]}"} "$1"
  [[ -n "$CERT_SHA1" ]] || return 0
  # Pin the requirement to the identifier this signature carries: read before
  # signing, it can differ (a linker signature, as the xcframework's
  # PPVPNClientFFI has, does not keep its identifier; x86_64 code has none).
  [[ -n "$id" ]] || id="$(codesign -dv "$1" 2>&1 | sed -n 's/^Identifier=//p')"
  [[ -n "$id" ]] || { echo "error: no identifier in the signature of $1" >&2; exit 1; }
  codesign --force --sign "$SIGN_IDENTITY" --options runtime --timestamp=none \
    --preserve-metadata=entitlements --identifier "$id" ${entitlements[@]+"${entitlements[@]}"} \
    --requirements "=designated => identifier \"$id\" and certificate leaf = H\"$CERT_SHA1\"" "$1"
}
# Loose helpers first (the service helpers, Sparkle's Autoupdate, ...), then bundles.
while IFS= read -r file; do
  real="$(cd "$(dirname "$file")" && pwd -P)/$(basename "$file")"
  grep -Fxq "$real" <<< "$MAINS" || [[ "$file" == "$APP/Contents/MacOS/PPVPN" ]] || sign "$file"
done < "$MACHO"
while IFS= read -r bundle; do [[ -n "$bundle" ]] && sign "$bundle"; done <<< "$BUNDLES"
sign "$APP"

# --- Checks ------------------------------------------------------------------
AGENT="$APP/Contents/Library/LoginItems/PPVPN Agent.app"
# Each bundle on its own first, so that a failure names it.
while IFS= read -r bundle; do
  [[ -z "$bundle" ]] || codesign --verify --strict --verbose=2 "$bundle"
done <<< "$BUNDLES"
codesign --verify --deep --strict --verbose=4 "$APP"
if [[ -n "$CERT_SHA1" ]]; then
  for bundle in "$APP" "$AGENT"; do
    codesign -dr - "$bundle" 2>&1 | grep -qi "certificate leaf = H\"$CERT_SHA1\"" \
      || { echo "error: $bundle lacks the pinned designated requirement" >&2; exit 1; }
  done
fi
for helper in service service-install service-uninstall; do
  file="$APP/Contents/MacOS/ppvpn-$helper"
  [[ "$(codesign -dv "$file" 2>&1 | sed -n 's/^Identifier=//p')" == "com.peakpassvpn.ppvpn.$helper" ]] \
    || { echo "error: $file is not signed as com.peakpassvpn.ppvpn.$helper" >&2; exit 1; }
  [[ -z "$CERT_SHA1" ]] || codesign -dr - "$file" 2>&1 | grep -qi "certificate leaf = H\"$CERT_SHA1\"" \
    || { echo "error: $file lacks the pinned designated requirement" >&2; exit 1; }
done
while IFS= read -r file; do
  [[ "$(lipo -archs "$file")" == "$ARCH" ]] || { echo "error: $file is not $ARCH only" >&2; exit 1; }
done < "$MACHO"
FFI="$(find "$APP" -type d -name 'PPVPNClientFFI.framework')"
[[ "$FFI" == "$APP/Contents/Frameworks/PPVPNClientFFI.framework" ]] \
  || { echo "error: expected one PPVPNClientFFI.framework in Contents/Frameworks, found: $FFI" >&2; exit 1; }
# The native app links the shared framework too; the Flutter app's own
# client is its bridge library instead (libppvpn_flutter.dylib).
CLIENTS=("$AGENT/Contents/MacOS/PPVPN Agent")
[[ -d "$APP/Contents/Frameworks/FlutterMacOS.framework" ]] || CLIENTS+=("$APP/Contents/MacOS/PPVPN")
for exe in "${CLIENTS[@]}"; do
  otool -L "$exe" | grep -q '@rpath/PPVPNClientFFI.framework/Versions/A/PPVPNClientFFI' \
    || { echo "error: $exe does not link the shared PPVPNClientFFI" >&2; exit 1; }
done
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  [[ -f "$APP/Contents/MacOS/$bin" ]] || { echo "error: $bin missing" >&2; exit 1; }
done

plist() { /usr/libexec/PlistBuddy -c "Print $1" "$APP/Contents/Info.plist"; }
VERSION="$(plist CFBundleShortVersionString)"
BUILD="$(plist CFBundleVersion)"
MIN_OS="$(plist LSMinimumSystemVersion)"
DMG="$OUT_DIR/PPVPN-$VERSION-$PLATFORM.dmg"
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE" "$MACHO"' EXIT

cp -R "$APP" "$STAGE/"
ln -s /Applications "$STAGE/Applications"
mkdir -p "$OUT_DIR"
rm -f "$DMG"
hdiutil create -volname "PPVPN" -srcfolder "$STAGE" -fs HFS+ -format UDZO -imagekey zlib-level=9 "$DMG" >/dev/null
codesign --force --sign - "$DMG"
echo "wrote $DMG"

# The first-install package: the same app, installed only to /Applications
# (not relocated to another copy of the bundle id Installer finds, such as
# the mounted DMG), with packaging/postinstall and nothing else. Unsigned:
# Gatekeeper asks once for it, and the app it installs has no quarantine.
PKG="$OUT_DIR/PPVPN-$VERSION-$PLATFORM.pkg"
PKG_STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE" "$MACHO" "$PKG_STAGE"' EXIT
mkdir "$PKG_STAGE/root" "$PKG_STAGE/scripts"
ditto "$APP" "$PKG_STAGE/root/PPVPN.app"
install -m 755 "$APP_DIR/packaging/postinstall" "$PKG_STAGE/scripts/postinstall"
pkgbuild --analyze --root "$PKG_STAGE/root" "$PKG_STAGE/components.plist" >/dev/null
plutil -replace 0.BundleIsRelocatable -bool NO "$PKG_STAGE/components.plist"
plutil -replace 0.BundleIsVersionChecked -bool NO "$PKG_STAGE/components.plist"
rm -f "$PKG"
pkgbuild --root "$PKG_STAGE/root" --component-plist "$PKG_STAGE/components.plist" \
  --scripts "$PKG_STAGE/scripts" --install-location /Applications \
  --identifier com.peakpassvpn.ppvpn.desktop.pkg --version "$BUILD" "$PKG" >/dev/null
pkgutil --payload-files "$PKG" | grep -qx './PPVPN.app/Contents/MacOS/PPVPN' \
  || { echo "error: $PKG does not install PPVPN.app" >&2; exit 1; }
echo "wrote $PKG"

ED_SIGNATURE=""
if [[ -n "${SPARKLE_PRIVATE_KEY_FILE:-}" ]]; then
  : "${PPVPN_SOURCE_PACKAGES:?PPVPN_SOURCE_PACKAGES is required with SPARKLE_PRIVATE_KEY_FILE}"
  SIGN_UPDATE="$(find "$PPVPN_SOURCE_PACKAGES/artifacts" -path '*/bin/sign_update' -type f | head -1)"
  [[ -x "$SIGN_UPDATE" ]] || { echo "error: sign_update not found" >&2; exit 1; }
  # Prints: sparkle:edSignature="<base64>" length="<bytes>"
  ED_SIGNATURE="$("$SIGN_UPDATE" --ed-key-file "$SPARKLE_PRIVATE_KEY_FILE" "$DMG" \
    | sed -n 's/.*sparkle:edSignature="\([^"]*\)".*/\1/p')"
  [[ -n "$ED_SIGNATURE" ]] || { echo "error: sign_update produced no signature" >&2; exit 1; }
fi

# Release metadata (schema 1, agreed across platforms); ed_signature is null
# for unsigned local builds, which must not be published.
META="$OUT_DIR/release-meta-$PLATFORM.json"
python3 - "$META" <<PY
import json, sys
json.dump({
    "schema": 1,
    "platform": "$PLATFORM",
    "channel": "$CHANNEL" or None,
    "version": "$VERSION",
    "build": "$BUILD",
    "file": "$(basename "$DMG")",
    "length": $(stat -f %z "$DMG"),
    "sha256": "$(shasum -a 256 "$DMG" | cut -d' ' -f1)",
    "ed_signature": "$ED_SIGNATURE" or None,
    "install_file": "$(basename "$PKG")",
    "install_length": $(stat -f %z "$PKG"),
    "install_sha256": "$(shasum -a 256 "$PKG" | cut -d' ' -f1)",
    "min_os": "$MIN_OS",
    "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
}, open(sys.argv[1], "w"), indent=2)
PY
echo "wrote $META"
