#!/usr/bin/env bash
# Checks that this workspace's [patch.crates-io] is sail's, as of the sail
# commit Cargo.toml pins: a crate linking sail must build against the same
# forks (btls with the REALITY/ECH hooks, quinn-proto, ...). Until sail ships
# a check of its own. Needs curl and network access to GitHub.
#
#   tools/sail-patch-check.sh
set -euo pipefail
root=$(cd "$(dirname "$0")/.." && pwd)
rev=$(sed -n 's/^sail = {.*rev = "\([0-9a-f]\{40\}\)".*/\1/p' "$root/Cargo.toml")
[ -n "$rev" ] || { echo "sail-patch-check: no sail = { ... rev = \"<sha>\" } in Cargo.toml" >&2; exit 1; }
# [patch.crates-io] entries, comments and blank lines left out, sorted.
patches() { awk '/^\[patch\.crates-io\]/{f=1; next} /^\[/{f=0} f' | sed 's/#.*//; s/[[:space:]]*$//' | grep -v '^$' | sort; }
theirs=$(curl -fsSL "https://raw.githubusercontent.com/peakpassvpn/sail/$rev/Cargo.toml" | patches)
ours=$(patches < "$root/Cargo.toml")
[ -n "$theirs" ] || { echo "sail-patch-check: sail $rev has no [patch.crates-io]?" >&2; exit 1; }
if [ "$ours" != "$theirs" ]; then
	echo "sail-patch-check: [patch.crates-io] differs from sail $rev's (- sail, + ours):" >&2
	diff -u --label "sail $rev" --label ours <(echo "$theirs") <(echo "$ours") >&2 || true
	exit 1
fi
echo "sail-patch-check: [patch.crates-io] is sail $rev's ($(echo "$ours" | wc -l | tr -d ' ') entries)"
