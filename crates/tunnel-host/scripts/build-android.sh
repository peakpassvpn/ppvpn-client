#!/usr/bin/env bash
# Builds the tunnel host for Android's VpnService (#448):
#
#   <out>/<abi>/libppvpn_tunnel.so   for the app's jniLibs, with the NDK's
#                                    libc++_shared.so beside it when a C++
#                                    dependency needs it
#
#   crates/tunnel-host/scripts/build-android.sh <out> [abi...]
#
# ABIs: arm64-v8a (phones) and x86_64 (emulators), both by default. Profile
# `mobile` (Cargo.toml), linked by the NDK's clang for API 26 (the app's
# minSdk, apps/flutter/scripts/ci-build.sh). Needs ANDROID_NDK_HOME.
# PPVPN_TUNNEL_CLIPPY=1 runs clippy for each ABI's target first (CI: the
# JNI code compiles for Android only).
set -euo pipefail

out=${1:?usage: $0 <out> [arm64-v8a] [x86_64]}
shift
abis=("$@")
[ ${#abis[@]} -gt 0 ] || abis=(arm64-v8a x86_64)
cd "$(dirname "$0")/../../.."
root=$(pwd)
mkdir -p "$out"
out=$(cd "$out" && pwd)

ndk=${ANDROID_NDK_HOME:?ANDROID_NDK_HOME must name the Android NDK}
case "$(uname -s)" in
Linux) host=linux-x86_64 ;;
Darwin) host=darwin-x86_64 ;;
*) echo "unsupported build host $(uname -s)" >&2; exit 2 ;;
esac
bin=$ndk/toolchains/llvm/prebuilt/$host/bin
api=26

for abi in "${abis[@]}"; do
	case "$abi" in
	arm64-v8a) triple=aarch64-linux-android ;;
	x86_64) triple=x86_64-linux-android ;;
	*) echo "unknown ABI $abi (arm64-v8a, x86_64)" >&2; exit 2 ;;
	esac
	rustup target add "$triple" >/dev/null
	upper=$(echo "$triple" | tr 'a-z-' 'A-Z_')
	ndk_env=("CARGO_TARGET_${upper}_LINKER=$bin/${triple}${api}-clang"
		"CC_${triple//-/_}=$bin/${triple}${api}-clang" "AR_${triple//-/_}=$bin/llvm-ar"
		"BINDGEN_EXTRA_CLANG_ARGS_${triple//-/_}=--target=${triple}${api}")
	if [ "${PPVPN_TUNNEL_CLIPPY:-}" = 1 ]; then
		env "${ndk_env[@]}" cargo clippy --locked -p ppvpn-tunnel-host --target "$triple" -- -D warnings
	fi
	env "${ndk_env[@]}" cargo rustc --locked --profile mobile -p ppvpn-tunnel-host --lib \
		--crate-type cdylib --target "$triple"
	so="$root/target/$triple/mobile/libppvpn_tunnel.so"
	[ -f "$so" ] || { echo "no $so" >&2; exit 1; }
	mkdir -p "$out/$abi"
	"$bin/llvm-strip" --strip-unneeded -o "$out/$abi/libppvpn_tunnel.so" "$so"
	# The VM's entry and every native method of TunnelHost are exported.
	# nm's list is read from a file: grep -q ending a pipe early fails the
	# pipe (pipefail).
	symbols=$(mktemp)
	"$bin/llvm-nm" -D --defined-only "$out/$abi/libppvpn_tunnel.so" >"$symbols"
	for symbol in JNI_OnLoad settings start stop networkChanged sleep wake version; do
		[ "$symbol" = JNI_OnLoad ] || symbol=Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_$symbol
		grep -q " T $symbol\$" "$symbols" || { echo "$abi: libppvpn_tunnel.so lacks $symbol" >&2; rm -f "$symbols"; exit 1; }
	done
	rm -f "$symbols"
	if "$bin/llvm-readelf" -d "$so" | grep -F 'libc++_shared.so' >/dev/null; then
		cp "$ndk/toolchains/llvm/prebuilt/$host/sysroot/usr/lib/$triple/libc++_shared.so" "$out/$abi/"
	fi
	ls -l "$out/$abi"
done
