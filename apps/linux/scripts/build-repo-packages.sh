#!/usr/bin/env bash
# Build the repository configuration packages:
#
#   dist/linux/ppvpn-archive-keyring_<version>_all.deb   apt key + /etc/apt/sources.list.d/ppvpn.list
#   dist/linux/ppvpn-release-<version>.noarch.rpm        dnf key + /etc/yum.repos.d/ppvpn.repo
#
# Usage: apps/linux/scripts/build-repo-packages.sh
#
# Environment:
#   PPVPN_REPO_KEY              ASCII-armored public key of the repository signing key
#                               (default: apps/linux/packaging/ppvpn.asc)
#   PPVPN_REPO_PACKAGE_VERSION  bump when the key, the source entry or anything else in the
#                               packages changes (default 1.0.1). The update site keeps every
#                               published file under its name for good and refuses to replace one
#                               with other bytes, so a version is built byte for byte the same
#                               every time: its timestamp is fixed (PACKAGE_TIME below).
#   PPVPN_PACKAGE_MAINTAINER    deb Maintainer / rpm Packager (default: PeakPass Labs LLC <support@peakpassvpn.com>)
#   PPVPN_DIST_DIR              output directory (default: dist/linux)
#
# Tools: nfpm, python3.
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$APP_DIR/../.." && pwd)"
OUT_DIR="${PPVPN_DIST_DIR:-$REPO_DIR/dist/linux}"
KEY="${PPVPN_REPO_KEY:-$APP_DIR/packaging/ppvpn.asc}"
VERSION="${PPVPN_REPO_PACKAGE_VERSION:-1.0.1}"
# The packages' timestamp (file times, archive headers, rpm build time): fixed per version so
# that each build of a version is identical. 1.0.0 was built by an older pipeline with the build
# time, and its maintainer spelled differently; 1.0.1 has the same key and source entry.
case "$VERSION" in
  1.0.1) PACKAGE_TIME="2026-10-06T00:00:00Z" ;;
  *) echo "error: no fixed timestamp for repository package version $VERSION (add one)" >&2; exit 1 ;;
esac
export SOURCE_DATE_EPOCH="$(date -u -d "$PACKAGE_TIME" +%s 2>/dev/null || date -u -j -f '%Y-%m-%dT%H:%M:%SZ' "$PACKAGE_TIME" +%s)"
[[ -f "$KEY" ]] || { echo "error: missing repository public key $KEY (set PPVPN_REPO_KEY)" >&2; exit 1; }
STAGE="$(mktemp -d)"
trap 'rm -rf "$STAGE"' EXIT

# apt's signed-by wants a binary keyring: de-armor the key, as `gpg --dearmor` does
# (the armor is just base64 plus a CRC line), so the build needs no gpg.
python3 - "$KEY" "$STAGE/ppvpn-archive-keyring.gpg" <<'PY'
import base64, sys
lines = open(sys.argv[1], encoding="ascii").read().splitlines()
if "-----BEGIN PGP PUBLIC KEY BLOCK-----" not in lines:
    sys.exit(f"error: {sys.argv[1]} is not an ASCII-armored public key")
start = lines.index("-----BEGIN PGP PUBLIC KEY BLOCK-----") + 1
end = lines.index("-----END PGP PUBLIC KEY BLOCK-----")
body = lines[start:end]
# Skip armor headers (up to the first blank line) and the "=XXXX" CRC line.
if "" in body:
    body = body[body.index("") + 1:]
data = base64.b64decode("".join(line for line in body if line and not line.startswith("=")))
# The "=XXXX" line is a CRC-24 (RFC 4880 6.1) of the data: check it.
crc = 0xB704CE
for byte in data:
    crc ^= byte << 16
    for _ in range(8):
        crc <<= 1
        if crc & 0x1000000:
            crc ^= 0x1864CFB
checksum = [line for line in body if line.startswith("=")]
if checksum and base64.b64decode(checksum[0][1:]) != (crc & 0xFFFFFF).to_bytes(3, "big"):
    sys.exit("error: the armored key's checksum does not match; the file is damaged")
open(sys.argv[2], "wb").write(data)
PY

# nfpm leaves ${VAR} in contents paths alone: expand every ${VAR} ourselves (unset = error).
expand_config() {
  python3 - "$1" "$2" <<'PY'
import os, re, sys
text = open(sys.argv[1]).read()
def value(match):
    name = match.group(1)
    if name not in os.environ:
        sys.exit(f"error: {sys.argv[1]} uses ${{{name}}}, which is not set")
    return os.environ[name]
open(sys.argv[2], "w").write(re.sub(r"\$\{(\w+)\}", value, text))
PY
}

mkdir -p "$OUT_DIR"
export PPVPN_REPO_PACKAGE_VERSION="$VERSION" PPVPN_REPO_KEY="$KEY" PPVPN_REPO_STAGE="$STAGE"
export PPVPN_REPO_PACKAGE_TIME="$PACKAGE_TIME"
export PPVPN_REPO_DIR="$APP_DIR/packaging"
export PPVPN_PACKAGE_MAINTAINER="${PPVPN_PACKAGE_MAINTAINER:-PeakPass Labs LLC <support@peakpassvpn.com>}"
PPVPN_REPO_PACKAGE=ppvpn-archive-keyring expand_config "$APP_DIR/packaging/nfpm-keyring.yaml" "$STAGE/keyring-deb.yaml"
PPVPN_REPO_PACKAGE=ppvpn-release expand_config "$APP_DIR/packaging/nfpm-keyring.yaml" "$STAGE/keyring-rpm.yaml"
nfpm package --config "$STAGE/keyring-deb.yaml" --packager deb --target "$OUT_DIR/ppvpn-archive-keyring_${VERSION}_all.deb"
nfpm package --config "$STAGE/keyring-rpm.yaml" --packager rpm --target "$OUT_DIR/ppvpn-release-${VERSION}.noarch.rpm"
