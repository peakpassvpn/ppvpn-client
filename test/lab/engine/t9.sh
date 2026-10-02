#!/bin/sh
# Checklist 9: memory, idle and under load (TUN + local proxy).
. /lab/core.sh
ENGINE=$1
start_core $ENGINE --tun=true --local-proxy=true >/dev/null && apply >/dev/null && api start >/dev/null
rss() { for p in $(pgrep -f "serve --socket") $(pgrep -x sail); do awk -v n=$(cat /proc/$p/comm) '/VmRSS|VmHWM/{printf "%s %s=%dMB ", n, $1, $2/1024}' /proc/$p/status; done; }
sleep 10; echo "idle 10s: $(rss)"
s=$(date +%s.%N)
pids=""
for i in 1 2 3 4; do curl -s -m 120 -o /dev/null -w "%{size_download}\n" "http://198.51.100.50/bytes?200000000" > /tmp/dl$i & pids="$pids $!"; done
sleep 3; echo "during load: $(rss)"
wait $pids
e=$(date +%s.%N)
echo "4x200MB via TUN: $(cat /tmp/dl1 /tmp/dl2 /tmp/dl3 /tmp/dl4 | tr '\n' ' ') in $(echo "$e - $s" | bc)s; after: $(rss)"
sleep 15; echo "15s later: $(rss)"
stop_core
