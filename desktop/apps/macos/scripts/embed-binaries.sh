#!/usr/bin/env bash
# Xcode pre-build phase (so the final app signature seals them): copy ppvpn-core and the privileged-service helpers into
# Contents/MacOS, where ppvpn-client (core_bin_dir) and ppvpn-service-install
# (current_exe siblings) look for them, and ad-hoc sign each one.
#
# Inputs are the universal binaries staged by the repo scripts:
#   scripts/stage-macos-core.sh && scripts/build-service.sh macos
# Override the source with PPVPN_BINARIES_DIR.
set -euo pipefail

SOURCE_DIR="${PPVPN_BINARIES_DIR:-$SRCROOT/../../build/binaries}"
DEST_DIR="$TARGET_BUILD_DIR/$EXECUTABLE_FOLDER_PATH"
BINARIES=(ppvpn-core ppvpn-service ppvpn-service-install ppvpn-service-uninstall)

missing=()
for bin in "${BINARIES[@]}"; do
  [[ -f "$SOURCE_DIR/$bin-universal-apple-darwin" ]] || missing+=("$bin")
done

if ((${#missing[@]})); then
  message="missing ${missing[*]} in $SOURCE_DIR; run scripts/stage-macos-core.sh and scripts/build-service.sh macos"
  if [[ "$CONFIGURATION" == Release ]]; then
    echo "error: $message"
    exit 1
  fi
  # Debug UI work does not need the cores; enhanced/standard mode will
  # report the missing binary at runtime.
  echo "warning: $message"
  exit 0
fi

mkdir -p "$DEST_DIR"
for bin in "${BINARIES[@]}"; do
  install -m 755 "$SOURCE_DIR/$bin-universal-apple-darwin" "$DEST_DIR/$bin"
  codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" --options runtime --timestamp=none "$DEST_DIR/$bin"
done
echo "embedded ${BINARIES[*]} from $SOURCE_DIR"
