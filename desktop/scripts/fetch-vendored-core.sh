#!/usr/bin/env bash
# Downloads the vendored Go core's binaries, which are not kept in git: each
# artifact of vendor/ppvpn-core/<CURRENT>/manifest.json comes from the GitHub
# Release the manifest names and is checked against the manifest's size and
# SHA-256 (verify-vendored-core.mjs). A binary already there and correct is
# kept.
#
#   scripts/fetch-vendored-core.sh [ARTIFACT...]
#
# ARTIFACT is a manifest key (windows-x86_64, macos-cli-arm64,
# macos-cli-x86_64, linux-x86_64); default: all of them. Needs curl, jq and node.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
VERSION="$(tr -d '[:space:]' < "${ROOT_DIR}/vendor/ppvpn-core/CURRENT")"
VENDOR_DIR="${ROOT_DIR}/vendor/ppvpn-core/${VERSION}"
MANIFEST="${VENDOR_DIR}/manifest.json"

for tool in curl jq node; do
  command -v "$tool" >/dev/null || { echo "fetch-vendored-core: $tool not found" >&2; exit 1; }
done

REPOSITORY="$(jq -r .source.repository "$MANIFEST")"
TAG="$(jq -r .source.tag "$MANIFEST")"
[ "$REPOSITORY" = "peakpassvpn/ppvpn-core" ] || { echo "fetch-vendored-core: unexpected repository ${REPOSITORY}" >&2; exit 1; }
[ "$TAG" = "v${VERSION}" ] || { echo "fetch-vendored-core: manifest tag ${TAG} is not v${VERSION}" >&2; exit 1; }

if [ "$#" -gt 0 ]; then
  artifacts=("$@")
else
  artifacts=()
  while IFS= read -r key; do artifacts+=("$key"); done < <(jq -r '.artifacts | keys[]' "$MANIFEST")
fi

verify() {
  node "${ROOT_DIR}/scripts/verify-vendored-core.mjs" \
    --vendor-dir "$VENDOR_DIR" --artifact "$1" --expected-version "$VERSION"
}

for artifact in "${artifacts[@]}"; do
  path="$(jq -r --arg k "$artifact" '.artifacts[$k].path // empty' "$MANIFEST")"
  [ -n "$path" ] || { echo "fetch-vendored-core: the manifest has no artifact ${artifact}" >&2; exit 1; }
  case "$path" in build/*) ;; *) echo "fetch-vendored-core: unexpected path ${path}" >&2; exit 1 ;; esac
  dest="${VENDOR_DIR}/${path}"
  if [ -f "$dest" ] && verify "$artifact" >/dev/null 2>&1; then
    echo "kept ${path}"
    continue
  fi
  mkdir -p "$(dirname "$dest")"
  curl --fail --location --silent --show-error --retry 3 \
    --output "${dest}.part" \
    "https://github.com/${REPOSITORY}/releases/download/${TAG}/$(basename "$path")"
  mv "${dest}.part" "$dest"
  case "$path" in *.exe) ;; *) chmod +x "$dest" ;; esac
  verify "$artifact"
done
