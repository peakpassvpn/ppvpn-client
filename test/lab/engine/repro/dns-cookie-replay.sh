#!/bin/sh
# DNS cookie replay: a hijacked query answered from Sail's cache carries the
# EDNS COOKIE of the query that filled the cache, not the asking client's.
# Clients that check cookies (dig, c-ares: Alpine's curl, Node dns.resolve)
# drop such answers and time out. sing-box echoes each client's cookie.
#   docker exec lab-client sh /lab/repro/dns-cookie-replay.sh sail /work/sail-rel
#   docker exec lab-client sh /lab/repro/dns-cookie-replay.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
C=/tmp/ck.json; L=$OUT/dns-cookie-$ENGINE.log
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"udp","tag":"local","server":"198.51.100.54"}],"final":"local"},
 "inbounds":[{"type":"direct","tag":"dns-in","listen":"127.0.0.1","listen_port":5354,"network":"udp"}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"dns-in","action":"hijack-dns"}],"final":"direct"}}
J
cp $C $OUT/dns-cookie-config.json
run_engine $C $L.full
{
echo "engine=$ENGINE"
for i in 1 2 3; do echo "query $i: $(dig +cookie +tries=1 +time=2 -p 5354 @127.0.0.1 www.lab.test A 2>&1 | grep -E 'COOKIE:|^www' | tr '\n' ' ')"; done
for i in 1 2; do s=$(date +%s.%N); r=$(curl -s -m 6 -o /dev/null -w '%{exitcode} %{errormsg}' --dns-servers 127.0.0.1:5354 http://dual.lab.test/ 2>&1); e=$(date +%s.%N)
echo "c-ares (curl --dns-servers) dual.lab.test try $i: $r $(echo "$e - $s" | bc)s"; done
} | tee $L
stop_engine
