#!/usr/bin/env bash
# Prepare apps/macos for Xcode: build the ppvpn-client Swift package, then
# generate PPVPN.xcodeproj. Re-run after ppvpn-client's interface changes.
#
# Usage: apps/macos/scripts/bootstrap.sh [--release]
#   PPVPN_CLIENT_CRATE  crate directory (default: <repo>/crates/desktop)
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
REPO_DIR="$(cd "$APP_DIR/../.." && pwd)"
CRATE_DIR="${PPVPN_CLIENT_CRATE:-$REPO_DIR/crates/desktop}"
MODE=--debug
[[ "${1:-}" == --release ]] && MODE=

if [[ ! -x "$CRATE_DIR/scripts/build-apple.sh" ]]; then
  echo "ppvpn-client not found at $CRATE_DIR (set PPVPN_CLIENT_CRATE)" >&2
  exit 1
fi

"$CRATE_DIR/scripts/build-apple.sh" "$APP_DIR/PPVPNClient" $MODE
(cd "$APP_DIR" && xcodegen generate)
