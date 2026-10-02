#!/bin/sh
# Runs t3.sh in $LAB-client and blocks/unblocks $LAB-a's port 443 on cue.
. "$(cd "$(dirname "$0")" && pwd)/lab.env"
ENGINE=${1:-sail}
docker exec $LAB-a iptables -F INPUT
docker exec $LAB-client rm -f /run/core/ready /run/core/blocked /run/core/unblock /run/core/unblocked
docker exec -e SAIL_BIN="${SAIL_BIN:-}" -e SAIL_FAILOVER="${SAIL_FAILOVER:-}" -e CHECK_INTERVAL="${CHECK_INTERVAL:-}" $LAB-client sh /lab/t3.sh $ENGINE &
until docker exec $LAB-client test -f /run/core/ready 2>/dev/null; do sleep 0.3; done
docker exec $LAB-a iptables -A INPUT -p tcp --dport 443 -j DROP; docker exec $LAB-client touch /run/core/blocked
until docker exec $LAB-client test -f /run/core/unblock 2>/dev/null; do sleep 0.3; done
docker exec $LAB-a iptables -F INPUT; docker exec $LAB-client touch /run/core/unblocked
wait
