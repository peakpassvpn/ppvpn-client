#!/usr/bin/env bash
# Runs a core test binary that needs a real TUN (PPVPN_TEST_REAL_TUN=1) in a
# network namespace of its own, and checks that the host was left as it was.
#
#   go test -c -o runtime.test ./internal/runtime        # as a normal user
#   sudo test/netns/run.sh ./runtime.test -test.run 'TestTUNRules' [more test flags]
#   sudo test/netns/run.sh --host <script> [args]        # a script that builds
#                                                        # namespaces of its own
#
# With --host the command runs as it is in the host's namespace (it must keep
# its changes inside namespaces it creates and removes, as the test/lab/localdns
# scripts do); the timeout and the host check are the same, and its exit
# status is the result.
#
# Namespaces (removed again on exit):
#   ppvpn-t: the test's; veth pt0 10.243.0.1/24, default route via 10.243.0.2
#   ppvpn-w: its "uplink"; veth pw0 10.243.0.2/24 (no way out of the machine)
# The TUN, its rules, routes and DNS stay inside ppvpn-t. Before and after,
# the host's own state is captured in the host's namespace: IPv4/IPv6 rules,
# every routing table, links, nftables, /etc/resolv.conf and systemd-resolved's
# per-link DNS (a process in a namespace can still reach the host's resolved
# over D-Bus, with the namespace's interface numbers). Any difference fails.
#
# Needs root, iproute2, nftables (nft) for the ruleset snapshot. Environment:
#   NETNS_TIMEOUT   seconds for the test binary (default 300)
#   NETNS_ENV       extra VAR=value pairs for the test (space separated)
set -euo pipefail

HOST_MODE=0; [ "${1:-}" = --host ] && { HOST_MODE=1; shift; }
BIN=$(realpath "${1:?usage: run.sh [--host] <test binary or script> [args]}"); shift
T=ppvpn-t; W=ppvpn-w
OUT=${NETNS_OUT:-$(mktemp -d)}

# Only what a test could leave behind is compared; values the host changes by
# itself are left out: nftables counters and quotas (the runner's own rules
# count its traffic), routes' remaining lifetimes (RA routes count down),
# link statistics (only link names are listed).
volatile() { sed -E 's/ expires [0-9]+sec//g; s/counter packets [0-9]+ bytes [0-9]+/counter/g; s/quota (over )?[0-9]+ [a-z]+( used [0-9]+ [a-z]+)?/quota/g'; }
snapshot() { # file
	{
		echo "## ip -4 rule"; ip -4 rule show
		echo "## ip -6 rule"; ip -6 rule show
		echo "## ip -4 route (all tables)"; ip -4 route show table all | grep -vE '^(local|broadcast) ' | volatile || true
		echo "## ip -6 route (all tables)"; ip -6 route show table all | grep -vE '^(local|multicast|anycast) ' | volatile || true
		echo "## links"; ip -o link show | awk -F': ' '{print $2}' | sed 's/@.*//' | sort
		echo "## nft"; { nft -s list ruleset 2>/dev/null || echo "(nft unavailable)"; } | volatile
		echo "## resolv.conf"; sha256sum /etc/resolv.conf 2>/dev/null || echo "(none)"
		echo "## resolvectl dns"; resolvectl dns 2>/dev/null || echo "(no systemd-resolved)"
		echo "## resolvectl domain"; resolvectl domain 2>/dev/null || true
	} > "$1"
}

cleanup() {
	for ns in "$T" "$W"; do
		ip netns pids "$ns" 2>/dev/null | xargs -r kill -9 2>/dev/null || true
		ip netns del "$ns" 2>/dev/null || true
	done
}
trap cleanup EXIT
cleanup

snapshot "$OUT/host-before.txt"

status=0
if [ "$HOST_MODE" = 1 ]; then
	# shellcheck disable=SC2086
	env ${NETNS_ENV:-} timeout --kill-after=10 "${NETNS_TIMEOUT:-300}" "$BIN" "$@" > "$OUT/test.txt" 2>&1 || status=$?
	cat "$OUT/test.txt"
	[ "$status" = 124 ] && echo "run.sh: $BIN timed out after ${NETNS_TIMEOUT:-300} s" >&2
else
	for ns in "$T" "$W"; do ip netns add "$ns"; ip -n "$ns" link set lo up; done
	ip link add pt0 netns "$T" type veth peer name pw0 netns "$W"
	ip -n "$T" addr add 10.243.0.1/24 dev pt0; ip -n "$T" link set pt0 up
	ip -n "$W" addr add 10.243.0.2/24 dev pw0; ip -n "$W" link set pw0 up
	ip -n "$T" route add default via 10.243.0.2
	# shellcheck disable=SC2086
	ip netns exec "$T" env PPVPN_TEST_REAL_TUN=1 ${NETNS_ENV:-} \
		timeout --kill-after=10 "${NETNS_TIMEOUT:-300}" "$BIN" -test.v -test.count=1 "$@" > "$OUT/test.txt" 2>&1 || status=$?
	cat "$OUT/test.txt"
	[ "$status" = 124 ] && echo "run.sh: the test binary timed out after ${NETNS_TIMEOUT:-300} s" >&2
	grep -qE '^--- (PASS|FAIL)' "$OUT/test.txt" || { echo "run.sh: no test ran (all skipped?)" >&2; status=1; }
	grep -q '^--- SKIP' "$OUT/test.txt" && grep '^--- SKIP' "$OUT/test.txt" >&2 || true
fi

cleanup
trap - EXIT
sleep 1
snapshot "$OUT/host-after.txt"
if ! diff -u "$OUT/host-before.txt" "$OUT/host-after.txt" > "$OUT/host-diff.txt"; then
	echo "run.sh: the host's network state changed:" >&2
	cat "$OUT/host-diff.txt" >&2
	status=1
else
	echo "run.sh: host routes, rules, links, nftables and DNS unchanged"
fi
echo "run.sh: output in $OUT"
exit "$status"
