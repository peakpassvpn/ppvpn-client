#!/bin/sh
# B5 on Sail >= 0.16: the runtime/control API is no longer open to any local
# process. Wanted: api.listen without a secret is refused by `-T`; with one,
# a call without the bearer token gets 401 and one with it works; a
# non-loopback listen is refused; "api":{} serves a unix socket (api.sock,
# mode 0600) in the data directory.
#   docker exec lab-client sh /lab/repro/b5-api-auth.sh /work/<sail>
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
L=$OUT/b5-api-auth.log
base='"inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7970}],"outbounds":[{"type":"direct","tag":"direct"}]'
t() { echo "{$base,\"api\":$1}" > /tmp/b5.json; $BIN -c /tmp/b5.json -T > /tmp/b5.out 2>&1; echo "exit=$? $(head -1 /tmp/b5.out | cut -c1-140)"; }
{
echo "engine=sail $($BIN -V)"
echo "-T api.listen without secret:    $(t '{"listen":"127.0.0.1:7971"}')"
echo "-T api.listen 0.0.0.0 with secret: $(t '{"listen":"0.0.0.0:7971","secret":"'$API_SECRET'"}')"
echo "-T api.listen with secret:       $(t '{"listen":"127.0.0.1:7971","secret":"'$API_SECRET'"}')"
echo "{$base,\"api\":{\"listen\":\"127.0.0.1:7971\",\"secret\":\"$API_SECRET\"}}" > /tmp/b5-run.json
run_engine /tmp/b5-run.json $OUT/b5-api-auth.log.full
echo "GET inbounds without token: $(curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:7971/api/v1/runtime/inbounds)"
echo "GET inbounds wrong token:   $(curl -s -o /dev/null -w '%{http_code}' -H 'Authorization: Bearer wrong' http://127.0.0.1:7971/api/v1/runtime/inbounds)"
echo "GET inbounds with token:    $(api_curl -s -o /dev/null -w '%{http_code}' http://127.0.0.1:7971/api/v1/runtime/inbounds)"
echo "POST reload without token: $(curl -s -o /dev/null -w '%{http_code}' -X POST http://127.0.0.1:7971/api/v1/runtime/reload)"
stop_engine
D=/tmp/b5-data; rm -rf $D; mkdir -p $D
echo "{$base,\"api\":{}}" > /tmp/b5-sock.json
( cd $D && NO_COLOR=1 $BIN -c /tmp/b5-sock.json -D $D > $OUT/b5-api-sock.log.full 2>&1 & echo $! > /tmp/b5.pid ); sleep 3
S=$D/api.sock; [ -S "$S" ] || S=""
echo "\"api\":{} socket: ${S:-none} mode $( [ -n "$S" ] && stat -c %a "$S")"
[ -n "$S" ] && echo "GET inbounds over the socket: $(curl -s -o /dev/null -w '%{http_code}' --unix-socket "$S" http://sail/api/v1/runtime/inbounds)"
kill $(cat /tmp/b5.pid) 2>/dev/null; sleep 0.5
} | tee $L
