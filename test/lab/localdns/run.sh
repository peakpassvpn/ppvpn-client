#!/bin/sh
# Lab test of dns-local following network changes (docs/design-local-dns.md §4.2).
#
#   run.sh <lab ppvpn-core> <ldnslab> <profile.json> <out dir>
#
# The core must be a lab build (make build-lab-linux: localdns_testsource),
# which reads the default interface's resolvers from $PPVPN_LOCALDNS_TEST_FILE
# instead of the system. Needs root, iproute2 (with netns), tcpdump and curl on Linux; no jq or GNU date. Everything runs
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
  for pid in $(cat $R/servers.pid 2>/dev/null); do kill $pid 2>/dev/null || true; done
  sleep 0.5
  for ns in ldns-c ldns-a ldns-b; do ip netns del $ns 2>/dev/null || true; done
}
trap cleanup EXIT
cleanup
rm -f $R/pid $R/servers.pid $R/tcpdump.pid

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

echo '{"ca":["10.201.0.1"],"cb":["10.202.0.1"]}' > $TESTFILE

# The core, TUN only, every domain routed direct (dns-local answers all).
"$LAB" apply-body "$PROFILE" > $R/apply.json
PPVPN_LOCALDNS_TEST_FILE=$TESTFILE ip netns exec ldns-c $PIN "$CORE" serve --socket $R/core.sock --session-secret-file $R/secret \
  --state-dir $R/state --log-file "$OUT/core.log" --log-level debug --tun --local-proxy=false > "$OUT/core.stdout" 2>&1 &
echo $! > $R/pid
for i in $(seq 50); do [ -S $R/core.sock ] && [ -s $R/secret ] && break; sleep 0.1; done
c curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST http://core/v1/apply-profile -d @$R/apply.json > $R/apply.out
c curl -s --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" -X POST http://core/v1/start -d '{}' > $R/start.out
log "apply: $(cat $R/apply.out) start: $(cat $R/start.out)"
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
changes() { grep -c 'msg="default interface" event=changed' "$OUT/core.log" || true; }
# sing-tun reports a default interface change after a 1 s debounce
# (monitor_shared.go delayCheckUpdate); dns-local follows from that event,
# as direct sockets' binding does. Waits for the next one, logs its delay.
await_change() { # previous count, what changed
  started=$(now)
  for i in $(seq 50); do [ "$(changes)" -gt "$1" ] && break; sleep 0.1; done
  log "default interface changed $(( $(now) - started )) ms after: $2"
  previous=$1
  check "default interface change reported ($2)" '[ "$(changes)" -gt "$previous" ]'
}

# 1. Network A.
r=$(q); log "network A: $r"
check "network A answered by A" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.1" ]'

# 2. Switch the default route to network B (another interface): the first
# query after the switch must already be answered by B.
k=$(changes)
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change $k "default route to cb"
a_before=$(count dns-a)
r=$(q); log "network B: $r"
check "first query after the change answered by B" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.2" ]'
check "no query reached A after the change" '[ "$(count dns-a)" = "$a_before" ]'

# 3. Another network on the same interface (a Wi-Fi switch on en0): new
# address on cb, new resolver in the file.
echo '{"ca":["10.201.0.1"],"cb":["10.202.0.53"]}' > $TESTFILE
k=$(changes)
ip -n ldns-c addr add 10.202.0.3/24 dev cb; ip -n ldns-c addr del 10.202.0.2/24 dev cb
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change $k "new address and resolver on cb"
b_before=$(count dns-b)
r=$(q); log "same interface, new network: $r"
check "same interface, new network answered by the new resolver" '[ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.3" ]'
check "no query reached the old resolver after the change" '[ "$(count dns-b)" = "$b_before" ]'

# 4. DHCP has not handed out DNS yet: queries fail fast (no 3 s timeout),
# then the servers appear without another interface change and are used
# within RetryInterval.
echo '{"ca":["10.201.0.1"],"cb":[]}' > $TESTFILE
k=$(changes)
ip -n ldns-c addr add 10.202.0.4/24 dev cb; ip -n ldns-c addr del 10.202.0.3/24 dev cb
ip -n ldns-c route replace default via 10.202.0.1 dev cb
await_change $k "cb without resolvers"
for i in 1 2 3; do
  r=$(q); log "no resolvers: $r"
  check "no resolvers: query $i gets SERVFAIL" '[ "$(echo $r | cut -d" " -f1,2)" = "fail SERVFAIL" ]'
  check "no resolvers: query $i fails within 500 ms" '[ "$(echo $r | cut -d" " -f3)" -lt 500 ]'
done
echo '{"ca":["10.201.0.1"],"cb":["10.202.0.1"]}' > $TESTFILE
APPEAR=$(now); log "resolver appears in the file (no interface change)"
recovered=""
for i in $(seq 30); do
  r=$(q)
  if [ "$(echo $r | cut -d" " -f1,2)" = "ok 192.0.2.2" ]; then recovered=$(( $(now) - APPEAR )); break; fi
  sleep 0.1
done
log "recovered after ${recovered:-never} ms"
check "servers that appear are used within 1.5 s" '[ -n "$recovered" ] && [ "$recovered" -lt 1500 ]'

check "the 127.0.0.1 trap was never asked" '[ "$(count dns-trap)" = 0 ]'
sleep 0.5; kill $(cat $R/tcpdump.pid) 2>/dev/null || true; sleep 0.3
check "capture on the TUN saw the queries to its own resolver" '[ "$(grep -c "> 10\.60\.159\.90\.53:" "$OUT/tun-dns.txt")" -gt 0 ]'
check "no query to a physical resolver entered the TUN" '! grep -Eq "> 10\.20[12]\.0\.(1|53)\.53:" "$OUT/tun-dns.txt"'
check "every resolver saw only physical source addresses" '! cat "$OUT/dns-a.log" "$OUT/dns-b.log" "$OUT/dns-b2.log" | grep -v " from 10\.20[12]\.0\.[0-9]*:" | grep -q .'
grep 'msg="local dns servers"' "$OUT/core.log" | tee -a "$OUT/steps.log" || true
check "a local dns servers line per change" '[ "$(grep -c "msg=\"local dns servers\"" "$OUT/core.log")" -ge 4 ]'
log "result: $([ $FAILED = 0 ] && echo PASS || echo FAIL)"
exit $FAILED
