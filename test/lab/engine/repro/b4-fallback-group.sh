#!/bin/sh
# B4: the fallback group as ingress failover. Members: a (VLESS/REALITY,
# 198.51.100.11, exit .11) then b (SS2022, 198.51.100.12, exit .12); check URL
# http://198.51.100.50/generate_204, interval 15s, timeout 5s. The client
# silently drops its packets to a (iptables OUTPUT DROP), then lifts that.
# Shows: how long each new connection waits while a is down, when the group
# switches, what the Clash API says about a, and when it switches back.
#   docker exec lab-client sh /lab/repro/b4-fallback-group.sh /work/sail-rel
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
C=/tmp/b4.json; L=$OUT/b4-sail.log; SEC=0123456789abcdef0123456789abcdef
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7950}],
 "outbounds":[{"type":"fallback","tag":"node","outbounds":["a","b"],"url":"http://198.51.100.50/generate_204","interval":"15s","timeout":"5s"},
   $(vless_node a),$(ss_node b),{"type":"direct","tag":"direct"}],
 "route":{"final":"node"},
 "clash_api":{"external_controller":"127.0.0.1:7951","secret":"$SEC"}}
J
redact $C > $OUT/b4-config.json
iptables -D OUTPUT -d 198.51.100.11 -p tcp --dport 443 -j DROP 2>/dev/null
run_engine $C $L.full
req() { s=$(date +%s.%N); o=$(curl -s -m 12 -x http://127.0.0.1:7950 http://198.51.100.50/ | exit_of); e=$(date +%s.%N); echo "$(date +%T) exit=${o:-FAIL} $(echo "$e - $s" | bc | cut -c1-4)s"; }
member() { curl -s -H "Authorization: Bearer $SEC" http://127.0.0.1:7951/proxies/$1 | jq -c '{name,now,alive,history}'; }
{
echo "engine=sail $($BIN -V)"
echo "baseline: $(req)  group: $(member node)"
iptables -I OUTPUT -d 198.51.100.11 -p tcp --dport 443 -j DROP; echo "$(date +%T) a blocked (DROP)"
for i in $(seq 25); do echo "  $(req)"; sleep 1; done
echo "group: $(member node)"; echo "member a: $(member a)"
iptables -D OUTPUT -d 198.51.100.11 -p tcp --dport 443 -j DROP; echo "$(date +%T) a unblocked"
for i in $(seq 8); do r=$(req); echo "  $r"; case "$r" in *28.11*) break;; esac; sleep 3; done
echo "group: $(member node)"
} | tee $L
stop_engine
echo "--- group log:" >> $L
grep -E 'group::|fallback' $L.full | grep -v 'created span' | cut -c1-220 | tail -40 >> $L
