#!/usr/bin/env bash
# Builds the tunnel host for the iOS packet tunnel extension (#448):
#
#   <out>/PPVPNTunnel.xcframework   libppvpn_tunnel.a per slice, with
#                                   include/ppvpn_tunnel.h as module PPVPNTunnel
#   <out>/PPVPNTunnel.ldflags       the system libraries it needs, as linker
#                                   flags for the extension (rustc's
#                                   native-static-libs), on one line
#
#   crates/tunnel-host/scripts/build-xcframework.sh <out> [slice...]
#
# Slices: ios (aarch64-apple-ios) and simulator (aarch64-apple-ios-sim),
# both by default. Profile `mobile` (Cargo.toml). BoringSSL is built from
# source: btls publishes no iOS prebuilt. On macOS with Xcode only.
set -euo pipefail

out=${1:?usage: $0 <out> [ios] [simulator]}
shift
slices=("$@")
[ ${#slices[@]} -gt 0 ] || slices=(ios simulator)
cd "$(dirname "$0")/../../.."
root=$(pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)

# The Flutter app's iOS minimum (apps/flutter/scripts/ci-build.sh).
export IPHONEOS_DEPLOYMENT_TARGET=15.0

headers=$(mktemp -d)
trap 'rm -rf "$headers"' EXIT
cp crates/tunnel-host/include/ppvpn_tunnel.h "$headers/"
cat >"$headers/module.modulemap" <<'MAP'
module PPVPNTunnel {
    header "ppvpn_tunnel.h"
    export *
}
MAP

args=()
ldflags=""
for slice in "${slices[@]}"; do
	case "$slice" in
	ios) triple=aarch64-apple-ios ;;
	simulator) triple=aarch64-apple-ios-sim ;;
	*) echo "unknown slice $slice (ios, simulator)" >&2; exit 2 ;;
	esac
	rustup target add "$triple" >/dev/null
	log="$out/build-$triple.log"
	# Without colour: CARGO_TERM_COLOR=always (ci.yml) would end the line
	# read below with an escape sequence, and the last flag with it.
	if ! cargo rustc --locked --color never --profile mobile -p ppvpn-tunnel-host --lib --crate-type staticlib \
		--target "$triple" -- --print native-static-libs 2>"$log"; then
		tail -50 "$log" >&2
		exit 1
	fi
	native=$(sed -n 's/.*native-static-libs: //p' "$log" | tail -1)
	[ -n "$native" ] || { tail -50 "$log" >&2; echo "rustc printed no native-static-libs" >&2; exit 1; }
	ldflags=$native
	lib="$root/target/$triple/mobile/libppvpn_tunnel.a"
	[ -f "$lib" ] || { echo "no $lib" >&2; exit 1; }
	# Every C function of the header is in the library. nm's list is read
	# from a file: grep -q ending a pipe early fails the pipe (pipefail).
	nm -gU "$lib" 2>/dev/null >"$headers/symbols" || true
	for symbol in $(sed -n 's/^[^#/ *].*\(ppvpn_tunnel_[a-z_]*\)(.*/\1/p' crates/tunnel-host/include/ppvpn_tunnel.h); do
		grep -q " _$symbol\$" "$headers/symbols" || { echo "$lib lacks $symbol; its ppvpn_tunnel symbols:" >&2; grep ppvpn_tunnel "$headers/symbols" >&2 || true; exit 1; }
	done
	rm -f "$headers/symbols"
	ls -l "$lib"
	args+=(-library "$lib" -headers "$headers")
done

rm -rf "$out/PPVPNTunnel.xcframework"
xcodebuild -create-xcframework "${args[@]}" -output "$out/PPVPNTunnel.xcframework"
printf '%s\n' "$ldflags" >"$out/PPVPNTunnel.ldflags"
echo "linker flags: $ldflags"
ls -R "$out/PPVPNTunnel.xcframework" | head -30
