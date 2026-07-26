#!/usr/bin/env bash
set -euo pipefail

framework="${1:-build/PPVPNCore.xcframework}"
if [[ ! -d "${framework}" ]]; then
  echo "XCFramework not found: ${framework}" >&2
  exit 1
fi

library_identifier="$(
  /usr/libexec/PlistBuddy -c 'Print :AvailableLibraries:0:LibraryIdentifier' \
    "${framework}/Info.plist"
)"
platform="$(
  /usr/libexec/PlistBuddy -c 'Print :AvailableLibraries:0:SupportedPlatform' \
    "${framework}/Info.plist"
)"
if [[ "${platform}" != "macos" ]]; then
  echo "XCFramework does not contain the required macOS library" >&2
  exit 1
fi

binary="${framework}/${library_identifier}/PPVPNCore.framework/Versions/A/PPVPNCore"
header="${framework}/${library_identifier}/PPVPNCore.framework/Versions/A/Headers/Mobile.objc.h"
[[ -f "${binary}" ]] || { echo "framework binary is missing" >&2; exit 1; }
[[ -f "${header}" ]] || { echo "generated public header is missing" >&2; exit 1; }

architectures="$(xcrun lipo -archs "${binary}")"
for architecture in arm64 x86_64; do
  if [[ " ${architectures} " != *" ${architecture} "* ]]; then
    echo "XCFramework is missing ${architecture}" >&2
    exit 1
  fi
done

minimum_versions="$(xcrun otool -l "${binary}" | awk '$1 == "minos" { print $2 }' | sort -u)"
if [[ "${minimum_versions}" != "13.0" ]]; then
  echo "unexpected macOS minimum versions: ${minimum_versions}" >&2
  exit 1
fi

for selector in \
  'classifyFlow:' \
  'openFlow:' \
  'decisionJSON:' \
  'localProxyMetadata:' \
  'localProxyCredential:' \
  '@interface MobileFlowConnection'; do
  if ! grep -Fq -- "${selector}" "${header}"; then
    echo "public flow adapter selector is missing: ${selector}" >&2
    exit 1
  fi
done
