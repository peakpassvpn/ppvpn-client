#!/usr/bin/env bash
# Build Debug PPVPN.app and run it from /Applications/PPVPN.app — the only
# client path the privileged service accepts (service/ checks it strictly),
# so enhanced mode can only be exercised from there.
#
# Usage: apps/macos/scripts/install-debug.sh [--replace] [-- env assignments...]
#   --replace  overwrite an existing non-native (e.g. Tauri) PPVPN.app
#   e.g.  install-debug.sh -- PPVPN_AUTO_SIGN_IN=1
set -euo pipefail

APP_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
TARGET=/Applications/PPVPN.app
REPLACE=0
ENV_ARGS=()
while (($#)); do
  case "$1" in
    --replace) REPLACE=1 ;;
    --) shift; ENV_ARGS=("$@"); break ;;
    *) echo "unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

xcodebuild -project "$APP_DIR/PPVPN.xcodeproj" -scheme PPVPN -configuration Debug \
  -derivedDataPath "$APP_DIR/build" -destination 'platform=macOS' build -quiet
BUILT="$APP_DIR/build/Build/Products/Debug/PPVPN.app"

if [[ -d "$TARGET" ]]; then
  # Native builds carry PPVPNUpdateFeedURL; anything else is another install.
  if ! /usr/libexec/PlistBuddy -c 'Print PPVPNUpdateFeedURL' "$TARGET/Contents/Info.plist" >/dev/null 2>&1 \
     && ((REPLACE == 0)); then
    echo "error: $TARGET is not a native PPVPN build; re-run with --replace to overwrite it" >&2
    exit 1
  fi
fi

pkill -x PPVPN 2>/dev/null && sleep 1 || true
rm -rf "$TARGET"
ditto "$BUILT" "$TARGET"
codesign --verify --deep --strict "$TARGET"
echo "installed $TARGET"
if ((${#ENV_ARGS[@]})); then
  env "${ENV_ARGS[@]}" "$TARGET/Contents/MacOS/PPVPN" >/dev/null 2>&1 &
else
  open "$TARGET"
fi
