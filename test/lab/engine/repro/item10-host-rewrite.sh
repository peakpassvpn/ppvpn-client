#!/bin/sh
# Item 10: the HTTP proxy (mixed inbound) rewrites the Host header of a
# plain-HTTP request to an address: "198.51.100.50" becomes "198.51.100.50:80".
# The lab target echoes the Host it received. Same config on both engines.
#   docker exec lab-client sh /lab/repro/item10-host-rewrite.sh sail /work/sail-rel
#   docker exec lab-client sh /lab/repro/item10-host-rewrite.sh sing-box /work/sing-box
ENGINE=$1; BIN=$2; . /lab/repro/common.sh
C=/tmp/i10.json; L=$OUT/item10-$ENGINE.log
echo '{"log":{"level":"debug"},"inbounds":[{"type":"mixed","tag":"in","listen":"127.0.0.1","listen_port":7930}],"outbounds":[{"type":"direct","tag":"direct"}]}' > $C
cp $C $OUT/item10-config.json
run_engine $C $L.full
{
echo "engine=$ENGINE"
echo "curl -x http://127.0.0.1:7930 http://198.51.100.50/      -> $(curl -s -m 5 -x http://127.0.0.1:7930 http://198.51.100.50/)"
echo "curl -x http://127.0.0.1:7930 http://www.lab.test/     -> $(curl -s -m 5 -x http://127.0.0.1:7930 http://www.lab.test/)"
echo "curl -x http://127.0.0.1:7930 http://198.51.100.50:80/   -> $(curl -s -m 5 -x http://127.0.0.1:7930 http://198.51.100.50:80/)"
echo "(no proxy) curl http://198.51.100.50/                   -> $(curl -s -m 5 http://198.51.100.50/)"
} | tee $L
stop_engine
