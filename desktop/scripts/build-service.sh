#!/usr/bin/env bash
# Build ppvpn-service binaries and stage them for app packaging (build/binaries).
#
# Usage:
#   scripts/build-service.sh windows   # build for x86_64-pc-windows-msvc
#   scripts/build-service.sh macos     # build both native slices + universal local tools
#   scripts/build-service.sh macos-arm64
#   scripts/build-service.sh macos-x86_64

set -euo pipefail

SCRIPT_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" &>/dev/null && pwd)"
OUT_DIR="$SCRIPT_DIR/../build/binaries"
SERVICE_DIR="$SCRIPT_DIR/../service"
mkdir -p "$OUT_DIR"

target="${1:-windows}"

build_macos_triple() {
  local triple="$1"
  echo "building ppvpn-service for $triple"
  (cd "$SERVICE_DIR" && cargo build --locked --release --target "$triple")
  local src_dir="$SERVICE_DIR/target/$triple/release"
  for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
    cp "$src_dir/$bin" "$OUT_DIR/$bin-$triple"
    chmod 755 "$OUT_DIR/$bin-$triple"
    echo "✓ $OUT_DIR/$bin-$triple"
  done
}

case "$target" in
  windows|win|win64)
    triple="x86_64-pc-windows-msvc"
    suffix=".exe"
    ;;
  macos|darwin)
    triples=("aarch64-apple-darwin" "x86_64-apple-darwin")
    for triple in "${triples[@]}"; do
      build_macos_triple "$triple"
    done
    for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
      lipo -create \
        "$OUT_DIR/$bin-aarch64-apple-darwin" \
        "$OUT_DIR/$bin-x86_64-apple-darwin" \
        -output "$OUT_DIR/$bin-universal-apple-darwin"
      chmod 755 "$OUT_DIR/$bin-universal-apple-darwin"
      echo "✓ $OUT_DIR/$bin-universal-apple-darwin"
    done
    exit 0
    ;;
  macos-arm64|darwin-arm64)
    build_macos_triple "aarch64-apple-darwin"
    exit 0
    ;;
  macos-x86_64|darwin-x86_64)
    build_macos_triple "x86_64-apple-darwin"
    exit 0
    ;;
  *)
    echo "usage: $0 [windows|macos|macos-arm64|macos-x86_64]" >&2
    exit 1
    ;;
esac

echo "building ppvpn-service for $triple"
(cd "$SERVICE_DIR" && cargo build --locked --release --target "$triple")

src_dir="$SERVICE_DIR/target/$triple/release"
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  cp "$src_dir/${bin}${suffix}" "$OUT_DIR/${bin}-${triple}${suffix}"
  echo "✓ $OUT_DIR/${bin}-${triple}${suffix}"
done
