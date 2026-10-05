#!/usr/bin/env bash
# L2 entry (make test-integration, docs/testing.md): the CLI's secret store
# against the platform's real one. crates/cli/tests/keystore.rs does
# nothing without PPVPN_TEST_KEYSTORE.
#   Linux: Secret Service, in a D-Bus session of its own with an unlocked
#          login keyring (needs dbus and gnome-keyring; no desktop session)
#   macOS: the login Keychain (a prompt would wait for a person: CI puts a
#          timeout on the step)
# More L2 tests join here as they are marked (feature it-sail).
set -euo pipefail
cd "$(dirname "$0")/.."

cargo test --locked -p ppvpn-cli --test keystore --no-run
case $(uname -s) in
  Linux)
    dbus-run-session -- bash -c '
      set -euo pipefail
      printf "%s" "ci-$RANDOM-$RANDOM" | gnome-keyring-daemon --unlock --components=secrets --daemonize >/dev/null
      PPVPN_TEST_KEYSTORE=1 cargo test --locked -p ppvpn-cli --test keystore -- --nocapture
    '
    ;;
  Darwin)
    PPVPN_TEST_KEYSTORE=1 cargo test --locked -p ppvpn-cli --test keystore -- --nocapture
    ;;
  *)
    echo "test-integration: no platform secret store test on $(uname -s)" >&2
    exit 1
    ;;
esac
