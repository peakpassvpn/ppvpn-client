#!/bin/sh
# Checklist 3: ingress failover, switch back, pin. Run by t3-host.sh, which
# blocks and unblocks the primary ingress (lab-a) from outside.
. /lab/core.sh
ENGINE=${1:-sail}
start_core $ENGINE --tun=false --local-proxy=true || exit 1
apply >/dev/null; api start >/dev/null
JP=$(cred '{"node_id":"jp"}'); T=http://198.51.100.50/
( curl -s -N --unix-socket $R/core.sock -H "Authorization: Bearer $(cat $R/secret)" http://core/v1/watch-events > $R/events & )
req() { s=$(date +%s.%N); out=$(curl -s -m 12 -x http://$JP $T | cut -d' ' -f1); e=$(date +%s.%N); printf "%s %s %.1fs\n" "$(date +%T)" "${out:-FAIL}" "$(echo "$e - $s" | bc)"; }
ing() { api get-status | jq -c '.data | {sel: .selected_ingress.endpoint_key, prev: .selected_ingress.previous_endpoint_key, pin: .nodes[0].pinned_endpoint_key, health: [.nodes[0].ingresses[] | "\(.endpoint_key):\(.healthy)/f\(.consecutive_failures)/a\(.active)"]}'; }
echo "# baseline: $(req)  $(ing)"
echo "# pin 9002 (SS2022 backup): $(api pin-ingress '{"node_id":"jp","endpoint_key":"9002"}' | jq -c '.ok,.data')"
echo "#   $(req)  $(ing)"
echo "# pin unknown key: $(api pin-ingress '{"node_id":"jp","endpoint_key":"nope"}' | jq -c .error.code)"
echo "# unpin: $(api pin-ingress '{"node_id":"jp","endpoint_key":null}' | jq -c .ok)"
echo "#   $(req)  $(ing)"
touch $R/ready; while [ ! -f $R/blocked ]; do sleep 0.2; done
echo "# primary blocked (DROP) at $(date +%T); a request every second:"
for i in $(seq 12); do echo "#   $(req)"; sleep 1; done
echo "#   $(ing)"
echo "# pin 9001 while it is blocked (core: no fallback, must fail): $(api pin-ingress '{"node_id":"jp","endpoint_key":"9001"}' | jq -c .ok)  $(req)"
echo "# unpin: $(api pin-ingress '{"node_id":"jp","endpoint_key":null}' | jq -c .ok)  $(req)"
touch $R/unblock; while [ ! -f $R/unblocked ]; do sleep 0.2; done
echo "# primary unblocked at $(date +%T); a request every 5 s until it is back on .11 (max 150 s):"
for i in $(seq 30); do r=$(req); echo "#   $r"; case "$r" in *28.11*) break;; esac; sleep 5; done
echo "#   $(ing)"
echo "# events: $(jq -c '.data | select(.type|test("Ingress|Node")) | {type, node_id, endpoint_key, message}' $R/events | tr '\n' ' ')"
grep -iE 'switch|unhealthy|recovered|tested' $R/core.log | cut -c1-260 | tail -14 | sed 's/^/#   log: /'
stop_core
