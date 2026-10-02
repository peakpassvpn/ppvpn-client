#!/bin/sh
# B7 as the Rust ppvpn-core will use it: plain DNS servers (udp, tcp, tls) with no
# detour, under a TUN with auto_route, must leave through the default
# interface (never into the TUN), and keep doing so after the default
# interface changes and the engine swaps the server list (a reload, as the
# engine re-reads the new network's resolvers: core #61/#69); a connection
# held across that reload must survive it.
# Runs inside lab-client, after b7-run.sh (host) connected it to the second
# network core-sailnet2 (203.0.113.0/24, TEST-NET-3: its DNS .54 answers
# split.lab.test with 203.0.113.60; the first network's .54 with .50, and
# its DoT .53 with .60).
#   docker exec lab-client sh /lab/repro/b7-dns-bind.sh sail /work/<sail>
#   docker exec lab-client sh /lab/repro/b7-dns-bind.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
TAG=$ENGINE-$(basename $BIN); L=$OUT/b7-dns-bind-$TAG.log
conf() { # server JSON
  cat > /tmp/b7.json <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[$1],"final":"d","disable_cache":true},
 "inbounds":[{"type":"tun","tag":"tun","address":["172.19.0.1/30"],"auto_route":true,"strict_route":true,"stack":"system"},
   {"type":"direct","tag":"dns-in","listen":"127.0.0.1","listen_port":5355,"network":"udp"}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"dns-in","action":"hijack-dns"},{"inbound":"tun","action":"sniff"},{"protocol":"dns","action":"hijack-dns"}],
  "final":"direct","auto_detect_interface":true}$API}
J
}
# The runtime API (for the reload in E5) is Sail's; sing-box has none and is
# restarted instead.
API=''; [ "$ENGINE" = sail ] && API=",\"api\":$(api_json 7912)"
# The TUN: the interface holding 172.19.0.1, plain (/30) or point-to-point
# ("172.19.0.1 peer ..."), as Sail configures its utun.
tunif() { ip -o -4 addr show | awk '/inet 172\.19\.0\.1[ \/]/ {print $2; exit}'; }
q() { dig +nocookie +tries=1 +time=6 -p 5355 @127.0.0.1 split.lab.test A +short 2>&1 | head -1; }
# One capture per interface (the TUN and every ethN), so a count says where
# a packet went: tcpdump -i any does not name the interface on every build.
cap_start() { rm -f /tmp/b7cap-*.txt; for i in $(tunif) $(ls /sys/class/net | grep '^eth'); do
  tcpdump -n -l -i $i "port 53 or port 853" > /tmp/b7cap-$i.txt 2>/dev/null & done; sleep 1; }
cap_stop() { sleep 1; pkill -x tcpdump 2>/dev/null; sleep 0.3; }
count() { # interface, address.port: packets sent to it there; "none" when
  # the interface was not captured (an assertion on it would prove nothing).
  [ -n "$1" ] && [ -f /tmp/b7cap-$1.txt ] || { echo none; return; }
  n=$(grep -c "> $2:" /tmp/b7cap-$1.txt 2>/dev/null); echo "${n:-0}"; }
case_d() { # name, server JSON, server address
  conf "$2"; run_engine /tmp/b7.json $OUT/b7-$TAG-$1.log.full; T=$(tunif)
  cap_start; r=$(q); cap_stop
  echo "D $1: answer=${r:-FAILED} tun=${T:-none} on-tun=$(count "$T" "$3") on-eth0=$(count eth0 "$3")   (want an answer, on-tun 0, on-eth0 > 0)"
  stop_engine
}
{
echo "engine=$ENGINE $($BIN -V 2>/dev/null || $BIN version 2>/dev/null | head -1)"
ip route replace default dev eth0
case_d udp '{"type":"udp","tag":"d","server":"198.51.100.54"}' '198.51.100.54.53'
case_d tcp '{"type":"tcp","tag":"d","server":"198.51.100.54"}' '198.51.100.54.53'
case_d tls '{"type":"tls","tag":"d","server":"198.51.100.53","tls":{"enabled":true,"server_name":"one.one.one.one"}}' '198.51.100.53.853'
# `local` (the system's servers, resolv.conf -> .54): the original B7 case,
# and the capture's positive control while it still loops (Sail 0.16.0:
# on-tun > 0).
case_d local '{"type":"local","tag":"d"}' '198.51.100.54.53'
# E5: switch the default interface while the TUN runs, then swap the server
# (the engine would re-read the new network's resolver).
conf '{"type":"udp","tag":"d","server":"198.51.100.54"}'; run_engine /tmp/b7.json $OUT/b7-$TAG-e5.log.full; T=$(tunif)
echo "E5 before: answer=$(q) (net A: 198.51.100.50)"
# A connection held across the reload (Sail's contract: a reload that only
# changes dns.servers keeps the TUN and routed connections): 6 s of drip
# through the TUN, started before the switch.
( curl -s -m 15 -o /dev/null -w 'http=%{http_code} bytes=%{size_download} err=%{errormsg}' http://198.51.100.50/drip?6 > /tmp/b7-held.txt 2>&1 & )
sleep 1
ip route replace default dev eth1
conf '{"type":"udp","tag":"d","server":"203.0.113.54"}'
code=$(api_curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:7912/api/v1/runtime/reload 2>/dev/null)
[ "$ENGINE" = sing-box ] && { stop_engine; run_engine /tmp/b7.json $OUT/b7-$TAG-e5b.log.full; T=$(tunif); code=restart; }
sleep 2; cap_start; r=$(q); cap_stop
echo "E5 after switch to eth1 + server 203.0.113.54 (reload $code): answer=${r:-FAILED} on-tun=$(count "$T" 203.0.113.54.53) on-eth1=$(count eth1 203.0.113.54.53) on-eth0=$(count eth0 203.0.113.54.53)   (want 203.0.113.60, on-tun 0, on-eth1 > 0)"
for i in $(seq 20); do [ -s /tmp/b7-held.txt ] && break; sleep 0.5; done
held=$(cat /tmp/b7-held.txt 2>/dev/null); [ "$ENGINE" = sing-box ] && held="n/a (sing-box restarted: $held)"
echo "E5 held connection across the reload: ${held:-none}   (want http=200, bytes >= 55000, no error)"
stop_engine
ip route replace default dev eth0
} | tee $L
