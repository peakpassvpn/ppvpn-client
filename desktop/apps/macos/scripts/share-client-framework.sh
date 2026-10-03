#!/usr/bin/env bash
# Xcode post-build phase of the app: keep exactly one PPVPNClientFFI.framework,
# in PPVPN.app/Contents/Frameworks. Xcode embeds a package's dynamic framework
# in whichever app targets link it (which one varies by configuration); the
# push agent loads the app's copy through its rpath, so a copy inside the
# agent is moved out or dropped, and the agent re-signed without it.
set -euo pipefail

APP="$TARGET_BUILD_DIR/$WRAPPER_NAME"
SHARED="$APP/Contents/Frameworks/PPVPNClientFFI.framework"
AGENT="$APP/Contents/Library/LoginItems/PPVPN Agent.app"
AGENT_COPY="$AGENT/Contents/Frameworks/PPVPNClientFFI.framework"

[[ -d "$AGENT_COPY" ]] || exit 0
if [[ ! -d "$SHARED" ]]; then
  mkdir -p "$APP/Contents/Frameworks"
  mv "$AGENT_COPY" "$SHARED"
else
  rm -rf "$AGENT_COPY"
fi
rmdir "$AGENT/Contents/Frameworks" 2>/dev/null || true
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" --options runtime --timestamp=none \
  --preserve-metadata=identifier,entitlements "$SHARED"
codesign --force --sign "${EXPANDED_CODE_SIGN_IDENTITY:--}" --options runtime --timestamp=none \
  --preserve-metadata=identifier,entitlements "$AGENT"
echo "shared PPVPNClientFFI.framework from Contents/Frameworks"
