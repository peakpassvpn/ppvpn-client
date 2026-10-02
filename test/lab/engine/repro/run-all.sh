#!/bin/sh
# Runs every repro for one Sail build (and sing-box 1.13.12 where the same
# config applies), plus the core-level comparisons t3 (ingress failover) and
# t4 (TUN + DNS) on the core engines in CORE_ENGINES (default "sing": the Go
# core with sing-box; "sail" needs a core with a Sail engine, see README).
# Host side, normally through lab.sh:
#   lab.sh up <sail-for-nodes> <core-binary>        # once
#   repro/run-all.sh <name> </work/path-to-sail>    # e.g. 0.16.0 /work/sail-0.16.0
# Logs: $LAB_WORK/repro-logs/<name>/ (repros in the client, core-level here).
NAME=$1; SAIL=$2; HERE=$(cd "$(dirname "$0")/.." && pwd)
. "$HERE/lab.env"
LOGS=$LAB_WORK/repro-logs/$NAME; mkdir -p "$LOGS"
E="docker exec -e OUT=/work/repro-logs/$NAME $LAB-client timeout 300 sh"
docker exec $LAB-client mkdir -p /work/repro-logs/$NAME
docker exec $LAB-web iptables -C INPUT -p udp --dport 53 -j DROP 2>/dev/null || docker exec $LAB-web iptables -I INPUT -p udp --dport 53 -j DROP
for s in b1-reverse-mapping b2-override-timing item10-host-rewrite item15-ruleset; do
  $E /lab/repro/$s.sh sail $SAIL; $E /lab/repro/$s.sh sing-box /work/sing-box
done
$E /lab/repro/b3-dns-fallback.sh $SAIL
# Sail >= 0.16: B3 as the sequential server, B5 API auth, B6 (DNS cookie,
# dig and c-ares) on both engines where the config applies.
$E /lab/repro/b3-sequential.sh $SAIL
$E /lab/repro/b5-api-auth.sh $SAIL
$E /lab/repro/dns-cookie-replay.sh sail $SAIL; $E /lab/repro/dns-cookie-replay.sh sing-box /work/sing-box
$E /lab/repro/b4-fallback-group.sh $SAIL
$E /lab/repro/item09-inbound-no-port.sh $SAIL
for v in untagged tagged untagged-known; do docker exec $LAB-client timeout 120 sh /lab/repro/item16-reload-untagged.sh $SAIL $v; done > /dev/null
docker exec $LAB-web iptables -D INPUT -p udp --dport 53 -j DROP
# core-level, per engine (a sail engine runs $SAIL)
for engine in ${CORE_ENGINES:-sing}; do
  SAIL_BIN=$SAIL "$HERE/t3-host.sh" $engine > "$LOGS/t3-$engine.txt" 2>&1
  SAIL_BIN=$SAIL "$HERE/t4-host.sh" $engine > "$LOGS/t4-$engine.txt" 2>&1
done
echo "done: $NAME"
