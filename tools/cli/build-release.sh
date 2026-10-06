#!/usr/bin/env bash
# Builds the CLI's release archive for one platform into dist/cli/:
#
#   ppvpn-cli-<version>-<platform>.tar.gz
#     ppvpn-cli-<version>-<platform>/ppvpn, README.md (crates/cli/README.md), LICENSE
#   ppvpn-cli-<version>-<platform>.symbols.tar.gz
#     ppvpn-cli-<version>-<platform>.symbols/ppvpn.debug (Linux) or ppvpn.dSYM (macOS)
#
# The shipped binary has no symbols; its symbols go into the second archive,
# which turns a backtrace's addresses into function names (docs/cli.md).
#
#   tools/cli/build-release.sh linux x86_64-unknown-linux-musl     # linux-x86_64
#   tools/cli/build-release.sh linux aarch64-unknown-linux-musl    # linux-aarch64
#   tools/cli/build-release.sh macos                               # macos-universal
#
# Linux builds are static (musl, with mimalloc: crates/cli/Cargo.toml), made
# on an x86_64 Linux host as sail builds its musl releases: with sail's own
# scripts at the commit Cargo.lock pins (a pinned, hash-checked musl-cross
# GCC; BoringSSL as the prebuilt-boringssl action sets it up). The macOS
# build is one universal binary of aarch64 and x86_64, made on macOS.
#
# Environment:
#   CHANNEL       empty (a merge: the crate's version, production API), dev or stable
#   VERSION       x.y.z of a dev or stable run. stable: must be the crate's
#                 version (Cargo.toml), or the build fails
#   DEV_API_BASE  dev: the dev backend, compiled in as the default origin
#                 (PPVPN_BUILD_PROFILE=dev); a dev archive is never published
#   MACOS_SIGNING_P12_BASE64, MACOS_SIGNING_P12_PASSWORD
#                 macOS: the "PPVPN Code Signing" certificate and key, as the
#                 desktop app is signed; without them ad-hoc (refused for stable)
set -euo pipefail

os=${1:?usage: $0 linux <target> | macos}
cd "$(dirname "$0")/../.."
root=$(pwd)
work=$(mktemp -d)

fail() {
	echo "build-release: $*" >&2
	exit 1
}

crate=$(cargo metadata --locked --no-deps --format-version 1 |
	jq -r '.packages[] | select(.name == "ppvpn-cli") | .version')
[ -n "$crate" ] || fail "no ppvpn-cli in cargo metadata"

# What the binary is built as: release builds take no profile, origin or
# version from the environment.
unset PPVPN_BUILD_PROFILE PPVPN_DEFAULT_API_BASE PPVPN_VERSION
case "${CHANNEL:-}" in
"")
	version=$crate
	name_version=$version
	;;
stable)
	[ "${VERSION:-}" = "$crate" ] || fail "the release's version ${VERSION:-(none)} is not the CLI's $crate (Cargo.toml)"
	version=$crate
	name_version=$version
	;;
dev)
	[ -n "${VERSION:-}" ] || fail "a dev build needs VERSION"
	[ -n "${DEV_API_BASE:-}" ] || fail "a dev build needs DEV_API_BASE"
	version=$VERSION
	name_version=$version-dev
	export PPVPN_BUILD_PROFILE=dev PPVPN_DEFAULT_API_BASE=$DEV_API_BASE PPVPN_VERSION=$version
	;;
*) fail "unknown channel $CHANNEL" ;;
esac

case $os in
linux)
	target=${2:?usage: $0 linux <target>}
	case $target in
	x86_64-unknown-linux-musl) platform=linux-x86_64 ;;
	aarch64-unknown-linux-musl) platform=linux-aarch64 ;;
	*) fail "unsupported target $target" ;;
	esac
	rev=$(awk '/^name = "sail"$/ {f=1; next} f && /^source = / {sub(/.*#/, ""); sub(/"$/, ""); print; exit}' Cargo.lock)
	[ ${#rev} -eq 40 ] || fail "no sail commit in Cargo.lock"
	sail=$(mktemp -d)
	git clone --quiet --filter=blob:none --no-checkout https://github.com/peakpassvpn/sail.git "$sail"
	git -C "$sail" checkout --quiet "$rev"
	(cd "$sail" && bash scripts/install_cross_toolchain.sh "$target" >/dev/null)
	bash "$sail/scripts/cross.sh" "$target" build --release --locked -p ppvpn-cli
	rm -rf "$sail"
	# The toolchain's binutils know the target's ELF.
	tools=$(echo "${SAIL_CROSS_DIR:-$HOME/.sail-cross}"/musl-*/"$target"/bin)
	[ -x "$tools/$target-objcopy" ] || fail "no $target-objcopy in the cross toolchain"
	symbols=$work/ppvpn.debug
	bin=$work/ppvpn
	"$tools/$target-objcopy" --only-keep-debug "$root/target/$target/release/ppvpn" "$symbols"
	"$tools/$target-objcopy" --strip-all --add-gnu-debuglink="$symbols" "$root/target/$target/release/ppvpn" "$bin"
	"$tools/$target-nm" "$symbols" | grep 'ppvpn_cli8commands3run' >/dev/null || fail "ppvpn.debug lacks the CLI's symbols"
	! "$tools/$target-nm" "$bin" 2>/dev/null | grep . >/dev/null || fail "the shipped ppvpn still has symbols"
	# Static: it runs on any Linux of its architecture, glibc or not.
	info=$(file -b "$bin")
	echo "$info"
	grep -Eq 'static(ally|-pie) linked' <<<"$info" || fail "ppvpn is not static: $info"
	case $target in
	x86_64-*)
		grep -q 'x86-64' <<<"$info" || fail "not an x86-64 binary: $info"
		reported=$("$bin" version)
		echo "$reported"
		[ "$reported" = "ppvpn $version" ] || fail "ppvpn reports '$reported', not $version"
		;;
	aarch64-*) grep -q 'ARM aarch64' <<<"$info" || fail "not an aarch64 binary: $info" ;;
	esac
	;;
macos)
	platform=macos-universal
	for target in aarch64-apple-darwin x86_64-apple-darwin; do
		cargo build --release --locked -p ppvpn-cli --target "$target"
	done
	bin=$root/target/universal-apple-darwin/ppvpn
	mkdir -p "$(dirname "$bin")"
	lipo -create -output "$bin" \
		target/aarch64-apple-darwin/release/ppvpn target/x86_64-apple-darwin/release/ppvpn
	lipo "$bin" -verify_arch arm64 x86_64
	# Symbols out before signing: a dSYM (the symbol tables of both slices;
	# release builds carry no DWARF, which dsymutil warns about), then strip.
	symbols=$work/ppvpn.dSYM
	dsymutil "$bin" -o "$symbols" 2>&1 | grep -v 'no debug symbols in executable' || true
	for arch in arm64 x86_64; do
		nm -arch "$arch" "$symbols/Contents/Resources/DWARF/ppvpn" | grep 'ppvpn_cli8commands3run' >/dev/null \
			|| fail "the dSYM lacks the CLI's symbols ($arch)"
	done
	strip "$bin"
	! nm -arch arm64 "$bin" 2>/dev/null | grep 'ppvpn_cli8commands3run' >/dev/null || fail "the shipped ppvpn still has symbols"
	# The desktop app's certificate (tools/desktop/ci/build-macos-native.sh):
	# self-signed, so the designated requirement is pinned to the identifier
	# and the certificate, and the Keychain item's access carries across
	# updates. A temporary keychain holds it for this build only.
	identifier=com.peakpassvpn.ppvpn.cli
	if [ -n "${MACOS_SIGNING_P12_BASE64:-}" ]; then
		: "${MACOS_SIGNING_P12_PASSWORD:?MACOS_SIGNING_P12_PASSWORD is required with MACOS_SIGNING_P12_BASE64}"
		identity="PPVPN Code Signing"
		key_dir=$(mktemp -d)
		keychain=$key_dir/signing.keychain-db
		keychain_password=$(uuidgen)
		original_keychains=()
		while IFS= read -r line; do original_keychains+=("$(echo "$line" | tr -d ' "')"); done \
			< <(security list-keychains -d user)
		cleanup() {
			security list-keychains -d user -s ${original_keychains[@]+"${original_keychains[@]}"} || true
			security delete-keychain "$keychain" || true
			rm -rf "$key_dir"
		}
		trap cleanup EXIT
		security create-keychain -p "$keychain_password" "$keychain"
		security set-keychain-settings -lut 21600 "$keychain"
		security unlock-keychain -p "$keychain_password" "$keychain"
		printf '%s' "$MACOS_SIGNING_P12_BASE64" | base64 --decode >"$key_dir/signing.p12"
		security import "$key_dir/signing.p12" -k "$keychain" -P "$MACOS_SIGNING_P12_PASSWORD" -T /usr/bin/codesign >/dev/null
		rm -f "$key_dir/signing.p12"
		security set-key-partition-list -S apple-tool:,apple:,codesign: -s -k "$keychain_password" "$keychain" >/dev/null
		security list-keychains -d user -s "$keychain" ${original_keychains[@]+"${original_keychains[@]}"}
		cert_sha1=$(security find-certificate -c "$identity" -Z "$keychain" | awk '/^SHA-1 hash:/ { print $NF; exit }')
		[ -n "$cert_sha1" ] || fail "no certificate for \"$identity\" in the signing keychain"
		codesign --force --sign "$identity" --identifier "$identifier" --options runtime --timestamp=none \
			--requirements "=designated => identifier \"$identifier\" and certificate leaf = H\"$cert_sha1\"" "$bin"
		codesign -dv --verbose=4 "$bin" 2>&1 | grep -qx "Authority=$identity" || fail "ppvpn is not signed by $identity"
	else
		[ "${CHANNEL:-}" != stable ] || fail "a stable build must be signed (MACOS_SIGNING_P12_BASE64)"
		echo "warning: MACOS_SIGNING_P12_BASE64 not set; signing ad-hoc (not for release)"
		codesign --force --sign - --identifier "$identifier" --options runtime "$bin"
	fi
	codesign --verify --strict --verbose=2 "$bin"
	reported=$("$bin" version)
	echo "$reported"
	[ "$reported" = "ppvpn $version" ] || fail "ppvpn reports '$reported', not $version"
	;;
*) fail "unknown os $os" ;;
esac

# pack <directory under $stage>: dist/cli/<directory>.tar.gz
pack() {
	if [ "$os" = linux ]; then
		# GNU tar: sorted names, no owner, the commit's time.
		tar -C "$stage" --sort=name --owner=0 --group=0 --numeric-owner --mtime="@$(git log -1 --format=%ct)" \
			-cf - "$1" | gzip -n >"dist/cli/$1.tar.gz"
	else
		# bsdtar: no AppleDouble (._*) files.
		COPYFILE_DISABLE=1 tar -C "$stage" --uid 0 --gid 0 --uname root --gname root -cf - "$1" |
			gzip -n >"dist/cli/$1.tar.gz"
	fi
	tar -tzvf "dist/cli/$1.tar.gz" | head -20
}

name=ppvpn-cli-$name_version-$platform
stage=$(mktemp -d)
mkdir -p dist/cli "$stage/$name" "$stage/$name.symbols"
cp "$bin" "$stage/$name/ppvpn"
chmod 755 "$stage/$name/ppvpn"
cp crates/cli/README.md "$stage/$name/README.md"
cp LICENSE "$stage/$name/LICENSE"
cp -R "$symbols" "$stage/$name.symbols/"
pack "$name"
pack "$name.symbols"
rm -rf "$stage" "$work"
ls -l dist/cli
