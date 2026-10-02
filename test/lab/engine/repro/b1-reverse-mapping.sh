#!/bin/sh
# B1: dns.reverse_mapping and hijack-dns. A query hijacked from the TUN is
# answered (b1.lab.test -> 198.51.100.50); then connections to that address
# carry no name of their own (HTTP/1.0 without Host, and UDP). With reverse
# mapping the route rule on b1.lab.test should match: exit 198.51.100.12
# (the node). Without it they go final: direct, exit 198.51.100.100.
#   docker exec lab-client sh /lab/repro/b1-reverse-mapping.sh sail /work/sail-rel
#   docker exec lab-client sh /lab/repro/b1-reverse-mapping.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
C=/tmp/b1.json; L=$OUT/b1-$ENGINE.log
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"udp","tag":"remote","server":"198.51.100.54"}],"final":"remote","reverse_mapping":true},
 "inbounds":[{"type":"tun","tag":"tun","address":["172.19.0.1/30"],"auto_route":true,"strict_route":true,"route_address":["198.51.100.50/32"],"stack":"system"}],
 "outbounds":[$(ss_node node),{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"action":"sniff"},{"protocol":"dns","action":"hijack-dns"},
   {"domain_suffix":["b1.lab.test"],"outbound":"node"}],
  "final":"direct","auto_detect_interface":true}}
J
redact $C > $OUT/b1-config.json
run_engine $C $L.full
{
echo "engine=$ENGINE $($BIN version 2>/dev/null | head -1)$($BIN -V 2>/dev/null)"
echo "1 hijacked query (to 198.51.100.50:53, no DNS server there) b1.lab.test -> $(dig +short +time=3 +tries=1 @198.51.100.50 b1.lab.test | tr '\n' ' ')"
echo "2 TCP to 198.51.100.50:80, HTTP/1.0 without Host -> exit $(printf 'GET / HTTP/1.0\r\n\r\n' | nc -w 5 198.51.100.50 80 | tail -1 | exit_of)   (expect .12)"
echo "3 UDP to 198.51.100.50:9999 -> exit $(echo ping | nc -u -w 3 198.51.100.50 9999 | exit_of)   (expect .12)"
echo "4 control: TCP with Host b1.lab.test -> exit $(curl -s -m 5 --resolve b1.lab.test:80:198.51.100.50 http://b1.lab.test/ | exit_of)   (sniffed, expect .12)"
} | tee $L
stop_engine
echo "--- route decisions in the engine log:" >> $L
grep -iE 'reverse|198.51.100.50|match|picked route|outbound/|rule ' $L.full | grep -viE 'created span|dialing|connected in' | cut -c1-230 | tail -20 >> $L
