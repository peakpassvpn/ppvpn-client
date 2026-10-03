#!/usr/bin/env bash
# Build a Release PPVPN.app for one architecture and wrap it in a DMG.
#
# Usage: apps/macos/scripts/package-dmg.sh arm64|x86_64
#   SKIP_BOOTSTRAP=1           reuse the existing PPVPNClient package / project
#   PPVPN_BINARIES_DIR         staged core + service helpers (see embed-binaries.sh)
#   PPVPN_API_BASE_DEFAULT     backend baked into the build (default: the project's)
#   PPVPN_UPDATE_FEED_URL      Sparkle appcast; derived from PPVPN_API_BASE_DEFAULT
#                              per platform when unset (neither: updates disabled)
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
# Sparkle, ppvpn-core and the service helpers) is thinned to the one
# architecture and re-signed inside out.
#
# Output (dist/macos), <platform> = macos-arm64 | macos-x64:
#   PPVPN-<version>-<platform>.dmg
#   release-meta-<platform>.json   read by the backend to serve the appcast
set -euo pipefail

ARCH="${1:-}"
case "$ARCH" in
  arm64) PLATFORM=macos-arm64 ;;
  x86_64) PLATFORM=macos-x64 ;;
  *) echo "usage: $0 arm64|x86_64" >&2; exit 2 ;;
esac
SIGN_IDENTITY="${PPVPN_CODESIGN_IDENTITY:--}"
CERT_SHA1=""
if [[ "$SIGN_IDENTITY" != "-" ]]; then
  CERT_SHA1="$(security find-certificate -c "$SIGN_IDENTITY" -Z | awk '/^SHA-1 hash:/ { print $NF; exit }')"
  [[ -n "$CERT_SHA1" ]] || { echo "error: no certificate for \"$SIGN_IDENTITY\" in the keychain search list" >&2; exit 2; }
fi

CHANNEL="${PPVPN_RELEASE_CHANNEL:-}"
case "$CHANNEL" in ''|dev|stable) ;; *) echo "error: PPVPN_RELEASE_CHANNEL must be dev or stable" >&2; exit 2 ;; esac
BUILD_NUMBER="${PPVPN_BUILD_NUMBER:-0}"
[[ "$BUILD_NUMBER" =~ ^[0-9]+$ ]] || { echo "error: PPVPN_BUILD_NUMBER must be an integer" >&2; exit 2; }
VERSION_OVERRIDE="${PPVPN_VERSION:-}"
[[ -z "$VERSION_OVERRIDE" || "$VERSION_OVERRIDE" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] \
  || { echo "error: PPVPN_VERSION must be x.y.z" >&2; exit 2; }

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$APP_DIR/../.." && pwd)"
BUILD_DIR="$APP_DIR/build/release-$ARCH"
OUT_DIR="$REPO_DIR/dist/macos"
FEED_URL="${PPVPN_UPDATE_FEED_URL:-}"
if [[ -z "$FEED_URL" && -n "${PPVPN_API_BASE_DEFAULT:-}" ]]; then
  FEED_URL="$PPVPN_API_BASE_DEFAULT/api/v1/desktop/releases/$PLATFORM/appcast.xml"
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

# --- Thin every Mach-O to $ARCH -------------------------------------------
MACHO="$(mktemp)"
trap 'rm -f "$MACHO"' EXIT
find "$APP" -type f -print0 | while IFS= read -r -d '' file; do
  archs="$(lipo -archs "$file" 2>/dev/null)" || continue
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
sign() {
  local req=()
  if [[ -n "$CERT_SHA1" ]]; then
    local id
    id="$(codesign -dv "$1" 2>&1 | sed -n 's/^Identifier=//p')"
    req=(--requirements "=designated => identifier \"$id\" and certificate leaf = H\"$CERT_SHA1\"")
  fi
  codesign --force --sign "$SIGN_IDENTITY" --options runtime --timestamp=none \
    --preserve-metadata=identifier,entitlements ${req[@]+"${req[@]}"} "$1"
}
# Loose helpers first (ppvpn-core, Sparkle's Autoupdate, ...), then bundles.
while IFS= read -r file; do
  real="$(cd "$(dirname "$file")" && pwd -P)/$(basename "$file")"
  grep -Fxq "$real" <<< "$MAINS" || [[ "$file" == "$APP/Contents/MacOS/PPVPN" ]] || sign "$file"
done < "$MACHO"
while IFS= read -r bundle; do [[ -n "$bundle" ]] && sign "$bundle"; done <<< "$BUNDLES"
sign "$APP"

# --- Checks ------------------------------------------------------------------
codesign --verify --deep --strict "$APP"
if [[ -n "$CERT_SHA1" ]]; then
  for bundle in "$APP" "$APP/Contents/Library/LoginItems/PPVPN Agent.app"; do
    codesign -dr - "$bundle" 2>&1 | grep -qi "certificate leaf = H\"$CERT_SHA1\"" \
      || { echo "error: $bundle lacks the pinned designated requirement" >&2; exit 1; }
  done
fi
while IFS= read -r file; do
  [[ "$(lipo -archs "$file")" == "$ARCH" ]] || { echo "error: $file is not $ARCH only" >&2; exit 1; }
done < "$MACHO"
FFI="$(find "$APP" -type d -name 'PPVPNClientFFI.framework')"
[[ "$FFI" == "$APP/Contents/Frameworks/PPVPNClientFFI.framework" ]] \
  || { echo "error: expected one PPVPNClientFFI.framework in Contents/Frameworks, found: $FFI" >&2; exit 1; }
for exe in "$APP/Contents/MacOS/PPVPN" "$APP/Contents/Library/LoginItems/PPVPN Agent.app/Contents/MacOS/PPVPN Agent"; do
  otool -L "$exe" | grep -q '@rpath/PPVPNClientFFI.framework/Versions/A/PPVPNClientFFI' \
    || { echo "error: $exe does not link the shared PPVPNClientFFI" >&2; exit 1; }
done
for bin in ppvpn-core ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
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

ED_SIGNATURE=""
if [[ -n "${SPARKLE_PRIVATE_KEY_FILE:-}" ]]; then
  SIGN_UPDATE="$(find "$BUILD_DIR/SourcePackages/artifacts" -path '*/bin/sign_update' -type f | head -1)"
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
    "min_os": "$MIN_OS",
    "published_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
}, open(sys.argv[1], "w"), indent=2)
PY
echo "wrote $META"
