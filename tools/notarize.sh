#!/usr/bin/env bash
# Notarizes one file with Apple's notary service (#471): the macOS app, DMG
# and pkg (apps/macos/scripts/package-app.sh) and the CLI's zip
# (tools/cli/build-release.sh).
#
#   tools/notarize.sh <file> <key.p8> <key id> <issuer id>
#
# Submits first and prints the submission id at once, then waits for the
# verdict: a job that runs out of time still leaves the id in its log, to
# look the submission up later (`xcrun notarytool info <id>` or `log <id>`
# with the same App Store Connect key). Exits 0 only when Accepted; on
# Invalid or Rejected it prints Apple's log.
#
# PPVPN_NOTARY_WAIT: how long to wait for the verdict (notarytool's
# --timeout, default 60m). Apple takes minutes as a rule, longer for a new
# team's first submissions; the release jobs' timeouts leave room for it.
set -euo pipefail

file=${1:?usage: $0 <file> <key.p8> <key id> <issuer id>}
key=(--key "${2:?}" --key-id "${3:?}" --issuer "${4:?}")
name=$(basename "$file")

field() { python3 -c 'import json, sys; print(json.loads(sys.stdin.read()).get(sys.argv[1], ""))' "$1" 2>/dev/null || true; }

out=$(xcrun notarytool submit "$file" "${key[@]}" --output-format json 2>&1) || true
id=$(field id <<<"$out")
if [ -z "$id" ]; then
	echo "error: notarytool submit $name: $out" >&2
	exit 1
fi
echo "notarization of $name submitted: $id"

out=$(xcrun notarytool wait "$id" "${key[@]}" --timeout "${PPVPN_NOTARY_WAIT:-60m}" --output-format json 2>&1) || true
status=$(field status <<<"$out")
case "$status" in
Accepted)
	echo "notarized $name ($id)"
	;;
Invalid | Rejected)
	echo "error: notarization of $name ($id): $status" >&2
	xcrun notarytool log "$id" "${key[@]}" >&2 || true
	exit 1
	;;
*)
	echo "error: notarization of $name ($id) has no verdict: ${status:-no answer}: $out" >&2
	echo "look it up later: xcrun notarytool info $id (same key)" >&2
	exit 1
	;;
esac
