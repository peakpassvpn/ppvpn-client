#!/usr/bin/env bash
# The iOS app's App Store signing for release.yml's ios job (#470).
#
#   ios-signing.sh setup     the Apple Distribution certificate into a
#                            temporary keychain, the App Store profiles from
#                            the App Store Connect API (asc_profiles.py) and
#                            the API key where altool finds it; then
#                            IOS_TEAM_ID and IOS_APP_PROFILE, which
#                            apps/flutter/scripts/ci-build.sh signs with, and
#                            ASC_KEY_DIR into $GITHUB_ENV
#   ios-signing.sh cleanup   the keychain and the key gone again
#
# From the release environment: IOS_DIST_P12 (base64), IOS_DIST_P12_PASSWORD,
# ASC_KEY_ID, ASC_ISSUER_ID, ASC_KEY_P8. The team is the certificate's, so it
# is in no variable.
set -euo pipefail

# The profiles' names in the account, one per bundle ID. The tunnel's joins
# once the packet tunnel extension target exists (#448 step 2).
APP_PROFILE="PPVPN iOS App Store"

keychain="$RUNNER_TEMP/ios-signing.keychain-db"
key_dir="$HOME/.appstoreconnect/private_keys"

case "${1:-}" in
  setup)
    : "${IOS_DIST_P12:?}" "${IOS_DIST_P12_PASSWORD:?}" "${ASC_KEY_ID:?}" "${ASC_ISSUER_ID:?}" "${ASC_KEY_P8:?}"
    keychain_password=$(uuidgen)
    security create-keychain -p "$keychain_password" "$keychain"
    security set-keychain-settings -lut 21600 "$keychain"
    security unlock-keychain -p "$keychain_password" "$keychain"
    p12="$RUNNER_TEMP/ios-dist.p12"
    printf '%s' "$IOS_DIST_P12" | base64 --decode >"$p12"
    security import "$p12" -k "$keychain" -P "$IOS_DIST_P12_PASSWORD" -T /usr/bin/codesign -T /usr/bin/security >/dev/null
    rm -f "$p12"
    security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
    # The runner's login keychain stays in the list after ours.
    existing=()
    while IFS= read -r line; do existing+=("$(echo "$line" | tr -d ' "')"); done < <(security list-keychains -d user)
    security list-keychains -d user -s "$keychain" ${existing[@]+"${existing[@]}"}

    # "Apple Distribution: <name> (<team>)": exactly one.
    identities=$(security find-identity -v -p codesigning "$keychain" | grep -c '"Apple Distribution: ' || true)
    [ "$identities" = 1 ] || { echo "IOS_DIST_P12: want one Apple Distribution identity, found $identities" >&2; exit 1; }
    team=$(security find-identity -v -p codesigning "$keychain" |
      sed -n 's/.*"Apple Distribution: .* (\([A-Z0-9]\{10\}\))".*/\1/p')
    [ -n "$team" ] || { echo "IOS_DIST_P12: no team ID in the identity's name" >&2; exit 1; }

    python3 -I "$(dirname "$0")/asc_profiles.py" "$APP_PROFILE"

    mkdir -p "$key_dir"
    (umask 077 && printf '%s\n' "$ASC_KEY_P8" >"$key_dir/AuthKey_$ASC_KEY_ID.p8")
    {
      echo "IOS_TEAM_ID=$team"
      echo "IOS_APP_PROFILE=$APP_PROFILE"
      echo "ASC_KEY_DIR=$key_dir"
    } >>"$GITHUB_ENV"
    echo "signing for team $team with $APP_PROFILE" ;;
  cleanup)
    security delete-keychain "$keychain" 2>/dev/null || true
    rm -rf "$key_dir" ;;
  *)
    echo "usage: $0 setup|cleanup" >&2
    exit 2 ;;
esac
