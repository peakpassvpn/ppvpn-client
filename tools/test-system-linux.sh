#!/usr/bin/env bash
# L3 entry on Linux (make test-system, docs/testing.md): tests that need a
# real TUN, each in network namespaces of their own (test/netns/run.sh), with
# a check that the host's routes, rules, links, nftables and DNS are as they
# were afterwards (gates G3 and G7 of #214). No internet: documentation
# addresses and local fake nodes only.
#
#   tools/test-system-linux.sh [tun|network-change]...   (all parts if none)
#
# Builds run as the calling user; only run.sh runs under sudo (passwordless,
# as on the CI runner). Needs iproute2, nftables, jq, Go (for the helpers in
# test/go.mod) and tcpdump for network-change.
set -euo pipefail
cd "$(dirname "$0")/.."
tmp=${RUNNER_TEMP:-$(mktemp -d)}
parts=("$@")
[ ${#parts[@]} -gt 0 ] || parts=(tun network-change)

tun() {
  uname -r; ls -l /dev/net/tun
  ip -V; nft --version || true

  # run.sh itself: a fake test that adds a rule in the host's namespace
  # must fail it.
  printf '#!/bin/sh\nnsenter -t 1 -n ip rule add priority 31999 lookup main\necho "--- PASS: TestLeak"\n' > "$tmp/leak.test"
  chmod +x "$tmp/leak.test"
  if sudo test/netns/run.sh "$tmp/leak.test"; then
    echo "run.sh did not notice the host change" >&2; sudo ip rule del priority 31999 || true; exit 1
  fi
  sudo ip rule del priority 31999

  # The crate's test binary with the test build's fault-injection feature
  # (never a release's).
  local bin
  bin=$(cargo test -p ppvpn-core --lib --no-run --locked --features fault-injection --message-format=json |
    jq -r 'select(.reason == "compiler-artifact" and .profile.test and .target.name == "ppvpn_core") | .executable')
  test -x "$bin"
  cp "$bin" "$tmp/ppvpn_core.test"

  # The tunrules guard against a real sail TUN: rules put back after a
  # deletion, and reported broken with restoring off.
  sudo NETNS_TIMEOUT=240 test/netns/run.sh --libtest "$tmp/ppvpn_core.test" 'tunrules::linux_tests::'

  # sail's route.default_interface changed by a reload (a new direct
  # connection leaves by the new link, an open one stays), and an instance
  # that fails, a start that fails once routed, a teardown step that panics
  # and a killed instance leave the namespace as it was. Five runs, each in
  # fresh namespaces: a timing flake shows here rather than in someone
  # else's pull request.
  local i
  for i in 1 2 3 4 5; do
    echo "::group::run $i"
    sudo NETNS_TIMEOUT=120 test/netns/run.sh --libtest "$tmp/ppvpn_core.test" 'runtime::netns_tests::'
    echo "::endgroup::"
  done
}

# dns-local and the host-IPv6 re-probe across network changes (#61, #69) on
# the Rust engine (ppvpn-core-lab), strictly: SWITCH_GRACE_MS=0, the change
# reported within 2 s and the first query after it on the new network
# (docs/rust-parity.md). Scripts that build namespaces of their own, through
# run.sh --host for the timeout and the host check.
network_change() {
  cargo build -p ppvpn-core-lab --locked
  (cd test && CGO_ENABLED=0 go build -trimpath -o "$tmp/ldnslab" ./lab/localdns/ldnslab)
  command -v tcpdump >/dev/null || { echo "network-change needs tcpdump" >&2; exit 1; }

  sudo NETNS_TIMEOUT=180 NETNS_ENV="CORE_ENGINE=rust SWITCH_GRACE_MS=0" \
    test/netns/run.sh --host test/lab/localdns/run.sh target/debug/ppvpn-core-lab "$tmp/ldnslab" testdata/profiles/multi-ingress.json "$tmp/localdns"

  # Link down/up, no kernel switch while offline (#69), modes 0-4.
  local status=0 mode
  for mode in 0 1 2 3 4; do
    echo "::group::mode $mode"
    sudo NETNS_TIMEOUT=90 NETNS_ENV="CORE_ENGINE=rust" \
      test/netns/run.sh --host test/lab/localdns/updown.sh target/debug/ppvpn-core-lab "$tmp/ldnslab" testdata/profiles/multi-ingress.json "$mode" "$tmp/updown-$mode" || status=1
    echo "::endgroup::"
  done
  return $status
}

for part in "${parts[@]}"; do
  case $part in
    tun) tun ;;
    network-change) network_change ;;
    *) echo "test-system: unknown part $part (tun, network-change)" >&2; exit 2 ;;
  esac
done
