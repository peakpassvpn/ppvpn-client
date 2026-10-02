#!/bin/sh
# B2 direct case (ppvpn-core #48 on Sail): the host's IPv6 stack is on but it
# has no IPv6 path (lab-client: no global v6 address, no v6 default route).
# The TUN routes IPv6 anyway (no leak), so applications use the AAAA of a
# dual-stack name and a direct dial to that address fails at once. With
#   {"inbound":["tun"],"ip_cidr":["2000::/3"],"action":"route-options","override_destination":"proxy_and_direct"}
# and direct's domain_resolver ipv4_only, the connection must go out over
# IPv4 to the name instead: exit 198.51.100.100 (direct, IPv4).
# dual.lab.test: A 198.51.100.50 + AAAA 2001:db8::50 (lab-dns-node, .54).
#   docker exec lab-client sh /lab/repro/b2-direct-ipv6.sh /work/<sail> with|without
BIN=$1; VARIANT=${2:-with}; ENGINE=sail; . /lab/repro/common.sh
C=/tmp/b2d.json; L=$OUT/b2-direct-$VARIANT.log
RULE=''; [ $VARIANT = with ] && RULE='{"inbound":["tun"],"ip_cidr":["2000::/3"],"action":"route-options","override_destination":"proxy_and_direct"},'
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"udp","tag":"local","server":"198.51.100.54"}],"final":"local","reverse_mapping":true},
 "inbounds":[{"type":"tun","tag":"tun","address":["172.19.0.1/30","fdfe:dcba:9876::1/126"],"auto_route":true,"strict_route":true,
   "route_address":["198.51.100.50/32","2000::/3"],"stack":"system"}],
 "outbounds":[{"type":"direct","tag":"direct","domain_resolver":{"server":"local","strategy":"ipv4_only"}}],
 "route":{"rules":[{"action":"sniff"},{"protocol":"dns","action":"hijack-dns"},$RULE
   {"ip_cidr":["10.0.0.0/8"],"outbound":"direct"}],
  "final":"direct","auto_detect_interface":true},
 "api":$(api_json 7911)}
J
cp $C $OUT/b2-direct-config-$VARIANT.json
{
echo "engine=sail $($BIN -V) variant=$VARIANT"
if ! $BIN -c $C -T > /tmp/b2d.T 2>&1; then echo "config refused: $(head -1 /tmp/b2d.T)"; exit 0; fi
run_engine $C $L.full
cat /etc/resolv.conf > /tmp/resolv.b2d; echo "nameserver 198.51.100.50" > /etc/resolv.conf
echo "host: global v6 addresses $(grep -c '^2' /proc/net/if_inet6), v6 default routes $(grep '^0\{32\} 00 ' /proc/net/ipv6_route | grep -vc ' lo$')"
# +nocookie / --resolve: independent of Sail's cached-cookie bug (dns-cookie-replay.sh).
echo "AAAA dual.lab.test (hijacked, fills the reverse map): $(dig +short +nocookie AAAA dual.lab.test)  A: $(dig +short +nocookie A dual.lab.test)"
t() { s=$(date +%s.%N); o=$(curl -s -m 6 "$@" 2>&1); r=$?; e=$(date +%s.%N); printf "  %-45s rc=%s %.2fs %s\n" "$*" $r $(echo "$e - $s" | bc) "$o"; }
V6='dual.lab.test:80:[2001:db8::50]'; V6S='dual.lab.test:443:[2001:db8::50]'
echo "TCP to [2001:db8::50], HTTP Host (sniff)   expect exit=198.51.100.100:"; t --resolve $V6 http://dual.lab.test/
echo "TCP to [2001:db8::50], TLS SNI (sniff)     expect exit=198.51.100.100:"; t --resolve $V6S https://dual.lab.test/
noname() { printf 'GET / HTTP/1.0\r\n\r\n' | nc -w 5 2001:db8::50 80 | tail -1; }
echo "TCP to [2001:db8::50], no name (map)       expect exit=198.51.100.100: $(noname)"
echo "UDP to [2001:db8::50]:9999, name from the reverse map  expect a reply from .100: $(echo ping | nc -u -w 3 2001:db8::50 9999)"
echo "after an in-place reload, UDP again without a new query: reload=$(api_curl -s -o /dev/null -w %{http_code} -X POST 127.0.0.1:7911/api/v1/runtime/reload) reply: $(echo ping2 | nc -u -w 3 2001:db8::50 9999)"
cat /tmp/resolv.b2d > /etc/resolv.conf
stop_engine
} | tee $L
echo "--- engine log:" >> $L
grep -iE 'dial .* as |override|picked route|2001:db8::50|reverse' $L.full | grep -v 'created span' | cut -c1-230 | tail -14 >> $L
