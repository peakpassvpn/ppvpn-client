#!/usr/bin/env bash
# Build the PPVPN deb and rpm (x86_64 only) plus their release metadata:
#
#   dist/linux/PPVPN-<version>-linux-x64.{deb,rpm}
#   dist/linux/release-meta-linux-x64-{deb,rpm}.json   (schema 1, as macOS/Windows;
#                                                       build is <version>.<build number>)
#
# Usage: apps/linux/scripts/build-package.sh
#
# Everything native is linked against glibc 2.35 (Ubuntu 22.04) with cargo-zigbuild,
# so the packages run on Ubuntu 22.04+, Debian 12+ and Fedora 36+. The NativeAOT push
# agent links against the build host's glibc: build on Ubuntu 22.04 (checked below).
#
# Tools: rustup + cargo, zig + cargo-zigbuild, uniffi-bindgen-cs (see
# crates/ppvpn-client/scripts/build-dotnet.sh), .NET 8 SDK, nfpm, python3.
#
# Environment:
#   PPVPN_VERSION             x.y.z (default: the project's <Version>)
#   PPVPN_BUILD_NUMBER        monotonic release counter (default 0); the app compares
#                             <version>.<build number> with the repository's latest.json
#   PPVPN_RELEASE_CHANNEL     dev | stable, recorded in release-meta (empty: null, not publishable)
#   PPVPN_API_BASE            backend baked into the app (default https://www.peakpassvpn.com)
#   PPVPN_UPDATE_FEED         latest.json the app polls for the update notice
#                             (default https://pkg.peakpassvpn.com/linux/<channel or stable>/latest.json)
#   PPVPN_CORE_DIR            directory with ppvpn-core-linux-amd64 (default: the vendored
#                             core, vendor/ppvpn-core/<CURRENT>, verified against its manifest)
#   PPVPN_PACKAGE_MAINTAINER  deb Maintainer / rpm Packager (default: PeakPass VPN LLC <support@peakpassvpn.com>)
#   PPVPN_DIST_DIR            output directory (default: dist/linux)
set -euo pipefail

# Linux ships for x86_64 only.
ARCH=x64 TRIPLE=x86_64-unknown-linux-gnu RID=linux-x64 NFPM_ARCH=amd64 CORE_ARCH=amd64
GLIBC=2.35

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
APP_DIR="$(cd "$SCRIPT_DIR/.." && pwd)"
REPO_DIR="$(cd "$APP_DIR/../.." && pwd)"
PROJECT="$APP_DIR/PPVPN.Linux.csproj"
OUT_DIR="${PPVPN_DIST_DIR:-$REPO_DIR/dist/linux}"
WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

VERSION="${PPVPN_VERSION:-$(dotnet msbuild "$PROJECT" -nologo -getProperty:Version)}"
BUILD="${PPVPN_BUILD_NUMBER:-0}"
CHANNEL="${PPVPN_RELEASE_CHANNEL:-}"
API_BASE="${PPVPN_API_BASE:-https://www.peakpassvpn.com}"
UPDATE_FEED="${PPVPN_UPDATE_FEED:-https://pkg.peakpassvpn.com/linux/${CHANNEL:-stable}/latest.json}"
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || { echo "error: version must be x.y.z, got '$VERSION'" >&2; exit 2; }
[[ "$BUILD" =~ ^[0-9]+$ ]] || { echo "error: build number must be an integer, got '$BUILD'" >&2; exit 2; }
case "$CHANNEL" in ''|dev|stable) ;; *) echo "error: channel must be dev or stable" >&2; exit 2 ;; esac
echo "PPVPN $VERSION (build $BUILD) for $RID, channel ${CHANNEL:-none}, api $API_BASE"

# --- 1. ppvpn-client: native library + C# bindings -----------------------------------
# With zigbuild, build-dotnet.sh generates the bindings from a native host build (zig's
# linker drops UniFFI's metadata) and checks the zig library exports every symbol they call.
PPVPN_CARGO_BUILD="cargo zigbuild" "$REPO_DIR/crates/ppvpn-client/scripts/build-dotnet.sh" "$TRIPLE.$GLIBC" --release

# --- 2. privileged service -----------------------------------------------------------
rustup target add "$TRIPLE" >/dev/null
(cd "$REPO_DIR/service" && cargo zigbuild --release --locked --bins --target "$TRIPLE.$GLIBC")
SERVICE_OUT="$(cd "$REPO_DIR/service" && cargo metadata --format-version 1 --no-deps \
  | python3 -c 'import json,sys; print(json.load(sys.stdin)["target_directory"])')/$TRIPLE/release"

# --- 3. ppvpn-core (static Go binary) -------------------------------------------------
# The vendored build, checked against its manifest (artifact linux-x86_64); PPVPN_CORE_DIR
# overrides it for local core builds.
if [[ -n "${PPVPN_CORE_DIR:-}" ]]; then
  CORE="$PPVPN_CORE_DIR/ppvpn-core-linux-$CORE_ARCH"
else
  CORE_VERSION="$(tr -d '[:space:]' < "$REPO_DIR/vendor/ppvpn-core/CURRENT")"
  VENDOR_DIR="$REPO_DIR/vendor/ppvpn-core/$CORE_VERSION"
  if command -v node >/dev/null 2>&1; then
    node "$REPO_DIR/scripts/verify-vendored-core.mjs" --vendor-dir "$VENDOR_DIR" \
      --artifact linux-x86_64 --expected-version "$CORE_VERSION"
  else
    python3 - "$VENDOR_DIR" linux-x86_64 "$CORE_VERSION" <<'PY'
import hashlib, json, os, sys
vendor, key, version = sys.argv[1:]
manifest = json.load(open(os.path.join(vendor, "manifest.json")))
if str(manifest.get("version", version)).lstrip("v") != version:
    sys.exit(f"error: manifest is for {manifest.get('version')}, not {version}")
artifact = manifest["artifacts"][key]
path = os.path.realpath(os.path.join(vendor, artifact["path"]))
if not path.startswith(os.path.realpath(vendor) + os.sep):
    sys.exit(f"error: artifact {key} escapes the vendor directory")
digest = hashlib.sha256(open(path, "rb").read()).hexdigest()
if digest != artifact["sha256"]:
    sys.exit(f"error: {artifact['path']} sha256 {digest} does not match the manifest")
print(f"verified vendored core {version} ({key})")
PY
  fi
  CORE="$VENDOR_DIR/$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["artifacts"]["linux-x86_64"]["path"])' "$VENDOR_DIR/manifest.json")"
fi
[[ -f "$CORE" ]] || { echo "error: missing $CORE" >&2; exit 1; }

# --- 4. the app (self-contained, apphost named ppvpn) --------------------------------
PUBLISH="$WORK/publish"
dotnet publish "$PROJECT" -c Release -r "$RID" --self-contained true -o "$PUBLISH" -nologo \
  -p:Version="$VERSION" -p:PPVPNBuildNumber="$BUILD" -p:PPVPNApiBase="$API_BASE" \
  -p:PPVPNUpdateFeed="$UPDATE_FEED" -p:DebugType=None -p:DebugSymbols=false
# PPVPN.Client copies every runtimes/<rid>/native it has; ship only this one.
find "$PUBLISH/runtimes" -mindepth 1 -maxdepth 1 ! -name "$RID" -exec rm -rf {} +
[[ -f "$PUBLISH/runtimes/$RID/native/libppvpn_client.so" ]] || { echo "error: libppvpn_client.so missing from publish" >&2; exit 1; }

# --- 4b. the push agent (NativeAOT: links against this host's glibc, checked below) ----
AGENT="$WORK/agent"
dotnet publish "$APP_DIR/PPVPN.PushAgent/PPVPN.PushAgent.csproj" -c Release -r "$RID" -o "$AGENT" -nologo \
  -p:Version="$VERSION" -p:DebugType=None -p:DebugSymbols=false -p:StripSymbols=true
[[ -x "$AGENT/ppvpn-push-agent" ]] || { echo "error: ppvpn-push-agent missing from publish" >&2; exit 1; }

# --- 5. staging tree -----------------------------------------------------------------
STAGE="$WORK/stage"
mkdir -p "$STAGE/app"
cp -a "$PUBLISH/." "$STAGE/app/"
install -m 0755 "$CORE" "$STAGE/app/ppvpn-core"
# It loads libppvpn_client.so from the app's runtimes/ folder.
install -m 0755 "$AGENT/ppvpn-push-agent" "$STAGE/app/ppvpn-push-agent"
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  install -m 0755 "$SERVICE_OUT/$bin" "$STAGE/app/$bin"
done
printf 'deb\n' > "$STAGE/package-format.deb"
printf 'rpm\n' > "$STAGE/package-format.rpm"
# The development copy of the app icon is not needed: the package installs it into hicolor.
rm -rf "$STAGE/app/Resources/app-icon"
ICON_SOURCE="$REPO_DIR/assets/icons"
for entry in 32x32:32x32.png 128x128:128x128.png 256x256:128x128@2x.png; do
  size="${entry%%:*}"
  mkdir -p "$STAGE/icons/$size/apps"
  install -m 0644 "$ICON_SOURCE/${entry#*:}" "$STAGE/icons/$size/apps/com.peakpassvpn.ppvpn.desktop.png"
done

# The glibc floor is the point of zigbuild; make sure nothing slipped past it.
max_glibc() { readelf -W --dyn-syms "$1" | grep -o 'GLIBC_[0-9.]*' | sed 's/GLIBC_//' | sort -Vu | tail -1; }
for binary in "$STAGE/app/runtimes/$RID/native/libppvpn_client.so" "$STAGE/app/ppvpn-push-agent" "$STAGE/app/ppvpn-service" \
    "$STAGE/app/ppvpn-service-install" "$STAGE/app/ppvpn-service-uninstall"; do
  needed="$(max_glibc "$binary")"
  if [[ -n "$needed" ]] && [[ "$(printf '%s\n%s\n' "$needed" "$GLIBC" | sort -V | tail -1)" != "$GLIBC" ]]; then
    echo "error: $(basename "$binary") needs GLIBC_$needed, above $GLIBC" >&2
    exit 1
  fi
done

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

# --- 6. deb + rpm ----------------------------------------------------------------------
mkdir -p "$OUT_DIR"
export NFPM_ARCH PPVPN_VERSION="$VERSION" PPVPN_BUILD_NUMBER="$BUILD"
export PPVPN_STAGE_DIR="$STAGE" PPVPN_PACKAGING_DIR="$APP_DIR/packaging"
export PPVPN_PACKAGE_MAINTAINER="${PPVPN_PACKAGE_MAINTAINER:-PeakPass VPN LLC <support@peakpassvpn.com>}"
expand_config "$APP_DIR/packaging/nfpm.yaml" "$WORK/nfpm.yaml"
PACKAGES=()
for format in deb rpm; do
  package="$OUT_DIR/PPVPN-$VERSION-linux-$ARCH.$format"
  rm -f "$package"
  nfpm package --config "$WORK/nfpm.yaml" --packager "$format" --target "$package"
  PACKAGES+=("$package")
done

# --- 7. release metadata ---------------------------------------------------------------
# Linux updates through the apt/dnf repositories, which the release workflow signs with
# GPG; the packages themselves carry no EdDSA signature (ed_signature null).
for package in "${PACKAGES[@]}"; do
  format="${package##*.}"
  meta="$OUT_DIR/release-meta-linux-$ARCH-$format.json"
  python3 - "$meta" "$package" "linux-$ARCH-$format" "$CHANNEL" "$VERSION" "$VERSION.$BUILD" <<'PY'
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
    # README: Ubuntu 22.04+ / Debian 12+ (deb), Fedora 36+ (rpm).
    "min_os": "fedora-36" if package.endswith(".rpm") else "ubuntu-22.04",
    "published_at": datetime.datetime.now(datetime.timezone.utc).strftime("%Y-%m-%dT%H:%M:%SZ"),
}, open(meta, "w"), indent=2)
PY
  echo "wrote $(basename "$package") and $(basename "$meta")"
done
