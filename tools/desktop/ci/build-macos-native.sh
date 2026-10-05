#!/usr/bin/env bash
# Build the native macOS app (apps/macos) as one DMG per architecture, plus
# release metadata for each: macos-arm64 (Apple silicon) and macos-x64
# (Intel), both built on an Apple silicon runner.
#
# Required: CI_PROJECT_DIR
# Optional:
#   MACOS_ARCHS                "arm64 x86_64" (default), or one of them
#   RUST_TOOLCHAIN             default stable
#   PPVPN_BUILD_NUMBER         monotonic counter shared by the release
#   PPVPN_RELEASE_CHANNEL      dev | stable, recorded in release-meta
#   PPVPN_VERSION              x.y.z from the release tag (default: project version)
#   PPVPN_API_BASE_DEFAULT     backend for Release builds
#   PPVPN_UPDATE_SITE          base address of the update site; with a channel, each
#                              build's Sparkle feed is
#                              <site>/desktop/<channel>/appcast-<platform>.xml;
#                              without either, the build has no update feed
#   PPVPN_SPARKLE_PUBLIC_KEY   EdDSA public key; empty disables updates
#   SPARKLE_PRIVATE_KEY        EdDSA private key (base64); signs the DMGs
#   MACOS_SIGNING_P12_BASE64   the fixed self-signed "PPVPN Code Signing"
#   MACOS_SIGNING_P12_PASSWORD   certificate + key; without them builds are ad-hoc
#   MACOS_STAGING_DIR          output directory (default staging/macos-native)
set -euo pipefail

: "${CI_PROJECT_DIR:?CI_PROJECT_DIR is required}"
: "${MACOS_ARCHS:=arm64 x86_64}"
: "${RUST_TOOLCHAIN:=stable}"
: "${PPVPN_BUILD_NUMBER:?PPVPN_BUILD_NUMBER is required}"
: "${PPVPN_API_BASE_DEFAULT:=https://www.peakpassvpn.com}"

for arch in $MACOS_ARCHS; do
  case "$arch" in arm64|x86_64) ;; *) echo "unsupported arch in MACOS_ARCHS: $arch" >&2; exit 2 ;; esac
done

cd "$CI_PROJECT_DIR"
rustup toolchain install "$RUST_TOOLCHAIN" --profile minimal
rustup default "$RUST_TOOLCHAIN"
# ppvpn-client's build-apple.sh builds both slices; each package is thinned.
rustup target add aarch64-apple-darwin x86_64-apple-darwin
command -v xcodegen >/dev/null || brew install xcodegen

# Universal service helpers; package-dmg.sh thins them per build.
bash tools/desktop/build-service.sh macos

# A build for a channel updates from that channel's feed; one without has no feed.
if [[ -n "${PPVPN_RELEASE_CHANNEL:-}" && -z "${PPVPN_UPDATE_SITE:-}" ]]; then
  echo "PPVPN_UPDATE_SITE is required for a channel build" >&2
  exit 3
fi
if [[ -n "${PPVPN_UPDATE_SITE:-}" && ! "$PPVPN_UPDATE_SITE" =~ ^https://[^/[:space:]]+(/[^[:space:]]*)?$ ]]; then
  echo "PPVPN_UPDATE_SITE must be an https URL" >&2
  exit 3
fi
export PPVPN_API_BASE_DEFAULT
export PPVPN_SPARKLE_PUBLIC_KEY="${PPVPN_SPARKLE_PUBLIC_KEY:-}"
export PPVPN_BUILD_NUMBER
unset PPVPN_UPDATE_FEED_URL

key_dir="$(mktemp -d)"
keychain=""
original_keychains=()
cleanup() {
  if [[ -n "$keychain" ]]; then
    security list-keychains -d user -s ${original_keychains[@]+"${original_keychains[@]}"} || true
    security delete-keychain "$keychain" || true
  fi
  rm -rf "$key_dir"
}
trap cleanup EXIT

# Code signing: one fixed self-signed certificate (no Apple Developer ID), so
# the designated requirement, keychain access and SMAppService registration
# carry across updates. A temporary keychain holds it for this build only.
if [[ -n "${MACOS_SIGNING_P12_BASE64:-}" ]]; then
  : "${MACOS_SIGNING_P12_PASSWORD:?MACOS_SIGNING_P12_PASSWORD is required with MACOS_SIGNING_P12_BASE64}"
  keychain="$key_dir/signing.keychain-db"
  keychain_password="$(uuidgen)"
  while IFS= read -r line; do original_keychains+=("$(echo "$line" | tr -d ' "')"); done \
    < <(security list-keychains -d user)
  security create-keychain -p "$keychain_password" "$keychain"
  security set-keychain-settings -lut 21600 "$keychain"
  security unlock-keychain -p "$keychain_password" "$keychain"
  printf '%s' "$MACOS_SIGNING_P12_BASE64" | base64 --decode > "$key_dir/signing.p12"
  security import "$key_dir/signing.p12" -k "$keychain" -P "$MACOS_SIGNING_P12_PASSWORD" -T /usr/bin/codesign >/dev/null
  rm -f "$key_dir/signing.p12"
  security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
  security list-keychains -d user -s "$keychain" ${original_keychains[@]+"${original_keychains[@]}"}
  export PPVPN_CODESIGN_IDENTITY="PPVPN Code Signing"
else
  echo "warning: MACOS_SIGNING_P12_BASE64 not set; signing ad-hoc (not for release)"
fi
if [[ -n "${SPARKLE_PRIVATE_KEY:-}" ]]; then
  [[ -n "$PPVPN_SPARKLE_PUBLIC_KEY" ]] || { echo "SPARKLE_PRIVATE_KEY set without PPVPN_SPARKLE_PUBLIC_KEY" >&2; exit 3; }
  printf '%s' "$SPARKLE_PRIVATE_KEY" > "$key_dir/sparkle.key"
  export SPARKLE_PRIVATE_KEY_FILE="$key_dir/sparkle.key"
else
  echo "warning: SPARKLE_PRIVATE_KEY not set; release-meta ed_signature will be null (not publishable)"
fi

staging_dir="${MACOS_STAGING_DIR:-$CI_PROJECT_DIR/staging/macos-native}"
rm -rf "$staging_dir"
mkdir -p "$staging_dir"

bootstrap=0
for arch in $MACOS_ARCHS; do
  platform=macos-arm64
  [[ "$arch" == x86_64 ]] && platform=macos-x64
  # The client package is built once (universal) and reused.
  SKIP_BOOTSTRAP=$bootstrap apps/macos/scripts/package-dmg.sh "$arch"
  bootstrap=1

  app="apps/macos/build/release-$arch/Build/Products/Release/PPVPN.app"
  expected_signature='Signature=adhoc'
  [[ -n "${PPVPN_CODESIGN_IDENTITY:-}" ]] && expected_signature="Authority=$PPVPN_CODESIGN_IDENTITY"
  if ! codesign -dv --verbose=4 "$app" 2>&1 | grep -qx "$expected_signature"; then
    echo "macOS app ($arch) is not signed as expected ($expected_signature)" >&2
    exit 4
  fi
  feed="$(/usr/libexec/PlistBuddy -c 'Print PPVPNUpdateFeedURL' "$app/Contents/Info.plist")"
  expected=""
  if [[ -n "${PPVPN_UPDATE_SITE:-}" && -n "${PPVPN_RELEASE_CHANNEL:-}" ]]; then
    expected="${PPVPN_UPDATE_SITE%/}/desktop/$PPVPN_RELEASE_CHANNEL/appcast-$platform.xml"
  fi
  [[ "$feed" == "$expected" ]] || { echo "feed URL mismatch ($arch): $feed" >&2; exit 4; }

  cp "dist/macos/"*"-$platform.dmg" "dist/macos/release-meta-$platform.json" "$staging_dir/"
done
ls -l "$staging_dir"
