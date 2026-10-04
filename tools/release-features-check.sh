#!/usr/bin/env bash
# Refuses a release build whose package graph turns on a test-only feature:
# sail's fault-injection (sail::fault, failures on purpose), ppvpn-core's
# fault-injection, testing (the in-memory runtime) and lab (test hooks).
# They reach a build through a --features flag or through any crate in the
# graph that names them; this asks Cargo which features the build resolves,
# for every target, without dev-dependencies (a test build's are allowed).
#
# Usage: run it before the build, with the build's own package and feature
# options (and --manifest-path for a crate outside the root workspace):
#   tools/release-features-check.sh -p ppvpn-client
#   tools/release-features-check.sh --manifest-path desktop/service/Cargo.toml
#   tools/release-features-check.sh -p ppvpn-core --features fault-injection   # fails
set -euo pipefail

forbidden='^(sail|ppvpn-core) feature "(fault-injection|testing|lab)"'
found=""
for crate in sail ppvpn-core; do
	if ! out=$(cargo tree --locked -e features,no-dev --target all --prefix none -i "$crate" "$@" 2>&1); then
		# Not in the graph at all: nothing of it is turned on.
		if grep -q "did not match any packages" <<<"$out"; then
			continue
		fi
		echo "$out" >&2
		echo "release-features-check: cargo tree failed" >&2
		exit 2
	fi
	hits=$(grep -E "$forbidden" <<<"$out" | sed 's/ (\*)$//' | sort -u || true)
	[ -z "$hits" ] || found+="$hits"$'\n'
done

if [ -n "$found" ]; then
	echo "release-features-check: a test-only feature is on in this build ($*):" >&2
	printf '%s' "$found" | sort -u >&2
	exit 1
fi
echo "release-features-check: no test-only feature in this build ($*)"
