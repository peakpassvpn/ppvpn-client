#!/usr/bin/env bash
# Build ppvpn-client as a native library for one target and regenerate the
# C# bindings used by apps/shared/PPVPN.Client (Windows WinUI 3, Linux Gir.Core).
#
# Usage: scripts/build-dotnet.sh <target-triple> [--release]
#
#   x86_64-unknown-linux-gnu   -> runtimes/linux-x64/native/libppvpn_client.so
#   aarch64-unknown-linux-gnu  -> runtimes/linux-arm64/native/libppvpn_client.so
#   x86_64-pc-windows-msvc     -> use scripts/build-dotnet.ps1 on Windows
#
# Writes:
#   apps/shared/PPVPN.Client/Generated/ppvpn_client.cs
#   apps/shared/PPVPN.Client/runtimes/<rid>/native/<native lib>
#
# Environment:
#   UNIFFI_BINDGEN_CS  generator executable (default: uniffi-bindgen-cs on PATH)
#   PPVPN_CARGO_BUILD  build command (default: "cargo build"); e.g. "cross build"
#                      or "cargo zigbuild" to cross-compile Linux from macOS.
#                      With zigbuild a glibc floor may be appended to the triple,
#                      e.g. x86_64-unknown-linux-gnu.2.35. zig drops the UniFFI
#                      metadata the generator reads, so the bindings then come
#                      from a host-native build and the zig library is checked
#                      to export every symbol they call.
#
# Dev only: aarch64-apple-darwin / x86_64-apple-darwin are accepted so the
# bindings can be generated and compiled on a Mac (osx-* runtimes).
set -euo pipefail

BINDGEN_CS_TAG="v0.11.0+v0.31.0"
BINDGEN_CS_VERSION="${BINDGEN_CS_TAG#v}"
BINDGEN_CS_INSTALL="cargo install --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag $BINDGEN_CS_TAG uniffi-bindgen-cs"

usage() {
  sed -n '2,23p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
  exit 2
}

TARGET=""
PROFILE=debug
for arg in "$@"; do
  case "$arg" in
    --release) PROFILE=release ;;
    -h|--help) usage ;;
    -*) echo "error: unknown option: $arg" >&2; usage ;;
    *)
      if [[ -n "$TARGET" ]]; then
        echo "error: more than one target triple given" >&2
        usage
      fi
      TARGET="$arg"
      ;;
  esac
done
[[ -n "$TARGET" ]] || usage

# cargo-zigbuild accepts "<triple>.<glibc>"; rustup and the output dir use <triple>.
TRIPLE="${TARGET%%.*}"
case "$TRIPLE" in
  x86_64-unknown-linux-gnu)  RID=linux-x64;   LIB=libppvpn_client.so ;;
  aarch64-unknown-linux-gnu) RID=linux-arm64; LIB=libppvpn_client.so ;;
  aarch64-apple-darwin)      RID=osx-arm64;   LIB=libppvpn_client.dylib ;;
  x86_64-apple-darwin)       RID=osx-x64;     LIB=libppvpn_client.dylib ;;
  x86_64-pc-windows-msvc)
    echo "error: build $TRIPLE on Windows with scripts/build-dotnet.ps1" >&2
    exit 2
    ;;
  *)
    echo "error: unsupported target triple: $TARGET" >&2
    usage
    ;;
esac

CRATE_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$CRATE_DIR/../.." && pwd)"
PROJECT_DIR="$REPO_DIR/apps/shared/PPVPN.Client"
BINDGEN="${UNIFFI_BINDGEN_CS:-uniffi-bindgen-cs}"

# Check the generator before spending time on the build. The generated code
# must match uniffi 0.31 exactly, so a mismatched version is an error.
if ! command -v "$BINDGEN" >/dev/null 2>&1; then
  echo "error: $BINDGEN not found. Install it with:" >&2
  echo "  $BINDGEN_CS_INSTALL" >&2
  exit 1
fi
FOUND_VERSION="$("$BINDGEN" --version 2>/dev/null | awk '{print $NF}')"
if [[ "$FOUND_VERSION" != "$BINDGEN_CS_VERSION" ]]; then
  echo "error: $BINDGEN is version '${FOUND_VERSION:-unknown}', need $BINDGEN_CS_VERSION. Reinstall with:" >&2
  echo "  $BINDGEN_CS_INSTALL --force" >&2
  exit 1
fi

CARGO_FLAGS=(--locked)
[[ "$PROFILE" == release ]] && CARGO_FLAGS+=(--release)
# Word-split on purpose: PPVPN_CARGO_BUILD may be e.g. "cargo zigbuild".
read -r -a BUILD_CMD <<< "${PPVPN_CARGO_BUILD:-cargo build}"

cd "$CRATE_DIR"
if command -v rustup >/dev/null 2>&1; then
  rustup target add "$TRIPLE" >/dev/null
fi
"${BUILD_CMD[@]}" ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"} --lib --target "$TARGET"

TARGET_DIR="$(cargo metadata --format-version 1 --no-deps | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')"
BUILT_LIB="$TARGET_DIR/$TRIPLE/$PROFILE/$LIB"
if [[ ! -f "$BUILT_LIB" ]]; then
  echo "error: expected build output not found: $BUILT_LIB" >&2
  exit 1
fi

# Bindings are generated from the compiled library so they always match it.
# A zig-linked library has no UniFFI metadata: generate from a host-native
# build of the same sources instead.
METADATA_LIB="$BUILT_LIB"
ZIG=0
[[ " ${BUILD_CMD[*]} " == *zigbuild* ]] && ZIG=1
if [[ "$ZIG" == 1 ]]; then
  cargo build ${CARGO_FLAGS[@]+"${CARGO_FLAGS[@]}"} --lib
  case "$(uname -s)" in
    Darwin) METADATA_LIB="$TARGET_DIR/$PROFILE/libppvpn_client.dylib" ;;
    *)      METADATA_LIB="$TARGET_DIR/$PROFILE/libppvpn_client.so" ;;
  esac
fi
GEN_DIR="$TARGET_DIR/dotnet-bindings/$TRIPLE"
rm -rf "$GEN_DIR" && mkdir -p "$GEN_DIR"
"$BINDGEN" --library "$METADATA_LIB" --crate ppvpn_client \
  --config "$CRATE_DIR/uniffi.toml" --no-format --out-dir "$GEN_DIR"
if [[ ! -f "$GEN_DIR/ppvpn_client.cs" ]]; then
  echo "error: generator did not produce ppvpn_client.cs in $GEN_DIR" >&2
  ls -l "$GEN_DIR" >&2
  exit 1
fi

if [[ "$ZIG" == 1 ]]; then
  # Every FFI symbol the bindings call must be exported by the zig library.
  EXPORTS="$GEN_DIR/exports.txt"
  { nm -D --defined-only "$BUILT_LIB" 2>/dev/null || nm -g "$BUILT_LIB"; } \
    | awk '{print $NF}' | sed 's/^_//' | sort -u >"$EXPORTS"
  MISSING="$(grep -oE '(uniffi|ffi)_ppvpn_client_[A-Za-z0-9_]+' "$GEN_DIR/ppvpn_client.cs" \
    | sort -u | comm -23 - "$EXPORTS")"
  if [[ -n "$MISSING" ]]; then
    echo "error: $BUILT_LIB does not export symbols the bindings call:" >&2
    echo "$MISSING" >&2
    exit 1
  fi
fi

mkdir -p "$PROJECT_DIR/Generated" "$PROJECT_DIR/runtimes/$RID/native"
rm -f "$PROJECT_DIR"/Generated/*.cs
cp "$GEN_DIR/ppvpn_client.cs" "$PROJECT_DIR/Generated/ppvpn_client.cs"
rm -f "$PROJECT_DIR/runtimes/$RID/native/"*
cp "$BUILT_LIB" "$PROJECT_DIR/runtimes/$RID/native/$LIB"

echo "C# bindings: $PROJECT_DIR/Generated/ppvpn_client.cs"
echo "Native lib:  $PROJECT_DIR/runtimes/$RID/native/$LIB ($PROFILE, $TARGET)"
