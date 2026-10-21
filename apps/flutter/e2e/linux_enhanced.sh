#!/usr/bin/env bash
# Enhanced mode of the Flutter app end to end on Linux (#401 step 2), the
# counterpart of crates/desktop/src/e2e_linux.rs for the native client: the
# installed release app (/usr/lib/ppvpn/ppvpn, scripts/install-linux-layout.sh)
# connects through the real privileged service, with a saved sign-in against
# linux_enhanced_backend.py and test/fakenode as the node. Checks that
# connecting brings up the TUN with the engine's policy rules (priorities
# 9091-9101) and routing table 2091, that a request to an outside address
# goes through the TUN and the node, and that the routing state is gone
# after a disconnect.
#
#   sudo test/netns/run.sh --script apps/flutter/e2e/linux_enhanced.sh <fakenode binary>
#
# Runs as root inside run.sh's namespace ppvpn-t (uplink ppvpn-w, 10.243.0.2),
# under an Xvfb display and a D-Bus session of its own (it starts both). Needs:
# - the app installed by scripts/install-linux-layout.sh;
# - the service installed by /usr/lib/ppvpn/ppvpn-service-install (the unit
#   and /usr/lib/ppvpn-service), then stopped: a systemd unit runs in the
#   host's namespace, so this script runs /usr/lib/ppvpn-service/ppvpn-service
#   itself, here, without a system bus (as e2e_linux.rs does);
# - xvfb-run, dbus-run-session, python3, curl, iproute2.
# The app runs with PPVPN_FLUTTER_E2E=enhanced (lib/e2e.dart) and its data
# under PPVPN_FLUTTER_HOME, a temporary directory, which holds the saved
# sign-in as a plain file (data/credentials, never the keyring; the client
# restores it via the refresh endpoints). Addresses: run.sh's 10.243.0.0/24, the TUN's own, the backend
# on 127.0.0.1:18090 and 192.0.2.10 (TEST-NET-1) for the HTTP target behind
# the node.
set -euo pipefail

APP=/usr/lib/ppvpn/ppvpn
SERVICE=/usr/lib/ppvpn-service/ppvpn-service
UNIT=/etc/systemd/system/ppvpn-service.service
SOCKET=/run/ppvpn/service.sock
UPLINK_NS=ppvpn-w NODE_IP=10.243.0.2 NODE_PORT=8388 NODE_KEY=AAAAAAAAAAAAAAAAAAAAAA==
TARGET_IP=192.0.2.10 TARGET_PORT=8080 TARGET_BODY=ppvpn-e2e-ok
API_PORT=18090
RULE_MIN=9091 RULE_MAX=9101 TABLE=2091

fakenode=$(realpath "${1:?usage: linux_enhanced.sh <fakenode binary>}")
here=$(cd "$(dirname "$0")" && pwd)

[ "${PPVPN_TEST_REAL_TUN:-}" = 1 ] || {
  echo "linux_enhanced.sh: run it with test/netns/run.sh --script" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "linux_enhanced.sh: run as root" >&2; exit 2; }
for path in "$APP" "$SERVICE" "$UNIT" "$fakenode"; do
  [ -e "$path" ] || { echo "linux_enhanced.sh: $path is missing (see the header)" >&2; exit 2; }
done

# A display and a session bus of this namespace's own (X11's and D-Bus's
# abstract sockets are per network namespace).
if [ -z "${PPVPN_E2E_SESSION:-}" ]; then
  export PPVPN_E2E_SESSION=1
  exec xvfb-run -a -s '-screen 0 1280x800x24 -nolisten tcp' dbus-run-session -- "$0" "$@"
fi

work=$(mktemp -d)
service_pid= app_pid=
log() { echo "e2e: $*"; }

routing_state() {
  echo "## ip -4 rule"; ip -4 rule show
  echo "## ip -6 rule"; ip -6 rule show 2>/dev/null || true
  echo "## table $TABLE (v4)"; ip -4 route show table "$TABLE" 2>/dev/null || true
  echo "## table $TABLE (v6)"; ip -6 route show table "$TABLE" 2>/dev/null || true
  echo "## links"; ip -o -br link show
}

dump() {
  routing_state
  for f in app.out service.out backend.log fakenode.log; do
    echo "## $f"; tail -n 60 "$work/$f" 2>/dev/null || true
  done
  echo "## client logs"; find "$work/home/logs" -name '*.log' -exec tail -n 40 {} + 2>/dev/null || true
  echo "## /var/log/ppvpn/ppvpn-service.log"; tail -n 40 /var/log/ppvpn/ppvpn-service.log 2>/dev/null || true
}

fail() {
  echo "::error::linux_enhanced.sh: $*"
  dump
  exit 1
}

cleanup() {
  exec 3>&- 2>/dev/null || true
  [ -z "$app_pid" ] || kill -9 "$app_pid" 2>/dev/null || true
  # SIGTERM: the service stops its instance on the way out.
  if [ -n "$service_pid" ] && kill -0 "$service_pid" 2>/dev/null; then
    kill -TERM "$service_pid" 2>/dev/null || true
    for _ in $(seq 150); do kill -0 "$service_pid" 2>/dev/null || break; sleep 0.2; done
    kill -9 "$service_pid" 2>/dev/null || true
  fi
  kill $(jobs -p) 2>/dev/null || true
  wait 2>/dev/null || true
}
trap cleanup EXIT

# until <what> <seconds> <command...>
until_ok() {
  local what=$1 deadline=$((SECONDS + $2)); shift 2
  until "$@" >/dev/null 2>&1; do
    [ "$SECONDS" -lt "$deadline" ] || fail "timed out waiting for $what"
    sleep 0.2
  done
}

ipv6=0
[ "$(cat /proc/sys/net/ipv6/conf/all/disable_ipv6 2>/dev/null)" = 0 ] && ipv6=1
[ "$ipv6" = 1 ] || log "IPv6 is disabled in this namespace; the IPv6 rule checks are skipped"

tun_links() { ip -o -d link show | grep ' tun type tun ' || true; }
rules() { ip "$1" rule show | awk -F: -v lo="$RULE_MIN" -v hi="$RULE_MAX" '$1 + 0 >= lo && $1 + 0 <= hi'; }
table_routes() { ip "$1" route show table "$TABLE" 2>/dev/null || true; }

routes_up() {
  [ -n "$(tun_links)" ] && [[ $(rules -4) == *"lookup $TABLE"* ]] && [ -n "$(table_routes -4)" ] &&
    { [ "$ipv6" = 0 ] || [ -n "$(rules -6)" ]; }
}
clean() {
  [ -z "$(tun_links)" ] && [ -z "$(rules -4)" ] && [ -z "$(table_routes -4)" ] &&
    { [ "$ipv6" = 0 ] || { [ -z "$(rules -6)" ] && [ -z "$(table_routes -6)" ]; }; }
}

# Lines the app printed after line $1 of app.out; `app_count <since>
# <regex>` counts those matching (grep -c reads all: no early exit into a
# pipefail).
app_lines() { tail -n +"$(($1 + 1))" "$work/app.out" 2>/dev/null || true; }
app_count() { app_lines "$1" | grep -cE "$2" || true; }
app_mark() { wc -l < "$work/app.out"; }

# wait_app <since line> <pattern> <seconds>: an `E2E <pattern>` line from the
# app after line <since>; an error phase, a failed action or the app's exit
# fails at once.
wait_app() {
  local since=$1 pattern=$2 deadline=$((SECONDS + $3))
  until [ "$(app_count "$since" "^E2E $pattern")" -gt 0 ]; do
    if [ "$(app_count "$since" '^E2E (phase error|action-error)')" -gt 0 ]; then
      fail "the app reported: $(app_lines "$since" | grep -E '^E2E (phase error|action-error)')"
    fi
    kill -0 "$app_pid" 2>/dev/null || fail "the app exited before \"E2E $pattern\""
    [ "$SECONDS" -lt "$deadline" ] || fail "no \"E2E $pattern\" from the app within $3 s"
    sleep 0.2
  done
}

# 0. The node, the service, the backend, the saved sign-in.
ip -n "$UPLINK_NS" addr add "$TARGET_IP/32" dev lo
ip netns exec "$UPLINK_NS" "$fakenode" -ss "$NODE_IP:$NODE_PORT" -key "$NODE_KEY" \
  -http "$TARGET_IP:$TARGET_PORT" > "$work/fakenode.log" 2>&1 &
until_ok "fakenode ready" 20 grep -q '^ready ' "$work/fakenode.log"

# No system bus: a network namespace does not separate D-Bus, so the core's
# per-link DNS would reach the host's systemd-resolved with this namespace's
# interface index (run.sh's no-host-dbus.sh hides /run/dbus too).
DBUS_SYSTEM_BUS_ADDRESS=unix:path=/run/ppvpn-e2e-no-system-bus MALLOC_ARENA_MAX=2 \
  "$SERVICE" > "$work/service.out" 2>&1 &
service_pid=$!
log "ppvpn-service started (pid $service_pid)"
until_ok "ppvpn-service on $SOCKET" 30 python3 -c \
  'import socket, sys; socket.socket(socket.AF_UNIX).connect(sys.argv[1])' "$SOCKET"

python3 -u "$here/linux_enhanced_backend.py" --port "$API_PORT" --log "$work/backend.log" \
  > "$work/backend.out" 2>&1 &
until_ok "the mock backend" 20 grep -q '^ready' "$work/backend.out"

export PPVPN_FLUTTER_HOME=$work/home
mkdir -p -m 0700 "$PPVPN_FLUTTER_HOME/data"
( umask 077; printf '%s' '{"version":1,"active_refresh":"jyr_e2e_saved"}' \
  > "$PPVPN_FLUTTER_HOME/data/credentials" )

# 1. The app: signed in, it connects in enhanced mode by itself.
mkfifo "$work/in"
: > "$work/app.out"
PPVPN_FLUTTER_E2E=enhanced PPVPN_API_BASE=http://127.0.0.1:$API_PORT PPVPN_TEST_TRUST_LOCAL_BACKEND=1 \
  "$APP" < "$work/in" > "$work/app.out" 2>&1 &
app_pid=$!
exec 3> "$work/in"
log "the app started (pid $app_pid)"
wait_app 0 ready 90
wait_app 0 'phase connected' 90
until_ok "routes up after connect" 30 routes_up
log "routes up after connect: TUN $(tun_links | awk -F': ' '{print $2}' | tr '\n' ' ')"

# 2. Traffic to an outside address takes the TUN and the node.
destination=$TARGET_IP:$TARGET_PORT
body= deadline=$((SECONDS + 30))
until body=$(curl -sS --max-time 10 "http://$destination/" 2>&1); do
  [ "$SECONDS" -lt "$deadline" ] || fail "GET http://$destination/: $body"
  log "GET http://$destination/: $body; retrying"
  sleep 1
done
case $body in
  *"$TARGET_BODY"*) ;;
  *) fail "unexpected answer from http://$destination/: $body" ;;
esac
until_ok "the node routed the request" 10 grep -qx "conn $destination" "$work/fakenode.log"
log "http://$destination/ answered through the node"

# 3. Disconnect: everything the engine installed is gone.
since=$(app_mark)
echo disconnect >&3
wait_app "$since" 'phase off' 60
until_ok "clean after disconnect" 30 clean
log "clean after disconnect"

# 4. Quit as the tray does.
since=$(app_mark)
echo quit >&3
wait_app "$since" quit 60
for _ in $(seq 150); do kill -0 "$app_pid" 2>/dev/null || break; sleep 0.2; done
kill -0 "$app_pid" 2>/dev/null && fail "the app did not exit after quit"
app_pid=
clean || fail "routing state left after the app quit"

echo "--- app"; cat "$work/app.out"
echo "--- backend requests"; cat "$work/backend.log" || true
log ok
