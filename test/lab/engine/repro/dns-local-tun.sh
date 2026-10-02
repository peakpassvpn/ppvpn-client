#!/bin/sh
# A direct-dial lookup through the `local` DNS server while a TUN with
# auto_route takes all traffic. With route.auto_detect_interface, sing-box
# binds every socket it makes, its DNS servers' included, to the default
# interface, so the lookup leaves through eth0. Seen in core's t4 on Sail
# 0.16 (and 0.15): once the TUN is up, a `local` lookup that needs the
# network (not answered from /etc/hosts) enters Sail's own TUN, is hijacked
# back to the same server and loops (a query storm) until it times out.
# Sail's auto_detect_interface binds outbounds, not DNS servers' sockets;
# bind_interface on the server avoids it. www/video are in the lab's
# /etc/hosts (answered without a query); split.lab.test is not.
# The lookup is made by Sail itself: a request through the mixed inbound
# to http://www.lab.test/ goes out direct, resolved by route's
# default_domain_resolver (the local server, resolv.conf -> .54).
#   docker exec lab-client sh /lab/repro/dns-local-tun.sh sail /work/<sail> [bind]
#   docker exec lab-client sh /lab/repro/dns-local-tun.sh sing-box /work/sing-box
# With "bind" (Sail only) the local server sets bind_interface eth0.
ENGINE=$1; BIN=$2; VARIANT=${3:-}; RUN=${RUN:-1}; . /lab/repro/common.sh
TAG=$ENGINE-$(basename $BIN)${VARIANT:+-$VARIANT}-$RUN
C=/tmp/dlt.json; L=$OUT/dns-local-tun-$TAG.log
BIND=''; [ "$VARIANT" = bind ] && BIND=',"bind_interface":"eth0"'
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"local","tag":"dns-local"$BIND}],"final":"dns-local"},
 "inbounds":[{"type":"tun","tag":"tun","address":["172.19.0.1/30"],"auto_route":true,"strict_route":true,"stack":"system"},
   {"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7980}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"tun","action":"sniff"},{"protocol":"dns","action":"hijack-dns"}],
  "final":"direct","auto_detect_interface":true,"default_domain_resolver":"dns-local"}}
J
cp $C $OUT/dns-local-tun-config-$TAG.json
echo "nameserver 198.51.100.54" > /etc/resolv.conf
run_engine $C $L.full
{
echo "engine=$ENGINE${VARIANT:+ ($VARIANT)} $($BIN -V 2>/dev/null || $BIN version 2>/dev/null | head -1)"
for n in www video split; do s=$(date +%s.%N); r=$(curl -s -m 12 -x http://127.0.0.1:7980 http://$n.lab.test/); e=$(date +%s.%N)
  echo "direct via mixed, $n.lab.test (resolved by dns-local after the TUN is up): ${r:-FAILED} $(echo "$e - $s" | bc | cut -c1-4)s"; done
} | tee $L
grep -iE 'dns-local|timeout' $L.full | grep -iE 'timeout|failed|exchanged' | head -4 | cut -c1-200 >> $L
stop_engine
