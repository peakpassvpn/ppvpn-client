#!/bin/sh
# Builds what the engine lab runs, into <out> (CI: lab.yml; any Linux host
# with Go, Rust and the repository works the same):
#   <out>/ppvpn-core       the Go core 0.5.21 (make build-desktop)
#   <out>/sing-box         sing-box 1.13.12, by its module version (checked
#                          against the Go checksum database)
#   <out>/sail             sail-cli at the sail commit Cargo.lock pins
#   <out>/ppvpn-core-lab   the Rust core behind Core API v1
#   $LAB_WORK/rs-a.srs, rs-b.srs   the rule sets (lab.sh rules)
# BoringSSL is linked as btls publishes it when BORING_BSSL_PATH_* is set
# (rust.yml's prebuilt step), else compiled.
#
#   test/lab/engine/ci-build.sh <out>
set -eu
OUT=$(mkdir -p "$1" && cd "$1" && pwd)
ROOT=$(cd "$(dirname "$0")/../../.." && pwd)
SING_BOX=v1.13.12
cd "$ROOT"
make build-desktop >/dev/null && cp build/ppvpn-core "$OUT/ppvpn-core"
GOBIN="$OUT" go install -trimpath -tags with_gvisor,with_utls "github.com/sagernet/sing-box/cmd/sing-box@$SING_BOX"
rev=$(awk '/^name = "sail"$/ {f=1; next} f && /^source = / {sub(/.*#/, ""); sub(/"$/, ""); print; exit}' Cargo.lock)
[ ${#rev} -eq 40 ] || { echo "ci-build: no sail commit in Cargo.lock" >&2; exit 1; }
cargo install --locked --quiet --git https://github.com/peakpassvpn/sail.git --rev "$rev" --root "$OUT/sail-$rev" sail-cli
cp "$OUT/sail-$rev/bin/sail" "$OUT/sail"
cargo build --release --locked --quiet -p ppvpn-core-lab && cp target/release/ppvpn-core-lab "$OUT/ppvpn-core-lab"
sh test/lab/engine/lab.sh rules
"$OUT/sail" --version 2>/dev/null | head -1 || true
"$OUT/sing-box" version | head -1
