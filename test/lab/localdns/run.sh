#!/bin/sh
# Lab test of dns-local following network changes (docs/design-local-dns.md §4.2).
#
#   [CORE_ENGINE=go|rust] run.sh <core> <ldnslab> <profile.json> <out dir>
#
# CORE_ENGINE=go (default): the core is the Go lab build (make
# build-lab-linux: localdns_testsource), which reads the default interface's
# resolvers from $PPVPN_LOCALDNS_TEST_FILE instead of the system; interface
# changes are read from its log lines.
# CORE_ENGINE=rust: the core is ppvpn-core-lab (the Rust engine), which has
# no test source: on Linux its dns-local reads /etc/resolv.conf, as sing-box's
# and sail's `local` do. Each step rewrites the client namespace's
# /etc/netns/ldns-c/resolv.conf in place (ip netns exec bind-mounts that
# file, so it is never replaced); interface changes are the engine's
# NetworkChanged events (watch-events).
# Needs root, iproute2 (with netns), tcpdump and curl on Linux; no jq or GNU date. Everything runs
# in three network namespaces of its own (ldns-c client, ldns-a and ldns-b
# networks); the host's network is not touched. Set CPUS (e.g. 12-15) to pin
# the processes with taskset.
#
# Network A (veth ca, 10.201.0.0/24) has a resolver at .1 answering 192.0.2.1;
# network B (veth cb, 10.202.0.0/24) has one at .1 answering 192.0.2.2 and,
# for the "new network on the same interface" step, one at .53 answering
# 192.0.2.3. A trap resolver on 127.0.0.1:53 in the client answers 192.0.2.99:
# it must never be asked. Every query uses a new name (no cache hits). DNS on
# the TUN is captured: no query to a physical resolver may enter it.
set -eu
mkdir -p "$4"
CORE=$(realpath "$1"); LAB=$(realpath "$2"); PROFILE=$(realpath "$3"); OUT=$(realpath "$4")
rm -rf "$OUT"/*
PIN=""; [ -n "${CPUS:-}" ] && PIN="taskset -c $CPUS"
R=$OUT/run; mkdir -p $R
TESTFILE=$OUT/servers.json
ENGINE=${CORE_ENGINE:-go}
case $ENGINE in go|rust) ;; *) echo "CORE_ENGINE must be go or rust, not $ENGINE" >&2; exit 2 ;; esac
NETNS_ETC=/etc/netns/ldns-c
RESOLV=$NETNS_ETC/resolv.conf
FAILED=0
c() { ip netns exec ldns-c "$@"; }
now() { "$LAB" now; }
log() { echo "$(now) $*" | tee -a "$OUT/steps.log"; }
check() { # description, condition
  if eval "$2"; then log "PASS $1"; else log "FAIL $1"; FAILED=1; fi
}

cleanup() {
  [ -f $R/pid ] && kill $(cat $R/pid) 2>/dev/null || true
  [ -f $R/tcpdump.pid ] && kill $(cat $R/tcpdump.pid) 2>/dev/null || true
  [ -f $R/events.pid ] && kill $(cat $R/events.pid) 2>/dev/null || true
  for pid in $(cat $R/servers.pid 2>/dev/null); do kill $pid 2>/dev/null || true; done
  sleep 0.5
  for ns in ldns-c ldns-a ldns-b; do ip netns del $ns 2>/dev/null || true; done
  if [ "$ENGINE" = rust ]; then rm -rf "$NETNS_ETC"; fi
}
trap cleanup EXIT
cleanup
rm -f $R/pid $R/servers.pid $R/tcpdump.pid $R/events.pid
# Before the first ip netns exec, which bind-mounts what is there then.
if [ "$ENGINE" = rust ]; then mkdir -p "$NETNS_ETC"; : > "$RESOLV"; fi

# Namespaces and links.
for ns in ldns-c ldns-a ldns-b; do ip netns add $ns; ip -n $ns link set lo up; done
# Deleting an interface's first address must keep the others (step 3 swaps
# cb's address): promote_secondaries is off on some kernels' defaults.
ip netns exec ldns-c sysctl -qw net.ipv4.conf.all.promote_secondaries=1 net.ipv4.conf.default.promote_secondaries=1
ip link add ca netns ldns-c type veth peer name ac netns ldns-a
ip link add cb netns ldns-c type veth peer name bc netns ldns-b
ip -n ldns-a addr add 10.201.0.1/24 dev ac; ip -n ldns-a link set ac up
ip -n ldns-b addr add 10.202.0.1/24 dev bc; ip -n ldns-b addr add 10.202.0.53/24 dev bc; ip -n ldns-b link set bc up
ip -n ldns-c addr add 10.201.0.2/24 dev ca; ip -n ldns-c link set ca up
ip -n ldns-c addr add 10.202.0.2/24 dev cb; ip -n ldns-c link set cb up
ip -n ldns-c route add default via 10.201.0.1 dev ca

serve() { # namespace, listen, answer, log name
  ip netns exec $1 $PIN "$LAB" serve -listen $2 -answer $3 > "$OUT/$4.log" 2>&1 &
  echo $! >> $R/servers.pid
}
serve ldns-a 10.201.0.1:53 192.0.2.1 dns-a
serve ldns-b 10.202.0.1:53 192.0.2.2 dns-b
serve ldns-b 10.202.0.53:53 192.0.2.3 dns-b2
serve ldns-c 127.0.0.1:53 192.0.2.99 dns-trap
sleep 0.5

# resolvers <Go test file JSON> <the default interface's resolvers>: what
# the system says now. resolv.conf is written in one go into the same file
# (the bind mount keeps the inode).
resolvers() {
  if [ "$ENGINE" = go ]; then echo "$1" > $TESTFILE; return; fi
  conf=""; for s in $2; do conf="${conf}nameserver $s
"; done
  printf '%s' "$conf" > "$RESOLV"
}
resolvers '{"ca":["10.201.0.1"],"cb":["10.202.0.1"]}' 10.201.0.1

# The core, TUN only, every domain routed direct (dns-local answers all).
"$LAB" apply-body "$PROFILE" > $R/apply.json
TEST_SOURCE=""; [ "$ENGINE" = go ] && TEST_SOURCE="PPVPN_LOCALDNS_TEST_FILE=$TESTFILE"
env ${TEST_SOURCE:+"$TEST_SOURCE"} ip netns exec ldns-c $PIN "$CORE" serve --socket $R/core.sock --session-secret-file $R/secret \
  --state-dir $R/state --log-file "$OUT/core.log" --log-level debug --tun --local-proxy=false > "$OUT/core.stdout" 2>&1 &
echo $! > $R/pid
for i in $(seq 50); do [ -S $R/core.sock ] && [ -s $R/secret ] && break; sleep 0.1; done
c curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST http://core/v1/apply-profile -d @$R/apply.json > $R/apply.out
c curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST http://core/v1/start -d '{}' > $R/start.out
log "apply: $(cat $R/apply.out) start: $(cat $R/start.out)"
# The engine's events: the Rust core's interface changes.
c curl -s -N --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" http://core/v1/watch-events > "$OUT/events.ndjson" 2>/dev/null &
echo $! > $R/events.pid
check "profile applied and started" 'grep -q "\"ok\":true" $R/apply.out && grep -q "\"ok\":true" $R/start.out'
sleep 2

# dns-local's socket must be bound to the physical interface
# (auto_detect_interface): auto_route sends everything else into the TUN, and
# a query to a physical resolver entering the TUN would loop. Capture DNS on
# the TUN for the whole run; queries to the TUN's own resolver 10.60.159.90
# are the positive control that the capture works.
TUN=$(ip -n ldns-c -o -4 addr show | awk '/ 10\.60\.159\.89\// {print $2}')
log "tun interface: ${TUN:-none}"
c tcpdump -l -n -i "$TUN" 'port 53' > "$OUT/tun-dns.txt" 2>"$OUT/tcpdump.err" &
echo $! > $R/tcpdump.pid
sleep 0.5

echo 0 > $R/n
q() { # -> "ok <ip> <ms>" | "fail <why> <ms>"; a new name every time (no cache hits)
  n=$(( $(cat $R/n) + 1 )); echo $n > $R/n
  c $PIN "$LAB" query -server 10.60.159.90:53 -name "q$n.lab.test" -timeout ${QTIMEOUT:-3s}
}
count() { grep -c . "$OUT/$1.log" 2>/dev/null || true; }
changes() { # changes of the default interface to $1 so far
  if [ "$ENGINE" = go ]; then grep -c "msg=\"default interface\" event=changed name=$1 " "$OUT/core.log" || true
  else grep '"type":"NetworkChanged"' "$OUT/events.ndjson" | grep -c "\"interface_name\":\"$1\"" || true; fi
}
# sing-tun reports a default interface change after a 1 s debounce
# (monitor_shared.go delayCheckUpdate); dns-local follows from that event,
# as direct sockets' binding does. Waits for the next one, logs its delay.
# The Rust core must report within 2 s (the default): sail's monitor waits
# for 100 ms of quiet, at most 1 s, and the engine sends NetworkChanged. On the Go baseline
# (SWITCH_GRACE_MS > 0) the front's own monitor can be held back for
# seconds too, as each netlink event restarts the debounce (5146 ms seen on
# a CI runner): it gets 10 s, and the log keeps the time it took.
GRACE=${SWITCH_GRACE_MS:-0}
if [ "$GRACE" -gt 0 ]; then REPORT_MS=${CHANGE_REPORT_MS:-10000}; else REPORT_MS=${CHANGE_REPORT_MS:-2000}; fi
await_change() { # interface, previous count of its changes, what changed
  # Waits for a change reported for that interface: a change of another one
  # (ca's link-local address settling, seen on CI runners) does not count.
  started=$(now)
  while [ "$(changes "$1")" -le "$2" ] && [ $(( $(now) - started )) -lt "$REPORT_MS" ]; do sleep 0.1; done
  CHANGED_AT=$(now)
  took=$(( CHANGED_AT - started )); previous=$2; iface=$1
  log "default interface changed to $1 $took ms after: $3 (limit $REPORT_MS ms)"
  check "default interface change reported within $REPORT_MS ms ($3)" '[ "$(changes "$iface")" -gt "$previous" ]'
}

# The change is logged from the front's interface monitor; dns-local (and
# direct dials) follow the kernel's own monitor, which on Go core 0.5.21 can
# lag by seconds while network events keep coming (each restarts sing-tun's
# 1 s debounce; #45, docs/rust-parity.md). SWITCH_GRACE_MS > 0 (the Go
# baseline in CI) waits up to that long after the reported change for
# dns-local to follow, and logs how long it took; 0 (the default, and the
# requirement for the Rust core) asks the first query after the change.
settle() { # answer regex, what
  [ "$GRACE" -gt 0 ] || return 0
  first=""
  while :; do
    r=$(q); [ -n "$first" ] || first=$r
    echo "$r" | grep -qE "$1" && break
    [ $(( $(now) - CHANGED_AT )) -ge "$GRACE" ] && break
    sleep 0.2
  done
  log "dns-local followed $(( $(now) - CHANGED_AT )) ms after the reported change ($2): $r; the first query got: $first"
}

# Checks carry the #45 dns-local case they cover: D1 (no query to a physical
# resolver enters the TUN), E1 (another interface), E2 (another network on
# the same interface), E3 (no resolvers yet), E5 (the switch with the TUN
# running: every step here), E6 (the 127.0.0.1 trap). E4 is updown.sh. D2
# (the same run on a build without the socket binding must fail D1) needs
# a way to build the core without it; D3 (an ingress given by domain only)
# needs a node: test/lab/engine t4.

# 1. Network A.
r=$(q); log "network A: $r"
check "[E1] network A answered by A" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.1" ]'

# 2. Switch the default route to network B (another interface): the first
# query after the switch must already be answered by B.
resolvers '{"ca":["10.201.0.1"],"cb":["10.202.0.1"]}' 10.202.0.1
k=$(changes cb)
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change cb $k "default route to cb"
settle '^ok 192\.0\.2\.2 ' "default route to cb"
a_before=$(count dns-a)
r=$(q); log "network B: $r"
check "[E1, E5] first query after the change answered by B" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.2" ]'
check "[E1, E5] no query reached A after the change" '[ "$(count dns-a)" = "$a_before" ]'

# 3. Another network on the same interface (a Wi-Fi switch on en0): new
# address on cb, new resolver in the file (each step writes the resolvers
# first, as a DHCP client does before the route).
resolvers '{"ca":["10.201.0.1"],"cb":["10.202.0.53"]}' 10.202.0.53
k=$(changes cb)
ip -n ldns-c addr add 10.202.0.3/24 dev cb; ip -n ldns-c addr del 10.202.0.2/24 dev cb
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change cb $k "new address and resolver on cb"
settle '^ok 192\.0\.2\.3 ' "new address and resolver on cb"
b_before=$(count dns-b)
r=$(q); log "same interface, new network: $r"
check "[E2] same interface, new network answered by the new resolver" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.3" ]'
check "[E2] no query reached the old resolver after the change" '[ "$(count dns-b)" = "$b_before" ]'

# 4. DHCP has not handed out DNS yet: queries fail fast (no 3 s timeout),
# then the servers appear without another interface change and are used
# within RetryInterval. The Rust core: SERVFAIL within 500 ms, recovery
# within 1.5 s. On the Go baseline (SWITCH_GRACE_MS > 0) a loaded CI runner
# can stretch both: 1500 ms and 4000 ms, the log keeping the times taken.
if [ "$GRACE" -gt 0 ]; then FAIL_MS=${FAIL_MS:-1500}; APPEAR_MS=${APPEAR_MS:-4000}; else FAIL_MS=${FAIL_MS:-500}; APPEAR_MS=${APPEAR_MS:-1500}; fi
resolvers '{"ca":["10.201.0.1"],"cb":[]}' ""
k=$(changes cb)
ip -n ldns-c addr add 10.202.0.4/24 dev cb; ip -n ldns-c addr del 10.202.0.3/24 dev cb
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change cb $k "cb without resolvers"
settle '^fail SERVFAIL ' "cb without resolvers"
for i in 1 2 3; do
  r=$(q); log "no resolvers: $r"
  check "[E3] no resolvers: query $i gets SERVFAIL" '[ "$(echo $r | cut -d" " -f1,2)" = "fail SERVFAIL" ]'
  check "[E3] no resolvers: query $i fails within $FAIL_MS ms (took $(echo $r | cut -d" " -f3))" '[ "$(echo $r | cut -d" " -f3)" -lt "$FAIL_MS" ]'
done
resolvers '{"ca":["10.201.0.1"],"cb":["10.202.0.1"]}' 10.202.0.1
APPEAR=$(now); log "resolver appears in the file (no interface change)"
recovered=""
while [ $(( $(now) - APPEAR )) -lt $(( APPEAR_MS + 1000 )) ]; do
  r=$(q)
  if [ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.2" ]; then recovered=$(( $(now) - APPEAR )); break; fi
  sleep 0.1
done
log "recovered after ${recovered:-never} ms (limit $APPEAR_MS ms)"
check "[E3] servers that appear are used within $APPEAR_MS ms (took ${recovered:-never})" '[ -n "$recovered" ] && [ "$recovered" -lt "$APPEAR_MS" ]'

check "[E6] the 127.0.0.1 trap was never asked" '[ "$(count dns-trap)" = 0 ]'
sleep 0.5; kill $(cat $R/tcpdump.pid) 2>/dev/null || true; sleep 0.3
check "[D1] capture on the TUN saw the queries to its own resolver" '[ "$(grep -c "> 10\.60\.159\.90\.53:" "$OUT/tun-dns.txt")" -gt 0 ]'
check "[D1] no query to a physical resolver entered the TUN" '[ -n "$TUN" ] && ! grep -Eq "> 10\.20[12]\.0\.(1|53)\.53:" "$OUT/tun-dns.txt"'
check "[D1] every resolver saw only physical source addresses" '! cat "$OUT/dns-a.log" "$OUT/dns-b.log" "$OUT/dns-b2.log" | grep -v " from 10\.20[12]\.0\.[0-9]*:" | grep -q .'
grep 'msg="local dns servers"' "$OUT/core.log" | tee -a "$OUT/steps.log" || true
check "a local dns servers line per change" '[ "$(grep -c "msg=\"local dns servers\"" "$OUT/core.log")" -ge 4 ]'
log "result: $([ $FAILED = 0 ] && echo PASS || echo FAIL)"
exit $FAILED
