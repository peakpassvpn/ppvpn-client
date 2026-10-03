#!/usr/bin/env bash
# Start the installed PPVPN once on a virtual display and check it came up: the native
# client library loaded and its first state reached the UI thread, without a crash.
#
# Usage: apps/linux/scripts/smoke-test.sh [expected package format: deb|rpm]
#
# Needs Xvfb and dbus-run-session; runs as an unprivileged or root user in a throwaway
# environment (container, chroot, CI runner). Uses a private HOME, so it never touches
# the invoking user's PPVPN data.
set -euo pipefail

FORMAT="${1:-}"
APP=/usr/lib/ppvpn/ppvpn
[[ -x "$APP" ]] || { echo "FAIL: $APP is not installed" >&2; exit 1; }
[[ "$(readlink -f /usr/bin/ppvpn)" == "$APP" ]] || { echo "FAIL: /usr/bin/ppvpn does not point at $APP" >&2; exit 1; }
if [[ -n "$FORMAT" ]]; then
  [[ "$(cat /usr/lib/ppvpn/package-format)" == "$FORMAT" ]] || { echo "FAIL: package-format is not $FORMAT" >&2; exit 1; }
fi
for file in /usr/share/applications/com.peakpassvpn.ppvpn.desktop.desktop \
    /usr/share/icons/hicolor/128x128/apps/com.peakpassvpn.ppvpn.desktop.png \
    /usr/lib/ppvpn/ppvpn-core /usr/lib/ppvpn/ppvpn-service /usr/lib/ppvpn/ppvpn-service-install \
    /usr/lib/ppvpn/ppvpn-push-agent /etc/xdg/autostart/com.peakpassvpn.ppvpn.push-agent.desktop; do
  [[ -e "$file" ]] || { echo "FAIL: missing $file" >&2; exit 1; }
done

WORK="$(mktemp -d)"
trap 'kill "${XVFB_PID:-}" 2>/dev/null || true; rm -rf "$WORK"' EXIT
export HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK/run"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset XDG_DATA_HOME XDG_STATE_HOME XDG_CONFIG_HOME

display=":$((90 + RANDOM % 9))"
Xvfb "$display" -screen 0 1024x768x24 >/dev/null 2>&1 &
XVFB_PID=$!
export DISPLAY="$display" GTK_A11Y=none GSK_RENDERER=cairo
sleep 2

# Let it start, restore (nothing saved) and settle; then stop it.
dbus-run-session -- bash -c "timeout --signal=TERM 20 /usr/bin/ppvpn > '$WORK/app.out' 2>&1 || true"

logs="$HOME/.local/state/ppvpn/logs"
client_log="$(ls "$logs"/ppvpn-client.*.log 2>/dev/null | head -1)"
app_log="$(ls "$logs"/ppvpn-app.*.log 2>/dev/null | head -1)"
agent_log="$(ls "$logs"/ppvpn-push-agent.*.log 2>/dev/null | head -1)"
show() { echo "--- $1"; cat "$2" 2>/dev/null | tail -20 || true; }
check() {
  if ! grep -q "$2" "$1" 2>/dev/null; then
    echo "FAIL: '$2' not found in $(basename "${1:-missing log}")" >&2
    show "app output" "$WORK/app.out"; show "client log" "$client_log"; show "app log" "$app_log"
    exit 1
  fi
}
check "$client_log" "ppvpn-client started"
check "$app_log" "listener: OnSnapshot"
# The app starts the push agent; it ends with the session bus.
check "$agent_log" "push agent: run"
agent_running() {
  local cmdline
  for cmdline in /proc/[0-9]*/cmdline; do
    [[ "$(tr '\0' ' ' < "$cmdline" 2>/dev/null)" == /usr/lib/ppvpn/ppvpn-push-agent* ]] && return 0
  done
  return 1
}
sleep 1
if agent_running; then
  echo "FAIL: the push agent outlived its session" >&2
  exit 1
fi
if grep -Eq "Unhandled exception|DllNotFoundException|EntryPointNotFoundException" "$WORK/app.out"; then
  show "app output" "$WORK/app.out"
  echo "FAIL: the app crashed" >&2
  exit 1
fi
echo "OK: PPVPN started, loaded libppvpn_client and delivered its first state; the push agent ran and exited with the session"
