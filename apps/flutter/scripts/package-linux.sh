#!/usr/bin/env bash
# Builds the PPVPN deb and rpm (x86_64) with the Flutter app (#401), the
# same files the native Linux release makes (apps/linux/scripts/build-package.sh):
#
#   <dist>/PPVPN-<version>-linux-x64.{deb,rpm}
#   <dist>/release-meta-linux-x64-{deb,rpm}.json   (schema 1; build is
#                                                   <version>.<build number>)
#
# Usage: apps/flutter/scripts/package-linux.sh <bundle>
#   <bundle>  build/linux/x64/release/bundle from scripts/ci-build.sh, built
#             with the same PPVPN_VERSION and PPVPN_BUILD_NUMBER (checked
#             against its flutter_assets/version.json)
#
# Builds the service (profile "service", as the native packages) and the
# push agent (apps/flutter/push-agent) itself, stages them with the bundle in
# the native layout (../packaging/linux/nfpm.yaml) and runs nfpm.
#
# Tools: cargo, nfpm, python3, readelf, strip.
#
# Environment:
#   PPVPN_VERSION             x.y.z (default: the workspace's, Cargo.toml)
#   PPVPN_BUILD_NUMBER        the release's counter (default 0): the package release
#   PPVPN_RELEASE_CHANNEL     dev | stable, recorded in release-meta (empty: null, not publishable)
#   PPVPN_PACKAGE_MAINTAINER  deb Maintainer / rpm Packager (default: PeakPass Labs LLC <support@peakpassvpn.com>)
#   PPVPN_DIST_DIR            output directory (default: dist/linux)
#   PPVPN_GLIBC_FLOOR         the newest glibc symbol version a binary may need (default
#                             2.35: Ubuntu 22.04, which the release builds on); empty only
#                             reports it (ci.yml builds on a newer runner)
set -euo pipefail

# Linux ships for x86_64 only.
ARCH=x64 NFPM_ARCH=amd64

app=$(cd "$(dirname "$0")/.." && pwd)
root=$(cd "$app/../.." && pwd)
bundle=$(cd "${1:?usage: package-linux.sh <bundle>}" && pwd)
out=${PPVPN_DIST_DIR:-$root/dist/linux}
glibc=${PPVPN_GLIBC_FLOOR-2.35}
work=$(mktemp -d)
trap 'rm -rf "$work"' EXIT

version=${PPVPN_VERSION:-$(sed -n '/^\[workspace.package\]/,/^\[/s/^version = "\(.*\)"$/\1/p' "$root/Cargo.toml")}
build=${PPVPN_BUILD_NUMBER:-0}
channel=${PPVPN_RELEASE_CHANNEL:-}
[[ $version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: version must be x.y.z, got '$version'" >&2; exit 2; }
[[ $build =~ ^[0-9]+$ ]] || { echo "error: build number must be an integer, got '$build'" >&2; exit 2; }
case "$channel" in '' | dev | stable) ;; *) echo "error: channel must be dev or stable" >&2; exit 2 ;; esac

for path in "$bundle/ppvpn" "$bundle/lib/libflutter_linux_gtk.so" "$bundle/lib/libapp.so" \
    "$bundle/lib/libppvpn_flutter.so" "$bundle/data/icudtl.dat" "$bundle/data/flutter_assets"; do
  [ -e "$path" ] || { echo "error: $path is missing" >&2; exit 1; }
done
# The version the bundle was built with (flutter build --build-name/--build-number).
python3 - "$bundle/data/flutter_assets/version.json" "$version" "$build" <<'PY'
import json, sys
path, version, build = sys.argv[1:]
stamped = json.load(open(path))
if (stamped.get("version"), str(stamped.get("build_number"))) != (version, build):
    sys.exit(f"error: the bundle is {stamped.get('version')}+{stamped.get('build_number')}, not {version}+{build}")
PY
echo "PPVPN $version (build $build) for linux-$ARCH, channel ${channel:-none}"

# --- 1. service and push agent --------------------------------------------------------
(cd "$root" && cargo build --locked -p ppvpn-service --profile service --bins)
(cd "$root" && cargo build --locked -p ppvpn-push-agent --release)
target=$(cd "$root" && cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')

# --- 2. staging tree --------------------------------------------------------------------
stage=$work/stage
mkdir -p "$stage/app"
cp -R "$bundle/." "$stage/app/"
# The bridge as ci-build.sh copied it from target/release: its symbol table goes,
# its exports (the bridge's entry points) stay.
strip --strip-unneeded "$stage/app/lib/libppvpn_flutter.so"
install -m 0755 -s "$target/release/ppvpn-push-agent" "$stage/app/ppvpn-push-agent"
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  install -m 0755 "$target/service/$bin" "$stage/app/$bin"
done
chmod -R u+rwX,go+rX,go-w "$stage/app"
# GPL-3.0-or-later: the license text, installed where each package format keeps it (nfpm.yaml).
install -m 0644 "$root/LICENSE" "$stage/LICENSE"
printf 'deb\n' >"$stage/package-format.deb"
printf 'rpm\n' >"$stage/package-format.rpm"
for entry in 32x32:32x32.png 128x128:128x128.png 256x256:128x128@2x.png; do
  size=${entry%%:*}
  mkdir -p "$stage/icons/$size/apps"
  install -m 0644 "$root/apps/assets/icons/${entry#*:}" "$stage/icons/$size/apps/com.peakpassvpn.ppvpn.desktop.png"
done

# The glibc floor: the newest symbol version any binary needs.
max_glibc() { readelf -W --dyn-syms "$1" | { grep -o 'GLIBC_[0-9.]*' || true; } | sed 's/GLIBC_//' | sort -Vu | tail -1; }
newest=
while IFS= read -r -d '' binary; do
  head -c 4 "$binary" | grep -q 'ELF' || continue
  needed=$(max_glibc "$binary")
  [ -n "$needed" ] || continue
  echo "  ${binary#"$stage/app/"}: GLIBC_$needed"
  newest=$(printf '%s\n%s\n' "$needed" "$newest" | sort -V | tail -1)
  if [ -n "$glibc" ] && [ "$(printf '%s\n%s\n' "$needed" "$glibc" | sort -V | tail -1)" != "$glibc" ]; then
    echo "error: ${binary#"$stage/app/"} needs GLIBC_$needed, above $glibc" >&2
    exit 1
  fi
done < <(find "$stage/app" -type f \( -perm -u+x -o -name '*.so*' \) -print0)
echo "newest glibc symbol: GLIBC_$newest (floor: ${glibc:-not checked})"

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

# --- 3. deb + rpm -----------------------------------------------------------------------
mkdir -p "$out"
export NFPM_ARCH PPVPN_VERSION="$version" PPVPN_BUILD_NUMBER="$build"
# The native package's maintainer scripts and push agent entry; the app's own desktop entry.
export PPVPN_STAGE_DIR="$stage" PPVPN_PACKAGING_DIR="$root/apps/linux/packaging"
export PPVPN_FLUTTER_PACKAGING_DIR="$app/packaging/linux"
export PPVPN_PACKAGE_MAINTAINER="${PPVPN_PACKAGE_MAINTAINER:-PeakPass Labs LLC <support@peakpassvpn.com>}"
expand_config "$app/packaging/linux/nfpm.yaml" "$work/nfpm.yaml"
packages=()
for format in deb rpm; do
  package="$out/PPVPN-$version-linux-$ARCH.$format"
  rm -f "$package"
  nfpm package --config "$work/nfpm.yaml" --packager "$format" --target "$package"
  packages+=("$package")
done

# --- 4. release metadata ----------------------------------------------------------------
# As the native packages': updates go through the GPG-signed apt/dnf repositories, the
# packages carry no EdDSA signature (ed_signature null).
for package in "${packages[@]}"; do
  format=${package##*.}
  meta="$out/release-meta-linux-$ARCH-$format.json"
  python3 - "$meta" "$package" "linux-$ARCH-$format" "$channel" "$version" "$version.$build" <<'PY'
import datetime, hashlib, json, os, sys
meta, package, platform, channel, version, build = sys.argv[1:]
with open(package, "rb") as handle:
    digest = hashlib.sha256(handle.read()).hexdigest()
json.dump({
    "schema": 1,
    "platform": platform,
    "channel": channel or None,
    "version": version,
    "build": build,
    "file": os.path.basename(package),
    "length": os.path.getsize(package),
    "sha256": digest,
    "ed_signature": None,
    # Ubuntu 22.04+ / Debian 12+ (deb), Fedora 36+ (rpm), as the native packages.
    "min_os": "fedora-36" if package.endswith(".rpm") else "ubuntu-22.04",
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
}, open(meta, "w"), indent=2)
PY
  echo "wrote $(basename "$package") and $(basename "$meta")"
done
