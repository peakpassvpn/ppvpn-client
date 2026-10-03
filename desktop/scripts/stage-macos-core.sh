#!/usr/bin/env bash
set -euo pipefail

# Stages the vendored macOS ppvpn-core for the app: the release ships one
# binary per architecture (verified here against the manifest); they are
# joined into the universal binary embed-binaries.sh embeds (package-dmg.sh
# thins the app per architecture again), next to the two slices.
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "${ROOT_DIR}/vendor/ppvpn-core/CURRENT")"
VENDOR_DIR="${ROOT_DIR}/vendor/ppvpn-core/${VERSION}"
CLI_DEST_DIR="${ROOT_DIR}/build/binaries"

for artifact in macos-cli-arm64 macos-cli-x86_64; do
  node "${ROOT_DIR}/scripts/verify-vendored-core.mjs" \
    --vendor-dir "${VENDOR_DIR}" \
    --artifact "${artifact}" \
    --expected-version "${VERSION}"
done

mkdir -p "${CLI_DEST_DIR}"
install -m 755 "${VENDOR_DIR}/build/ppvpn-core-darwin-arm64" \
  "${CLI_DEST_DIR}/ppvpn-core-aarch64-apple-darwin"
install -m 755 "${VENDOR_DIR}/build/ppvpn-core-darwin-amd64" \
  "${CLI_DEST_DIR}/ppvpn-core-x86_64-apple-darwin"
lipo -create \
  "${CLI_DEST_DIR}/ppvpn-core-aarch64-apple-darwin" \
  "${CLI_DEST_DIR}/ppvpn-core-x86_64-apple-darwin" \
  -output "${CLI_DEST_DIR}/ppvpn-core-universal-apple-darwin"
chmod 755 "${CLI_DEST_DIR}/ppvpn-core-universal-apple-darwin"
echo "Staged ppvpn-core ${VERSION} macOS TUN CLI (arm64 + x86_64 → universal)"
