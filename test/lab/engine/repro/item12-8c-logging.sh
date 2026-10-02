#!/bin/sh
# 12b: -T prints every warning (stderr) and "ok" (stdout).
# 12c: a load error keeps its cause (truncated local rule-set).
# 8c:  log.redact ["destination","source","process"] hides destinations at
#      INFO/WARN/ERROR (also Clash API /logs), not at DEBUG.
#   docker exec <client> sh /lab/repro/item12-8c-logging.sh /work/<sail>
BIN=$1; ENGINE=sail; . /lab/repro/common.sh
D=/tmp/i12; rm -rf $D; mkdir -p $D; L=$OUT/item12-8c.log
{
echo "engine=sail $($BIN -V)"
echo '{"inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7901},{"type":"tun","tag":"tun","address":["172.19.9.1/30"],"stack":"gvisor"}],"outbounds":[{"type":"direct","tag":"direct"}]}' > $D/warn.json
$BIN -c $D/warn.json -T > $D/out 2> $D/err; rc=$?
echo "12b -T: exit=$rc stdout=[$(cat $D/out)] stderr=[$(cat $D/err | tr '\n' '|')]"
head -c 30 /work/rs-b.srs > $D/bad.srs
echo "{\"inbounds\":[],\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}],\"route\":{\"rule_set\":[{\"type\":\"local\",\"tag\":\"rs\",\"format\":\"binary\",\"path\":\"$D/bad.srs\"}],\"rules\":[{\"rule_set\":\"rs\",\"outbound\":\"direct\"}]}}" > $D/bad.json
echo "12c -T truncated rule-set: $($BIN -c $D/bad.json -T 2>&1 | tr '\n' '|')"
echo "12c start truncated rule-set: $(NO_COLOR=1 timeout 3 $BIN -c $D/bad.json 2>&1 | grep -iE 'fail|error|rule' | head -2 | tr '\n' '|')"
for level in info debug; do
  echo "{\"log\":{\"level\":\"$level\",\"redact\":[\"destination\",\"source\",\"process\"]},\"inbounds\":[{\"type\":\"mixed\",\"tag\":\"in\",\"listen\":\"127.0.0.1\",\"listen_port\":7902}],\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}],\"clash_api\":{\"external_controller\":\"127.0.0.1:7903\",\"secret\":\"$API_SECRET\"}}" > $D/r-$level.json
  NO_COLOR=1 $BIN -c $D/r-$level.json > $D/r-$level.log 2>&1 & P=$!; sleep 1
  ( curl -s -N -m 3 -H "Authorization: Bearer $API_SECRET" "127.0.0.1:7903/logs?level=$level" > $D/clash-$level.log & ); sleep 0.5
  curl -s -m 3 -o /dev/null -x http://127.0.0.1:7902 http://www.lab.test/; sleep 2.5; kill $P; sleep 0.3
  echo "8c log.level=$level, lines naming www.lab.test or 198.51.100.50 in the log: $(grep -cE 'www.lab.test|198.51.100.50' $D/r-$level.log), in Clash /logs: $(grep -cE 'www.lab.test|198.51.100.50' $D/clash-$level.log)"
  echo "   handled line: $(grep -m1 'handled' $D/r-$level.log | sed 's/.*handled/handled/' | cut -c1-140)"
done
} | tee $L
