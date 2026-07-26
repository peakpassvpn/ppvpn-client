#!/usr/bin/env bash
set -euo pipefail

framework="${1:-build/PPVPNCore.xcframework}"
if [[ ! -d "${framework}" ]]; then
  echo "XCFramework not found: ${framework}" >&2
  exit 1
fi

plist="${framework}/Info.plist"
device_identifier=""
simulator_identifier=""
library_count="$(
  /usr/libexec/PlistBuddy -c 'Print :AvailableLibraries' "${plist}" |
    grep -c 'Dict {'
)"

for ((index = 0; index < library_count; index++)); do
  platform="$(
    /usr/libexec/PlistBuddy \
      -c "Print :AvailableLibraries:${index}:SupportedPlatform" "${plist}"
  )"
  [[ "${platform}" == "ios" ]] || continue

  identifier="$(
    /usr/libexec/PlistBuddy \
      -c "Print :AvailableLibraries:${index}:LibraryIdentifier" "${plist}"
  )"
  variant="$(
    /usr/libexec/PlistBuddy \
      -c "Print :AvailableLibraries:${index}:SupportedPlatformVariant" "${plist}" \
      2>/dev/null || true
  )"
  if [[ "${variant}" == "simulator" ]]; then
    simulator_identifier="${identifier}"
  elif [[ -z "${variant}" ]]; then
    device_identifier="${identifier}"
  fi
done

[[ -n "${device_identifier}" ]] ||
  { echo "XCFramework is missing its iOS device library" >&2; exit 1; }
[[ -n "${simulator_identifier}" ]] ||
  { echo "XCFramework is missing its iOS simulator library" >&2; exit 1; }

device_binary="${framework}/${device_identifier}/PPVPNCore.framework/PPVPNCore"
simulator_binary="${framework}/${simulator_identifier}/PPVPNCore.framework/PPVPNCore"
[[ -f "${device_binary}" ]] || { echo "iOS device binary is missing" >&2; exit 1; }
[[ -f "${simulator_binary}" ]] || { echo "iOS simulator binary is missing" >&2; exit 1; }

device_architectures="$(xcrun lipo -archs "${device_binary}")"
simulator_architectures="$(xcrun lipo -archs "${simulator_binary}")"
[[ " ${device_architectures} " == *" arm64 "* ]] ||
  { echo "iOS device library is missing arm64" >&2; exit 1; }
for architecture in arm64 x86_64; do
  [[ " ${simulator_architectures} " == *" ${architecture} "* ]] ||
    { echo "iOS simulator library is missing ${architecture}" >&2; exit 1; }
done
