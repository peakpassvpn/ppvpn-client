#!/bin/sh
# Item 9: POST /api/v1/runtime/inbounds with an inbound that has no
# listen/listen_port is accepted (200, "added inbound") but nothing listens.
#   docker exec lab-client sh /lab/repro/item09-inbound-no-port.sh /work/sail-rel
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
C=/tmp/i9.json; L=$OUT/item09.log
cat > $C <<J
{"log":{"level":"debug","timestamp":true},"inbounds":[],"outbounds":[{"type":"direct","tag":"direct"}],"api":$(api_json 7941)}
J
cp $C $OUT/item09-config.json
run_engine $C $L.full
{
echo "engine=sail $($BIN -V)"
for body in '{"type":"mixed","tag":"no-port"}' '{"type":"mixed","tag":"with-port","listen":"127.0.0.1","listen_port":7942}'; do
  echo "POST $body -> $(api_curl -s -o /tmp/b -w %{http_code} -X POST -H 'Content-Type: application/json' -d "$body" 127.0.0.1:7941/api/v1/runtime/inbounds) [$(cat /tmp/b)]"
done
sleep 0.5; echo "listening sockets of sail:"; ss -ltnp | grep sail | awk '{print "  " $4}'
} | tee $L
stop_engine
echo "--- log:" >> $L; grep -E 'inbound|listening' $L.full | cut -c1-200 >> $L
