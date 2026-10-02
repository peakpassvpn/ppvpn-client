#!/bin/sh
# Item 15 (+12): local binary rule set.
#  a) replacing the file is not picked up until a reload (sing-box watches
#     local rule-set files and reloads them by itself);
#  b) a corrupt file: reload keeps the old set and answers 202 with no
#     reason; start and -T fail with "route.rule_set[0]: [rs]" and no cause.
#  c) the 407 realm of a mixed inbound with users is "sail".
# rs-a.srs matches www.lab.test, rs-b.srs video.lab.test (made with
# sing-box's srs writer: test/lab/engine/mksrs); matched -> node (exit .12),
# else direct (exit .100). Same config on both engines.
#   docker exec lab-client sh /lab/repro/item15-ruleset.sh sail /work/sail-rel
#   docker exec lab-client sh /lab/repro/item15-ruleset.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
D=/tmp/rs15; rm -rf $D; mkdir -p $D; C=$D/config.json; L=$OUT/item15-$ENGINE.log
API=''; [ $ENGINE = sail ] && API=",\"api\":$(api_json 7921)"
cat > $C <<J
{"log":{"level":"debug","timestamp":true},
 "inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7920,"users":[{"username":"u","password":"p"}]}],
 "outbounds":[$(ss_node node),{"type":"direct","tag":"direct"}],
 "route":{"rule_set":[{"type":"local","tag":"rs","format":"binary","path":"$D/current.srs"}],
  "rules":[{"rule_set":"rs","outbound":"node"}],"final":"direct"}$API}
J
redact $C > $OUT/item15-config-$ENGINE.json
cp /work/rs-a.srs $D/current.srs
run_engine $C $L.full
p() { curl -s -m 5 -x http://u:p@127.0.0.1:7920 http://$1/ | exit_of; }
reload() {
  if [ $ENGINE = sail ]; then code=$(api_curl -s -o $D/body -w %{http_code} -X POST 127.0.0.1:7921/api/v1/runtime/reload); echo "$code body=[$(cat $D/body)]"
  else echo "(sing-box: no reload API, relies on its file watch)"; fi
}
{
echo "engine=$ENGINE $($BIN -V 2>/dev/null)"
echo "a) set rs-a: www=$(p www.lab.test) video=$(p video.lab.test)   (expect www .12, video .100)"
cp /work/rs-b.srs $D/new && mv $D/new $D/current.srs; sleep 3
echo "   file replaced by rs-b, 3 s later, no reload: www=$(p www.lab.test) video=$(p video.lab.test)   (rs-b: www .100, video .12)"
echo "   reload: $(reload)"; sleep 1
echo "   after reload: www=$(p www.lab.test) video=$(p video.lab.test)"
head -c 30 /work/rs-b.srs > $D/new && mv $D/new $D/current.srs; sleep 3
echo "b) file truncated to 30 bytes, 3 s later: www=$(p www.lab.test) video=$(p video.lab.test)"
echo "   reload: $(reload)"; sleep 1
echo "   after reload: www=$(p www.lab.test) video=$(p video.lab.test) process=$(kill -0 $EPID 2>/dev/null && echo alive || echo gone)"
echo "c) no credentials -> $(curl -s -m 5 -D - -o /dev/null -x http://127.0.0.1:7920 http://198.51.100.50/ | tr -d '\r' | grep -iE '^HTTP|^Proxy-Auth' | tr '\n' ' ')"
} | tee $L
stop_engine
{
case $ENGINE in
  sail) echo "   start with the truncated file: $(NO_COLOR=1 timeout 4 $BIN -c $C 2>&1 | grep -iE 'fail|error' | head -2 | tr '\n' ' ')"
        echo "   -T with the truncated file: $($BIN -c $C -T 2>&1 | head -2 | tr '\n' ' ')";;
  sing-box) echo "   check with the truncated file: $($BIN check -c $C 2>&1 | head -2 | tr '\n' ' ')";;
esac
echo "--- engine log (rule set / reload lines):"
grep -iE 'rule.?set|reload|srs' $L.full | cut -c1-220 | tail -10
} | tee -a $L
