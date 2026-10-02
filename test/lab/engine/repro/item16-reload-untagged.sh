#!/bin/sh
# Item 16: reload after the config file was replaced by an outbound without a
# tag and of an unknown type. Run inside lab-client:
#   docker exec lab-client sh /lab/repro/item16-reload-untagged.sh /work/sail-rel [variant]
# variant: untagged (default) | tagged | untagged-known
# Prints the reload request's result, whether the API, the proxy and the
# process still answer during and after it, and the log.
SAIL=${1:-/work/sail-rel}; VARIANT=${2:-untagged}
BIN=$SAIL; . /lab/repro/common.sh
D=/tmp/item16; rm -rf $D; mkdir -p $D
cat > $D/config.json <<J
{"log":{"level":"debug","timestamp":true},
 "inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7980}],
 "outbounds":[{"type":"direct","tag":"direct"}],
 "api":$(api_json 7981)}
J
cp $D/config.json $D/config.before.json
NO_COLOR=1 $SAIL -c $D/config.json > $D/sail.log 2>&1 &
PID=$!; sleep 1
api() { api_curl -s -m 3 -o /dev/null -w "%{http_code}" -X POST 127.0.0.1:7981/api/v1/runtime/$1; }
probe() { echo "  [$1] process=$(kill -0 $PID 2>/dev/null && echo alive || echo gone) api(GET assets)=$(api_curl -s -m 3 -o /dev/null -w %{http_code} 127.0.0.1:7981/api/v1/runtime/assets) proxy=$(curl -s -m 3 -o /dev/null -w %{http_code} -x http://127.0.0.1:7980 http://198.51.100.50/)"; }
probe before
case $VARIANT in
  untagged) echo '{"outbounds":[{"type":"nope"}]}' > $D/config.json ;;
  tagged) echo '{"outbounds":[{"type":"nope","tag":"x"}]}' > $D/config.json ;;
  untagged-known) echo '{"outbounds":[{"type":"direct"}]}' > $D/config.json ;;
esac
cp $D/config.json $D/config.after.json
echo "config after: $(cat $D/config.json)"
s=$(date +%s%N)
api_curl -s -m 60 -o $D/reload.body -w "%{http_code}" -X POST 127.0.0.1:7981/api/v1/runtime/reload > $D/reload.code &
RL=$!
sleep 2; probe "2s into reload"
wait $RL
e=$(date +%s%N)
echo "reload: http=$(cat $D/reload.code) body=[$(cat $D/reload.body)] after $(( (e-s)/1000000 )) ms (000 = no answer within the 60 s client timeout)"
probe after
kill $PID 2>/dev/null; sleep 0.5; kill -9 $PID 2>/dev/null
echo "--- sail log from the reload on:"; sed -n '/reloading/,$p' $D/sail.log | cut -c1-260 | head -20
