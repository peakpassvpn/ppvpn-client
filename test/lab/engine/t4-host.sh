#!/bin/sh
# Checklist 4: TUN + DNS, driven from the host. Usage: t4-host.sh sail|sing
. "$(cd "$(dirname "$0")" && pwd)/lab.env"
ENGINE=${1:-sail}
c() { docker exec -e SAIL_BIN="${SAIL_BIN:-}" $LAB-client sh -c ". /lab/core.sh; $1"; }
dnslog() { docker logs --since "$1" $LAB-dns 2>&1 | grep -E 'IN (A|AAAA)' | sed -E 's/^\[INFO\] //' | cut -c1-110 | sed 's/^/      dot-server: /'; }
# q <name>: one hijacked query (to an arbitrary address, port 53, through the TUN); prints rcode, answer, time.
Q='q() { s=$(date +%s.%N); o=$(dig +tries=1 +time=${2:-15} @198.51.100.99 $1 A 2>&1); e=$(date +%s.%N); printf "%s -> %s [%s] %.1fs\n" "$1" "$(echo "$o" | sed -n "s/.*status: \([A-Z]*\).*/\1/p;/timed out/s/.*/TIMEOUT/p" | head -1)" "$(echo "$o" | awk "/^[a-z].*IN[[:space:]]+A[[:space:]]/{print \$5}" | tr "\n" " ")" "$(echo "$e - $s" | bc)"; };'
docker exec $LAB-dns sh -c 'iptables -F INPUT' ; for n in $LAB-a $LAB-b $LAB-c; do docker exec $n iptables -F INPUT; done
c "start_core $ENGINE --tun=true --local-proxy=true && apply >/dev/null && api start | jq -c .ok"
sleep 1
echo "== 4.1 hijack-dns: a query to 198.51.100.99:53 (nothing there) is answered by dns-remote: DoT, through the selected node"
t=$(date -u +%Y-%m-%dT%H:%M:%SZ); c "$Q q www.lab.test; q split.lab.test"; sleep 0.5; dnslog $t
echo "   tcp/53 too: $(c 'dig +tcp +short +tries=1 +time=5 @198.51.100.99 video.lab.test')"
echo "== 4.2 real IP (no fake-ip): split.lab.test is .60 over DoT (no such host); only a node, which resolves it to .50, can reach it"
c 'echo "   tls sni:    $(curl -s -m 8 --resolve split.lab.test:443:198.51.100.60 https://split.lab.test/)"
   echo "   http host:  $(curl -s -m 8 --resolve split.lab.test:80:198.51.100.60 http://split.lab.test/)"
   echo "   reverse mapping (HTTP/1.0 without Host to .60, the address just answered): $(printf "GET / HTTP/1.0\r\n\r\n" | nc -w 6 198.51.100.60 80 | tail -1)"
   echo "   never resolved address .61 (expect failure): $(printf "GET / HTTP/1.0\r\n\r\n" | nc -w 4 198.51.100.61 80 | tail -1)"
   echo "   plain IP target .50: $(curl -s -m 8 http://198.51.100.50/)"
   echo "   rule video.lab.test -> us (.13): $(curl -s -m 8 --resolve x.video.lab.test:443:198.51.100.50 https://x.video.lab.test/)"
   echo "   udp through the node: $(echo ping | nc -u -w 3 198.51.100.50 9999)"
   echo "   private destination with a sniffed name (rule: direct; sing-box connects to the address itself): $(curl -s -m 8 --resolve split.lab.test:80:10.77.0.50 http://split.lab.test/)"
   echo "   198.18.0.9 without a domain (expect rejected): $(curl -s -m 4 -o /dev/null -w "%{http_code} %{errormsg}" http://198.18.0.9/)"'
P=r$(date +%s)
halfopen() { # silently drop the flows now established to the DoT server, and only those
  docker exec $LAB-dns sh -c 'ss -Htn state established "( sport = :853 )" | awk "{print \$4}" | sed "s/\\[::ffff://;s/\\]//" | while read peer; do iptables -I INPUT -p tcp -s ${peer%:*} --sport ${peer##*:} --dport 853 -j DROP; echo "      dropping established flow from $peer"; done'; }
echo "== 4.3 peer closes the pooled DoT connection (CoreDNS restarted), next queries"
c "$Q q $P-a.lab.test"; docker restart -t 1 $LAB-dns >/dev/null; docker exec $LAB-dns ip route add default dev eth0 2>/dev/null; sleep 1.5
c "$Q q $P-b.lab.test; q $P-c.lab.test"
echo "== 4.4 half-open: the established DoT flows are silently dropped (new connections pass), next queries"
c "$Q q $P-d.lab.test"; halfopen
c "$Q q $P-e.lab.test 20; q $P-f.lab.test 20"; docker exec $LAB-dns iptables -F INPUT
echo "== 4.5 all three upstreams fail (DoT server drops everything): rcode and time (core: SERVFAIL within 8 s)"
docker exec $LAB-dns iptables -I INPUT -p tcp --dport 853 -j DROP
c "$Q q $P-g.lab.test 20; q $P-h.lab.test 20"
docker exec $LAB-dns iptables -F INPUT
echo "   recovered: $(c "$Q q $P-i.lab.test 20")  $(c "$Q q $P-j.lab.test 20")"
echo "== 4.6 only 1.1.1.1 fails (dropped on the nodes; 8.8.8.8 and 9.9.9.9 still answer)"
for n in $LAB-a $LAB-b $LAB-c; do docker exec $n sh -c 'iptables -t nat -D OUTPUT -p tcp -d 1.1.1.1 --dport 853 -j DNAT --to-destination 198.51.100.53:853; iptables -A OUTPUT -p tcp -d 1.1.1.1 --dport 853 -j DROP'; done
docker restart -t 1 $LAB-dns >/dev/null; docker exec $LAB-dns ip route add default dev eth0 2>/dev/null; sleep 1.5
t=$(date -u +%Y-%m-%dT%H:%M:%SZ); c "$Q q $P-k.lab.test 20; q $P-l.lab.test 20; q $P-m.lab.test 20"
for n in $LAB-a $LAB-b $LAB-c; do docker exec $n sh -c 'iptables -F OUTPUT; iptables -t nat -I OUTPUT -p tcp -d 1.1.1.1 --dport 853 -j DNAT --to-destination 198.51.100.53:853'; done
echo "== log lines (dns / connection / failures)"
c 'grep -E "msg=dns|msg=connection|outbound failed" $R/core.log | cut -c1-330 | tail -8; echo; grep -iE "dns.*(fail|error|timeout|servfail)|WARN|ERROR" $R/core.log | cut -c1-330 | tail -12; stop_core'
