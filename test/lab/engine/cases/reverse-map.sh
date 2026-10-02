#!/bin/sh
# reverse-map: what t4 4.2 leaves out — a name learned from the core's DNS
# answer (split.lab.test -> .60 over DoT; only a node, which resolves it to
# .50, can reach it) still routes the Host-less connection to its address
# after the kernel is switched (new revision) and after another node is
# selected. ids reverse-map.<n>.
ENGINE=${1:-sing}; . /lab/cases/lib.sh
start_core $ENGINE --tun=true --local-proxy=true >/dev/null && apply >/dev/null && api start >/dev/null
sleep 1
# hostless: HTTP/1.0 without Host to .60, the address the core answered.
hostless() { printf 'GET / HTTP/1.0\r\n\r\n' | nc -w 6 198.51.100.60 80 | tail -1 | exit_of; }
dig +short +tries=1 +time=8 @198.51.100.99 split.lab.test A >/dev/null

check reverse-map.1 "Host-less connection to an answered address goes to the node by name" "$(hostless)" '198\.51\.100\.1[12]'
# A new revision with a changed rule: a kernel switch, not a restart.
jq '.revision = "reverse-map-2" | .routing.rules += [{"id":"switch","match":{"domain_suffixes":["switch.lab.test"]},"action":{"type":"direct"}}]' \
  /work/profile.json > /run/reverse-map-2.json
PROFILE=/run/reverse-map-2.json apply >/dev/null; sleep 1
check reverse-map.2 "the apply switched kernels" "$(grep -c 'msg="kernel switched"' $R/core.log)" '[1-9][0-9]*'
check reverse-map.3 "after the kernel switch the name is still known" "$(hostless)" '198\.51\.100\.1[12]'
api select-node '{"node_id":"us"}' >/dev/null
check reverse-map.4 "after selecting another node it goes there, by name" "$(hostless)" '198\.51\.100\.13'
stop_core; summary
