#!/bin/sh
# dns-hijack: what t4 4.1 leaves out — DNS to an IPv6 address and to the
# tunnel's own DNS addresses is hijacked; a direct-routed name goes to
# dns-local, the rest to dns-remote (split.lab.test answers .50 from the
# node-side DNS, .60 over DoT, which tells them apart); DoT to a server is
# not hijacked, only routed. ids dns-hijack.<n>.
ENGINE=${1:-sing}; . /lab/cases/lib.sh
# The lab profile plus one direct rule for direct-split.lab.test.
jq '.revision = "dns-hijack" | .routing.rules += [{"id":"direct-split","match":{"domain_suffixes":["direct-split.lab.test"]},"action":{"type":"direct"}}]' \
  /work/profile.json > /run/dns-hijack-profile.json
export PROFILE=/run/dns-hijack-profile.json
start_core $ENGINE --tun=true --local-proxy=true >/dev/null && apply >/dev/null && api start >/dev/null
sleep 1
up dns-hijack
# server <name>: the core DNS server that answered name (dns log line).
server() { sed -n "s/.*msg=dns name=$1\. type=A server=\([^ ]*\).*/\1/p" $R/core.log | tail -1; }
a() { dig +short +tries=1 +time=8 "$@" | tail -1; }

check dns-hijack.1 "IPv4 query to an arbitrary address is answered over DoT" "$(a @198.51.100.99 split.lab.test A)" '198\.51\.100\.60'
check dns-hijack.2 "IPv6 query to an arbitrary address is hijacked too" "$(a @2001:db8::99 split.lab.test A)" '198\.51\.100\.60'
check dns-hijack.3 "query to the tunnel's own IPv4 DNS address" "$(a @10.60.159.90 split.lab.test A)" '198\.51\.100\.60'
check dns-hijack.4 "query to the tunnel's own IPv6 DNS address" "$(a @fde2:ec40:9312:c7fd::2 split.lab.test A)" '198\.51\.100\.60'
check dns-hijack.5 "a proxied name is resolved by dns-remote" "$(a @198.51.100.99 hj5.split.lab.test A >/dev/null; server hj5.split.lab.test)" 'dns-remote'
check dns-hijack.6 "a direct-routed name is resolved by dns-local" "$(a @198.51.100.99 direct-split.lab.test A >/dev/null; server direct-split.lab.test)" 'dns-local'
# No dns line holds with no core too: the DoT server must also have
# answered (any status: dot7 is not in its hosts), and the core's log must
# have dns lines at all (5, 6).
check dns-hijack.7 "DoT to a server (port 853) is answered and not hijacked: no dns log line" "$(a=$(kdig +tls +timeout=8 @198.51.100.53 dot7.lab.test A 2>/dev/null | sed -n 's/.*status: \([A-Z]*\).*/\1/p' | head -1); s=$(server dot7.lab.test); echo "answered=${a:-none} dns=${s:-none}")" 'answered=(NOERROR|NXDOMAIN) dns=none'
check dns-hijack.8 "DoT to a server is routed as a connection (a connection log line)" "$(grep -E 'msg=connection .*destination=198\.51\.100\.53:853' $R/core.log | sed -n 's/.*outbound=\([^ ]*\).*/\1/p' | tail -1)" '.+'
stop_core; summary
