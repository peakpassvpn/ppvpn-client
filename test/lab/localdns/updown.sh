#!/bin/sh
# Link down/up through the core's TUN: how fast direct traffic comes back, and
# that the host-IPv6 re-probe does not switch kernels while offline (#69).
#
#   [CORE_ENGINE=go|rust] updown.sh <core> <ldnslab> <profile.json> <mode> <out dir>
#
# go (default): the Go lab build (make build-lab-linux: localdns_testsource),
# resolvers from $PPVPN_LOCALDNS_TEST_FILE, kernel switches from its log.
# rust: ppvpn-core-lab; resolvers from the namespace's resolv.conf
# (/etc/netns/ud-c/resolv.conf), kernel switches from its KernelSwitched
# events (watch-events). As run.sh.
# Needs root, iproute2 (with netns) and curl; everything runs in two network
# namespaces of its own (ud-c client, ud-a network), removed on exit.
#
# Modes (the link is down 6 s; IPv6 = a global address and default route):
#   0  no IPv6: no kernel switch at all
#   1  IPv6 before; lost during the outage, back 2 s after the link: the
#      path is the same before and after, so no switch (before #69: two,
#      the first built while offline)
#   2  IPv6 before, never back: one switch, after the link is back
#   3  no IPv6; the core starts while the link is down, up 5 s later
#   4  no IPv6; after the link is up, a host route is added and deleted
#      every 200 ms for 5 s (a burst of netlink events)
# Probe: a direct DNS query through the TUN every ~100 ms (dns-local in the
# kernel, its socket bound to the default interface, like direct-host).
# Checks: recovery within LIMIT_MS (2500 by default, 6000 on the Go baseline:
# SWITCH_GRACE_MS > 0) of the link coming up, and
# the kernel switches before the link came up and in total. Exit status 1 on
# a FAIL.
set -eu
CORE=$(realpath "$1"); LAB=$(realpath "$2"); PROFILE=$(realpath "$3"); MODE=$4
mkdir -p "$5"; OUT=$(realpath "$5"); rm -rf "${OUT:?}"/*
R=$OUT/run; mkdir -p "$R"
# The Rust core must recover within 2.5 s. On the Go baseline
# (SWITCH_GRACE_MS > 0, as CI runs it) the kernel's own interface monitor can
# be held back by each netlink event (#45, docs/rust-parity.md): 3035 ms was
# seen on a CI runner. It gets up to 6 s, and the log keeps the time taken.
GRACE=${SWITCH_GRACE_MS:-0}
if [ "$GRACE" -gt 0 ]; then LIMIT_MS=${LIMIT_MS:-6000}; else LIMIT_MS=${LIMIT_MS:-2500}; fi
ENGINE=${CORE_ENGINE:-go}
case $ENGINE in go|rust) ;; *) echo "CORE_ENGINE must be go or rust, not $ENGINE" >&2; exit 2 ;; esac
NETNS_ETC=/etc/netns/ud-c
now() { "$LAB" now; }
log() { echo "$(now) $*" >> "$OUT/steps.log"; }
c() { ip netns exec ud-c "$@"; }
FAILED=0
check() { # description, condition
	if eval "$2"; then v=PASS; else v=FAIL; FAILED=1; fi
	echo "$v $1" | tee -a "$OUT/steps.log"
}
cleanup() {
	[ -f "$R/pid" ] && kill "$(cat "$R/pid")" 2>/dev/null || true
	[ -f "$R/probe.pid" ] && kill "$(cat "$R/probe.pid")" 2>/dev/null || true
	[ -f "$R/events.pid" ] && kill "$(cat "$R/events.pid")" 2>/dev/null || true
	for pid in $(cat "$R/servers.pid" 2>/dev/null); do kill "$pid" 2>/dev/null || true; done
	sleep 0.5
	for ns in ud-c ud-a; do
		ip netns pids $ns 2>/dev/null | xargs -r kill -9 2>/dev/null || true
		ip netns del $ns 2>/dev/null || true
	done
	if [ "$ENGINE" = rust ]; then rm -rf "$NETNS_ETC"; fi
}
trap cleanup EXIT
cleanup
rm -f "$R/pid" "$R/probe.pid" "$R/servers.pid" "$R/events.pid"
# Before the first ip netns exec, which bind-mounts what is there then.
if [ "$ENGINE" = rust ]; then mkdir -p "$NETNS_ETC"; echo "nameserver 10.201.0.1" > "$NETNS_ETC/resolv.conf"; fi

for ns in ud-c ud-a; do ip netns add $ns; ip -n $ns link set lo up; done
ip link add ca netns ud-c type veth peer name ac netns ud-a
ip -n ud-a addr add 10.201.0.1/24 dev ac; ip -n ud-a link set ac up
ip -n ud-c addr add 10.201.0.2/24 dev ca; ip -n ud-c link set ca up
v6up() {
	ip -n ud-a -6 addr replace 2001:db8:1::1/64 dev ac nodad
	ip -n ud-c -6 addr replace 2001:db8:1::2/64 dev ca nodad
	ip -n ud-c -6 route replace default via 2001:db8:1::1 dev ca
}
ip -n ud-c route add default via 10.201.0.1 dev ca
case $MODE in 1|2) v6up ;; esac
[ "$MODE" = 3 ] && ip -n ud-c link set ca down
ip netns exec ud-a "$LAB" serve -listen 10.201.0.1:53 -answer 192.0.2.1 > "$OUT/dns-a.log" 2>&1 &
echo $! >> "$R/servers.pid"
echo '{"ca":["10.201.0.1"]}' > "$OUT/servers.json"
sleep 0.5
"$LAB" apply-body "$PROFILE" > "$R/apply.json"
TEST_SOURCE=""; [ "$ENGINE" = go ] && TEST_SOURCE="PPVPN_LOCALDNS_TEST_FILE=$OUT/servers.json"
env $TEST_SOURCE ip netns exec ud-c "$CORE" serve --socket "$R/core.sock" --session-secret-file "$R/secret" \
	--state-dir "$R/state" --log-file "$OUT/core.log" --log-level debug --tun --local-proxy=false > "$OUT/core.stdout" 2>&1 &
echo $! > "$R/pid"
for i in $(seq 50); do [ -S "$R/core.sock" ] && [ -s "$R/secret" ] && break; sleep 0.1; done
api() { c curl -s --unix-socket "$R/core.sock" -H "Authorization: Bearer $(cat "$R/secret")" -X POST "http://core/v1/$1" -d "$2"; }
api apply-profile "@$R/apply.json" > "$R/apply.out"
api start '{}' > "$R/start.out"
grep -q '"ok":true' "$R/start.out" || { echo "FAIL start: $(cat "$R/apply.out" "$R/start.out")"; exit 1; }
c curl -s -N --unix-socket "$R/core.sock" -H "Authorization: Bearer $(cat "$R/secret")" http://core/v1/watch-events > "$OUT/events.ndjson" 2>/dev/null &
echo $! > "$R/events.pid"
probe() {
	( n=0; while :; do n=$((n+1)); r=$(c "$LAB" query -server 10.60.159.90:53 -name "p$n.lab.test" -timeout 300ms); echo "$(now) $r" >> "$OUT/probe.log"; sleep 0.1; done ) &
	echo $! > "$R/probe.pid"
}
switches() {
	if [ "$ENGINE" = go ]; then grep -c 'msg="kernel switched"' "$OUT/core.log" || true
	else grep -c '"type":"KernelSwitched"' "$OUT/events.ndjson" || true; fi
}
SW_OFF=0
if [ "$MODE" = 3 ]; then
	probe; sleep 5
	UP=$(now); log "up (the core started offline)"
	c ip link set ca up; c ip route replace default via 10.201.0.1 dev ca
	sleep 15
else
	sleep 3; probe; sleep 2
	log "down"; c ip link set ca down
	sleep 6
	SW_OFF=$(switches); UP=$(now); log "up"
	c ip link set ca up; c ip route replace default via 10.201.0.1 dev ca
	[ "$MODE" = 1 ] && { sleep 2; v6up; log "IPv6 back"; }
	if [ "$MODE" = 4 ]; then
		for i in $(seq 25); do c ip route add 203.0.113.$i/32 dev ca 2>/dev/null; sleep 0.1; c ip route del 203.0.113.$i/32 dev ca 2>/dev/null; sleep 0.1; done
	fi
	sleep 12; [ "$MODE" = 2 ] && sleep 8
fi
kill "$(cat "$R/probe.pid")" 2>/dev/null || true; rm -f "$R/probe.pid"
first=$(awk -v up="$UP" '$1 > up && $2 == "ok" {print $1; exit}' "$OUT/probe.log")
RECOVERED=$(( ${first:-0} > 0 ? ${first:-0} - UP : -1 ))
SW_ALL=$(switches)
echo "mode=$MODE recovered_ms=$RECOVERED switches_before_up=$SW_OFF switches_total=$SW_ALL" | tee -a "$OUT/steps.log"
check "[E4] recovered within ${LIMIT_MS} ms of the link coming up (got $RECOVERED)" '[ "$RECOVERED" -ge 0 ] && [ "$RECOVERED" -le "$LIMIT_MS" ]'
check "[E4] no kernel switch while offline (got $SW_OFF)" '[ "$SW_OFF" = 0 ]'
case $MODE in
2) check "[E4] one kernel switch, after the link came back (got $SW_ALL)" '[ "$SW_ALL" = 1 ]' ;;
*) check "[E4] no kernel switch in all (got $SW_ALL)" '[ "$SW_ALL" = 0 ]' ;;
esac
grep -E 'msg="(default interface|host ipv6|host ipv6 changed|kernel switched)"' "$OUT/core.log" | sed -E 's/ (mtu|index)=[0-9]+//g' | cut -c12-200 >> "$OUT/steps.log"
exit $FAILED
