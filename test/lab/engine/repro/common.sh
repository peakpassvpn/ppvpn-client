# Sourced by the repro scripts inside lab-client. ENGINE is "sail" or
# "sing-box"; BIN its binary. Lab keys are read from /work at run time and
# redacted from every config copy written under $OUT.
OUT=${OUT:-/work/repro-logs}; mkdir -p $OUT
SSK=$(cat /work/ss-server.key); SSU=$(cat /work/ss-user.key); UUID=$(cat /work/uuid.txt)
PUB=$(sed -n 's/^PublicKey: //p' /work/reality.txt)
ss_node() { echo "{\"type\":\"shadowsocks\",\"tag\":\"$1\",\"server\":\"198.51.100.12\",\"server_port\":8443,\"method\":\"2022-blake3-aes-128-gcm\",\"password\":\"$SSK:$SSU\"}"; }
vless_node() { echo "{\"type\":\"vless\",\"tag\":\"$1\",\"server\":\"198.51.100.11\",\"server_port\":443,\"uuid\":\"$UUID\",\"flow\":\"xtls-rprx-vision\",\"tls\":{\"enabled\":true,\"server_name\":\"www.lab.test\",\"utls\":{\"enabled\":true,\"fingerprint\":\"chrome\"},\"reality\":{\"enabled\":true,\"public_key\":\"$PUB\",\"short_id\":\"01\"}}}"; }
redact() { sed -e "s#$SSK#<ss-server-key>#g" -e "s#$SSU#<ss-user-key>#g" -e "s#$UUID#<uuid>#g" -e "s#$PUB#<reality-public-key>#g" "$1"; }
run_engine() { # config log
  case $ENGINE in
    sail) NO_COLOR=1 $BIN -c $1 > $2 2>&1 & ;;
    sing-box) $BIN run -c $1 --disable-color > $2 2>&1 & ;;
  esac
  EPID=$!; sleep 3
}
stop_engine() { # by pid, then anything still running this binary
  kill $EPID 2>/dev/null; sleep 1; kill -9 $EPID 2>/dev/null
  pkill -f "^$BIN( |\$)" 2>/dev/null; sleep 0.3; pkill -9 -f "^$BIN( |\$)" 2>/dev/null; true; }
exit_of() { cut -d' ' -f1 | sed 's/exit=//'; }

# Runtime API on 127.0.0.1:<port>. Builds since 187fc131 (B5) require a
# secret on a TCP listen; older ones reject the field. api_json prints the
# "api" object the running build takes; api_curl adds the header.
API_SECRET=0123456789abcdef0123456789abcdef0123
api_json() { # port
  t=/tmp/api-probe.json
  echo "{\"inbounds\":[],\"outbounds\":[{\"type\":\"direct\",\"tag\":\"direct\"}],\"api\":{\"listen\":\"127.0.0.1:$1\",\"secret\":\"$API_SECRET\"}}" > $t
  if $BIN -c $t -T >/dev/null 2>&1; then echo "{\"listen\":\"127.0.0.1:$1\",\"secret\":\"$API_SECRET\"}"; else echo "{\"listen\":\"127.0.0.1:$1\"}"; fi
}
api_curl() { curl -H "Authorization: Bearer $API_SECRET" "$@"; }
