#!/bin/sh
# Builds what the engine lab runs, into <out> (by hand on any Linux host with
# Go, Rust and the repository; no CI runs the lab):
#   <out>/ppvpn-core       the Go core 0.5.21, the release file (checked
#                          against the release's SHA256SUMS)
#   <out>/sing-box         sing-box 1.13.12, by its module version (checked
#                          against the Go checksum database)
#   <out>/sail             sail-cli at the sail commit Cargo.lock pins, for
#                          musl ($LAB_SAIL_TARGET, default <arch>-unknown-
#                          linux-musl): the nodes run on Alpine, whose gcompat
#                          lacks glibc symbols sail uses (__res_init)
#   <out>/ppvpn-core-lab   the Rust core behind Core API v1, for musl like
#                          sail-cli: it runs in the Alpine client
#   $LAB_WORK/rs-a.srs, rs-b.srs   the rule sets (lab.sh rules)
# The Go binaries are static (CGO_ENABLED=0). BoringSSL is linked as btls
# publishes it when BORING_BSSL_PATH_<target> is set (btls's
# prebuilt-boringssl.sh, for both targets), else compiled. sail-cli is built as sail builds
# its musl releases: with its own scripts at that commit
# (install_cross_toolchain.sh fetches a pinned, hash-checked musl-cross
# GCC with its libstdc++, which BoringSSL needs; cross.sh points cargo at
# it); x86_64 Linux hosts only.
#
#   test/lab/engine/ci-build.sh <out>
set -eu
OUT=$(mkdir -p "$1" && cd "$1" && pwd)
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
SING_BOX=v1.13.12
cd "$ROOT"
case $(uname -m) in x86_64) goarch=amd64 ;; aarch64|arm64) goarch=arm64 ;; *) echo "ci-build: $(uname -m)?" >&2; exit 1 ;; esac
rel=https://github.com/peakpassvpn/ppvpn-client/releases/download/v0.5.21
curl -fsSL --retry 3 -o "$OUT/ppvpn-core" "$rel/ppvpn-core-linux-$goarch"
(cd "$OUT" && curl -fsSL --retry 3 "$rel/SHA256SUMS" | awk -v f="ppvpn-core-linux-$goarch" '$2 == f || $2 == "*" f {print $1 "  ppvpn-core"}' | sha256sum -c -)
chmod +x "$OUT/ppvpn-core"
CGO_ENABLED=0 GOBIN="$OUT" go install -trimpath -tags with_gvisor,with_utls "github.com/sagernet/sing-box/cmd/sing-box@$SING_BOX"
rev=$(awk '/^name = "sail"$/ {f=1; next} f && /^source = / {sub(/.*#/, ""); sub(/"$/, ""); print; exit}' Cargo.lock)
[ ${#rev} -eq 40 ] || { echo "ci-build: no sail commit in Cargo.lock" >&2; exit 1; }
target=${LAB_SAIL_TARGET:-$(uname -m)-unknown-linux-musl}
# sail's scripts at that commit: its pinned musl toolchain, for both builds.
src=$(mktemp -d)
git clone --quiet --filter=blob:none --no-checkout https://github.com/peakpassvpn/sail.git "$src"
git -C "$src" checkout --quiet "$rev"
git -C "$src" submodule update --quiet --init --recursive
(cd "$src" && bash scripts/install_cross_toolchain.sh "$target" >/dev/null)
if [ ! -x "$OUT/sail-$rev/bin/sail" ]; then
	(cd "$src" && bash scripts/cross.sh "$target" build --release --locked --quiet -p sail-cli)
	mkdir -p "$OUT/sail-$rev/bin" && cp "$src/target/$target/release/sail" "$OUT/sail-$rev/bin/sail"
fi
cp "$OUT/sail-$rev/bin/sail" "$OUT/sail"
# A glibc build fails in Alpine the same way (gcompat lacks __res_init).
bash "$src/scripts/cross.sh" "$target" build --release --locked --quiet -p ppvpn-core-lab
cp "target/$target/release/ppvpn-core-lab" "$OUT/ppvpn-core-lab"
rm -rf "$src"
sh test/lab/engine/lab.sh rules
"$OUT/sail" --version 2>/dev/null | head -1 || true
"$OUT/sing-box" version | head -1
