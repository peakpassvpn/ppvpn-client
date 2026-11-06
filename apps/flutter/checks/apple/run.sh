#!/usr/bin/env bash
# Offline state tests and real Apple SDK typechecks. Never launches an app,
# modifies a profile, registers with APNs, or touches network settings.
set -euo pipefail
app=$(cd "$(dirname "$0")/../.." && pwd)
work=$(mktemp -d "${TMPDIR:-/tmp}/ppvpn-apple-checks.XXXXXX")
trap 'rm -rf "$work"' EXIT
common="$app/runner/apple/ApplePushState.swift"
swiftc -swift-version 5 -module-cache-path "$work/modules" "$common" \
  "$app/checks/apple/ApplePushStateTests.swift" -o "$work/state-tests"
"$work/state-tests"
xcrun swiftc -swift-version 5 -typecheck -target arm64-apple-ios15.0 \
  -sdk "$(xcrun --sdk iphoneos --show-sdk-path)" -module-cache-path "$work/ios-modules" \
  "$common" "$app/runner/ios/Runner/ApplePush.swift"
xcrun swiftc -swift-version 5 -typecheck -target arm64-apple-macos13.0 \
  -sdk "$(xcrun --sdk macosx --show-sdk-path)" -module-cache-path "$work/mac-modules" \
  "$common" "$app/runner/macos/Runner/ApplePush.swift"
echo 'iOS and macOS APNs receivers typechecked against Apple SDKs'
