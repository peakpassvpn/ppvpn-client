#!/usr/bin/env bash
# Inside the container: runs the prototype (/app/bundle) under Xvfb with a
# traffic sample pushed every second (PPVPN_FLUTTER_SYNTHETIC_TRAFFIC),
# signed out, and samples its RSS every 10 s until /out holds HOURS of
# samples in all. Each line goes to /out/curve.csv and to stdout:
#   segment,seconds_in_segment,total_seconds,rss_kib
# A run after a preemption (or an app exit) starts a new segment and goes on
# from the total already in /out/curve.csv.
set -euo pipefail

hours=${HOURS:-24}
csv=/out/curve.csv
[ -s "$csv" ] || echo "segment,seconds_in_segment,total_seconds,rss_kib" > "$csv"
last=$(tail -n 1 "$csv")
segment=1 total=0
if [[ $last != segment* ]]; then
  IFS=, read -r s _ t _ <<<"$last"
  segment=$((s + 1)) total=$t
fi
limit=$((hours * 3600))
export HOME=/tmp/home XDG_RUNTIME_DIR=/tmp/run PPVPN_FLUTTER_SYNTHETIC_TRAFFIC=1
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"; chmod 700 "$XDG_RUNTIME_DIR"

failed=0
while (( total < limit )); do
  echo "# segment $segment from total ${total}s" >&2
  xvfb-run -a -s '-screen 0 1280x800x24' dbus-run-session -- /app/bundle/ppvpn >/out/app-$segment.log 2>&1 &
  sleep 20
  pid=$(pgrep -x ppvpn | head -1 || true)
  if [ -z "$pid" ]; then
    # Three starts in a row without a process: stop instead of spinning.
    failed=$((failed + 1))
    echo "# segment $segment: the app did not start (see /out/app-$segment.log)" >&2
    if (( failed >= 3 )); then echo "# giving up after $failed failed starts" >&2; exit 1; fi
    segment=$((segment + 1))
    continue
  fi
  failed=0
  start=$SECONDS
  while [ -n "$pid" ] && [ -r "/proc/$pid/status" ] && (( total < limit )); do
    rss=$(awk '/^VmRSS:/ {print $2}' "/proc/$pid/status")
    seg=$((SECONDS - start))
    line="$segment,$seg,$((total + 0)),$rss"
    echo "$line" >> "$csv"
    echo "$line"
    sleep 10
    total=$((total + 10))
  done
  pkill -x ppvpn || true
  wait || true
  segment=$((segment + 1))
done
echo "# done: ${total}s" >&2
