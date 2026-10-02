#!/bin/sh
# B2: sniff override_destination rewrites the destination at the sniff rule,
# so a later ip_cidr rule no longer sees the address. A connection to a
# private address (10.77.0.50) whose HTTP Host is split.lab.test should be
# routed direct by the 10.0.0.0/8 rule. sing-box 1.13 has no
# override_destination (the core hands the name to the node in its own
# outbound, after routing), so its config is the same without that field.
#   docker exec lab-client sh /lab/repro/b2-override-timing.sh sail /work/sail-rel
#   docker exec lab-client sh /lab/repro/b2-override-timing.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
C=/tmp/b2.json; L=$OUT/b2-$ENGINE.log
OVR=',"override_destination":true'; [ $ENGINE = sing-box ] && OVR=''
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7970}],
 "outbounds":[$(ss_node node),{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"in","action":"sniff"$OVR},
   {"ip_cidr":["198.51.100.50/32"],"outbound":"direct"}],
  "final":"node"}}
J
redact $C > $OUT/b2-config-$ENGINE.json
run_engine $C $L.full
{
echo "engine=$ENGINE"
echo "SOCKS5 CONNECT to 198.51.100.50:80 (an address), HTTP Host split.lab.test -> $(curl -s -m 5 --socks5 127.0.0.1:7970 -H 'Host: split.lab.test' http://198.51.100.50/ | exit_of)"
echo "   expected: direct (exit 198.51.100.100, the client); 198.51.100.12 means it went to the node"
} | tee $L
stop_engine
echo "--- engine log:" >> $L
grep -iE 'sniff|override|picked route|match|198.51.100.50|outbound/' $L.full | grep -viE 'created span|connected in' | cut -c1-230 | tail -12 >> $L
