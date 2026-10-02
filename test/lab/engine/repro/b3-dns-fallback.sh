#!/bin/sh
# B3: an ordered DNS fallback with an overall budget. Wanted: ask the first
# server; only when it fails (error or no answer within a per-try timeout)
# ask the next, in order; answer SERVFAIL within the budget (core: 8 s) when
# all fail. Sail's race (the only multi-server server) asks every member at
# once for every query. This shows that, with a packet capture.
# Setup (host): docker exec lab-web iptables -I INPUT -p udp --dport 53 -j DROP
# so the second server exists (packets leave) but never answers.
#   docker exec lab-client sh /lab/repro/b3-dns-fallback.sh /work/sail-rel
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
C=/tmp/b3.json; L=$OUT/b3-sail.log
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"udp","tag":"first","server":"198.51.100.54"},{"type":"udp","tag":"second","server":"198.51.100.50"},
   {"type":"race","tag":"remote","servers":["first","second"]}],"final":"remote","timeout":"8s"},
 "inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7960},{"type":"direct","tag":"dns-in","listen":"127.0.0.1","listen_port":5353,"network":"udp"}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"dns-in","action":"hijack-dns"}],"final":"direct"}}
J
cp $C $OUT/b3-config.json
run_engine $C $L.full
tcpdump -n -l -i eth0 'udp dst port 53 and (dst host 198.51.100.54 or dst host 198.51.100.50)' > /tmp/b3.pcap.txt 2>/dev/null &
TP=$!; sleep 1
{
echo "engine=sail $($BIN -V)"
echo "first (198.51.100.54) answers; second (198.51.100.50) drops every query."
for n in www video split; do echo "query $n.lab.test -> $(dig +short +nocookie +time=10 +tries=1 -p 5353 @127.0.0.1 $n.lab.test | head -1)"; sleep 0.3; done
} | tee $L
sleep 1; kill $TP
echo "--- packets sent per query (wanted: only to .54 while it answers):" >> $L
grep -E "> 198.51.100.(54|50).53" /tmp/b3.pcap.txt | awk '{print $1, $3, $4, $5}' >> $L
stop_engine
cat $L | tail -8
