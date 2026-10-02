#!/bin/sh
# B3 on Sail >= 0.16: the `sequential` DNS server (a sail extension) as the
# core's ordered fallback: ask the first server; only when it gives no
# answer within attempt_timeout ask the next; SERVFAIL once the budget is
# spent; prefer_for keeps asking first the server that answered last.
# .54 answers, .50 drops every query (host: iptables DROP on lab-web, as
# run-all sets it up).
#   docker exec lab-client sh /lab/repro/b3-sequential.sh /work/<sail>
# Wanted:
#   A [.54, .50]: answered by .54, nothing sent to .50.
#   B [.50, .54]: the first query ~3 s (attempt_timeout on .50, then .54);
#     the next ones fast and only to .54 (prefer_for).
#   C [.50, .50b]: SERVFAIL within the 8 s budget, not dns.timeout.
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
L=$OUT/b3-sequential.log
conf() { # tag order, file
  cat > $2 <<J
{"log":{"level":"debug","timestamp":true},
 "dns":{"servers":[{"type":"udp","tag":"a","server":"198.51.100.54"},{"type":"udp","tag":"drop","server":"198.51.100.50"},
   {"type":"udp","tag":"drop2","server":"198.51.100.50","server_port":5300},
   {"type":"sequential","tag":"remote","servers":[$1],"attempt_timeout":"3s","budget":"8s","prefer_for":"10m"}],
  "final":"remote","timeout":"10s"},
 "inbounds":[{"type":"direct","tag":"dns-in","listen":"127.0.0.1","listen_port":5353,"network":"udp"}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "route":{"rules":[{"inbound":"dns-in","action":"hijack-dns"}],"final":"direct"}}
J
}
q() { s=$(date +%s.%N); a=$(dig +nocookie +time=12 +tries=1 -p 5353 @127.0.0.1 $1 2>&1 | grep -E 'status:|^[a-z0-9.]+\s.*IN\s+A' | sed -E 's/.*status: ([A-Z]+).*/\1/' | tr '\n' ' '); e=$(date +%s.%N); echo "$1 -> $a $(echo "$e - $s" | bc | cut -c1-4)s"; }
case_run() { # name, servers, queries...
  name=$1; servers=$2; shift 2
  conf "$servers" /tmp/b3s-$name.json; cp /tmp/b3s-$name.json $OUT/b3-sequential-config-$name.json
  run_engine /tmp/b3s-$name.json $OUT/b3-sequential-$name.log.full
  tcpdump -n -l -i eth0 'udp dst host 198.51.100.54 or udp dst host 198.51.100.50' > /tmp/b3s-$name.pcap 2>/dev/null & TP=$!; sleep 1
  echo "## $name servers=[$servers]"
  for n in "$@"; do q $n; done
  sleep 1; kill $TP
  echo "   packets to .54: $(grep -c '> 198.51.100.54.53:' /tmp/b3s-$name.pcap)  to .50: $(grep -cE '> 198.51.100.50.(53|5300):' /tmp/b3s-$name.pcap)"
  stop_engine
}
{
echo "engine=sail $($BIN -V)"
$BIN -c /dev/null -T >/dev/null 2>&1; conf '"a","drop"' /tmp/b3s-check.json; $BIN -c /tmp/b3s-check.json -T 2>&1 | head -3 | sed 's/^/check: /'
case_run A '"a","drop"' www.lab.test video.lab.test split.lab.test
case_run B '"drop","a"' www.lab.test video.lab.test split.lab.test
case_run C '"drop","drop2"' www.lab.test
} | tee $L
