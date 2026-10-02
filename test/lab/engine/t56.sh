#!/bin/sh
# Checklist 2, 5, 6 on the local proxy core (no TUN): protocols, per-user
# routing, auth failures, global mode, select-node, traffic, connections.
. /lab/core.sh
ENGINE=${1:-sail}
start_core $ENGINE --tun=false --local-proxy=true || exit 1
echo "## apply: $(apply)"; echo "## start: $(api start)"
echo "## status: $(api get-status | jq -c '.data | {state, revision, selected_node_id, selected_ingress, routing_mode, nodes}')"
JP=$(cred '{"node_id":"jp"}'); US=$(cred '{"node_id":"us"}'); RT=$(cred '{"kind":"routed"}')
T=http://198.51.100.50/
echo "## 2/5 node user jp (VLESS+REALITY primary, expect exit .11): $(curl -s -m 8 -x http://$JP $T)"
echo "## 2/5 node user us (AnyTLS, expect exit .13):                $(curl -s -m 8 -x http://$US $T)"
echo "## 5 socks5 node user us (expect .13):                        $(curl -s -m 8 -x socks5h://$US $T)"
echo "## 5 https via jp (expect .11):                               $(curl -s -m 8 -x http://$JP https://www.lab.test/)"
echo "## 5 routed user, final=selected(jp) (expect .11):            $(curl -s -m 8 -x http://$RT $T)"
echo "## 5 routed user, rule video.lab.test -> us (expect .13):     $(curl -s -m 8 -x http://$RT http://x.video.lab.test/)"
echo "## 5 no auth -> 407:"; curl -s -m 5 -D - -o /dev/null -x http://127.0.0.1:${JP##*:} $T | tr -d '\r' | sed 's/^/     /'
echo "## 5 wrong password -> 407: $(curl -s -m 5 -o /dev/null -w '%{http_code}' -x http://${JP%%:*}:wrong@${JP##*@} $T)"
echo "## 5 unknown user -> 407:   $(curl -s -m 5 -o /dev/null -w '%{http_code}' -x http://nobody:$(echo $JP | cut -d: -f2 | cut -d@ -f1)@${JP##*@} $T)"
echo "## 5 socks5 wrong password (RFC 1929 reply bytes, expect 0101):"
python3 - "${JP##*@}" <<'PY'
import socket, sys
host, port = sys.argv[1].split(":")
s = socket.create_connection((host, int(port)), 5); s.sendall(b"\x05\x01\x02"); print("     method reply:", s.recv(2).hex())
s.sendall(b"\x01\x01x\x01y"); print("     auth reply:", s.recv(2).hex(), "then closed:", s.recv(10) == b"")
s = socket.create_connection((host, int(port)), 5); s.sendall(b"\x05\x01\x00"); print("     no-auth method offered, reply:", s.recv(2).hex())
PY
echo "## 6 select-node us: $(api select-node '{"node_id":"us"}')"
echo "##   routed user after select (expect .13): $(curl -s -m 8 -x http://$RT $T)"
echo "##   node user jp unchanged (expect .11):   $(curl -s -m 8 -x http://$JP $T)"
echo "## 6 select-node jp: $(api select-node '{"node_id":"jp"}' | jq -c .ok)   routed -> $(curl -s -m 8 -x http://$RT $T)"
echo "## 5 global mode: $(apply global)"
echo "##   routed user video.lab.test in global (expect selected jp .11): $(curl -s -m 8 -x http://$RT http://x.video.lab.test/)"
echo "##   back to rules: $(apply rules)   video -> $(curl -s -m 8 -x http://$RT http://x.video.lab.test/)"
echo "## 6 traffic before: $(api get-traffic | jq -c .data)"
curl -s -m 10 -x http://$JP "http://198.51.100.50/drip?4" -o /dev/null &
DRIP=$!
curl -s -m 10 -x http://$US "http://198.51.100.50/bytes?3000000" -o /dev/null
sleep 1
echo "## 6 connections during a jp download: $(api get-connections | jq -c .data)"
wait $DRIP
echo "## 6 traffic after (3 MB + drip): $(api get-traffic | jq -c .data)"
echo "## 6 connections after: $(api get-connections | jq -c '.data|length')"
echo "## 6 probe-availability jp: $(api probe-availability '{"node_id":"jp","target":"http://198.51.100.50/generate_204"}' | jq -c .data)"
echo "## 6 system proxy on: $(api set-system-proxy '{"enabled":true}' | jq -c .)"
SP=$(api get-system-proxy-endpoints | jq -r '.data | .http // .endpoints // . | tostring'); echo "##   endpoints: $SP"
PORT=$(api get-status | jq -r '.data.system_proxy | .. | numbers' | head -1)
echo "##   via system proxy port $PORT (no auth, expect .11): $(curl -s -m 8 -x http://127.0.0.1:$PORT $T)"
echo "## 6 system proxy off: $(api set-system-proxy '{"enabled":false}' | jq -c .ok)  then: $(curl -s -m 3 -o /dev/null -w '%{http_code} %{errormsg}' -x http://127.0.0.1:$PORT $T)"
echo "## 6 hot apply (revision lab#2):"; sed 's/lab#1/lab#2/' /work/profile.json > /tmp/p2.json
( curl -s -m 10 -x http://$JP "http://198.51.100.50/drip?5" -o /dev/null -w '     in-flight download during apply: http=%{http_code} bytes=%{size_download} err=%{errormsg}\n' & )
sleep 1; t0=$(date +%s%N); PROFILE=/tmp/p2.json apply | sed 's/^/     /'; t1=$(date +%s%N); echo "     apply took $(( (t1-t0)/1000000 )) ms"
echo "     after apply jp -> $(curl -s -m 8 -x http://$JP $T)"; sleep 5
echo "## status: $(api get-status | jq -c '.data | {state, revision, selected_node_id, selected_ingress}')"
echo "## stop: $(api stop)"; sleep 0.5; echo "## sail processes after stop: $(pgrep -x sail | wc -l)"
echo "## core log (non-debug):"; grep -v 'level=debug' $R/core.log | cut -c1-330 | sed 's/^/     /' | tail -30
stop_core
