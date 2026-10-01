#!/usr/bin/env bash
# Runs `go test -race "$@"` and retries a failed run when every data race it
# reported is a known upstream one. Go's race detector has no suppression
# list, and these races are inside sing / sing-shadowsocks2, on paths sing-box
# takes for ordinary connections, so about one run in forty trips one:
#
#   - bufio.CachedConn / CachedPacketConn: the cached buffer is read by
#     Read/ReaderReplaceable and cleared by Close from the other copy
#     direction (SagerNet/sing#112).
#   - shadowaead_2022.clientConn: writeRequest vs Close
#     (SagerNet/sing-shadowsocks2#9).
#
# A run with any other race, or that fails without reporting a race, fails at
# once and is never retried. Remove a pattern when its upstream fix is
# vendored.
set -uo pipefail

# Bracket classes, not backslashes: awk -v would strip the escapes.
known='bufio[.][(][*]Cached(Packet)?Conn[)]|shadowaead_2022[.][(][*]clientConn[)]'
attempts=3

# only_known_races FILE: succeeds when FILE reports at least one data race and
# every report has a known upstream frame.
only_known_races() {
	awk -v known="$known" '
		/^==================$/ {
			if (inblock) { total++; if (block ~ known) matched++; inblock = 0; block = "" }
			next
		}
		/^WARNING: DATA RACE$/ { inblock = 1 }
		inblock { block = block "\n" $0 }
		END { exit !(total > 0 && matched == total) }
	' "$1"
}

if [ "${1:-}" = "--classify" ]; then
	only_known_races "$2"
	exit
fi

log=$(mktemp)
trap 'rm -f "$log"' EXIT
for attempt in $(seq 1 "$attempts"); do
	go test -race "$@" 2>&1 | tee "$log"
	status=${PIPESTATUS[0]}
	if [ "$status" -eq 0 ]; then
		exit 0
	fi
	if [ "$attempt" -eq "$attempts" ] || ! only_known_races "$log"; then
		exit "$status"
	fi
	echo "test-race: only known upstream races (SagerNet/sing#112, SagerNet/sing-shadowsocks2#9) were reported; retrying ($attempt/$attempts)" >&2
done
