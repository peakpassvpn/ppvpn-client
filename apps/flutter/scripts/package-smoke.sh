#!/usr/bin/env bash
# Checks a package from package-linux.sh in a throwaway container, as root
# (ci.yml, release.yml: the images in ../packaging/linux/smoke-images.sh):
#
# 1. installs it alone (no recommended packages) and checks the layout and
#    that every binary resolves its libraries (ldd) from its dependencies;
# 2. starts /usr/bin/ppvpn once on Xvfb with a session bus: still running
#    after a while, the client's and the push agent's logs written;
# 3. removes it (the maintainer scripts);
# 4. deb, with a native package given: installs that, then upgrades it in
#    place to this one.
#
# The checks install nothing from the distribution's mirrors: every package
# they need comes from PPVPN_SMOKE_PKGS (the workflows keep it in the Actions
# cache), with the repositories off. A mirror that fails mid-download failed
# the rpm smoke on main (release run 41001515285). Only a set missing from
# there is fetched first (step 0), for this package and image, so that
# happens when the image, this script or the package's dependencies change.
#
# Usage: [PPVPN_SMOKE_PKGS=<dir>] apps/flutter/scripts/package-smoke.sh <package.deb|package.rpm> [<native deb>]
set -euo pipefail
shopt -s nullglob

package=$(realpath "${1:?usage: package-smoke.sh <package> [<native deb>]}")
native=${2:+$(realpath "$2")}
format=${package##*.}
pkgs=${PPVPN_SMOKE_PKGS:-$(mktemp -d)}
case $format in
  deb)
    export DEBIAN_FRONTEND=noninteractive
    # What a desktop has anyway: a display (here Xvfb), Mesa for the engine's EGL, a font.
    desktop=(xvfb dbus libegl-mesa0 libgl1-mesa-dri fonts-dejavu-core)
    pkg_fetch() {
      local dir=$1; shift
      mkdir -p "$dir/partial"
      apt-get install -y -qq --no-install-recommends --download-only \
        -o Dir::Cache::archives="$dir" "$@" >/dev/null
      rm -rf "$dir/partial" "$dir/lock"
    }
    pkg_install() { apt-get install -y -qq --no-install-recommends "$@" >/dev/null; }
    pkg_remove() { apt-get remove -y -qq ppvpn >/dev/null; } ;;
  rpm)
    desktop=(xorg-x11-server-Xvfb dbus-daemon mesa-libEGL mesa-dri-drivers dejavu-sans-fonts)
    pkg_fetch() {
      local dir=$1; shift
      dnf install -y -q --setopt=install_weak_deps=False --downloadonly --destdir "$dir" "$@"
    }
    pkg_install() { dnf install -y -q --setopt=install_weak_deps=False --disablerepo='*' "$@"; }
    pkg_remove() { dnf remove -y -q --disablerepo='*' ppvpn; } ;;
  *) echo "FAIL: not a deb or rpm: $package" >&2; exit 2 ;;
esac

# --- 0. the packages the checks install ---------------------------------------------------
# Each set is what its install below needs on top of the image: the package's
# dependencies, the desktop's, the native package's. Fetched without being
# installed, so each set is complete on its own.
sets=(app desktop)
[[ $format == deb && -n "$native" ]] && sets+=(native)
fetched=
for set in "${sets[@]}"; do
  [[ -e "$pkgs/$set.complete" ]] && continue
  if [[ $format == deb && -z "$fetched" ]]; then apt-get update -qq; fi
  fetched=1
  rm -rf "${pkgs:?}/$set"; mkdir -p "$pkgs/$set"
  case $set in
    app) pkg_fetch "$pkgs/app" "$package" ;;
    desktop) pkg_fetch "$pkgs/desktop" "${desktop[@]}" ;;
    native) pkg_fetch "$pkgs/native" "$native" ;;
  esac
  touch "$pkgs/$set.complete"
  files=("$pkgs/$set"/*)
  echo "fetched the $set set: ${#files[@]} packages"
done
# From here on, nothing from the mirrors: apt without its lists, dnf without
# its repositories.
if [[ $format == deb ]]; then rm -rf /var/lib/apt/lists/*; fi

# --- 1. install, layout, libraries --------------------------------------------------------
pkg_install "$package" "$pkgs"/app/*."$format"
APP=/usr/lib/ppvpn/ppvpn
[[ -x "$APP" ]] || { echo "FAIL: $APP is not installed" >&2; exit 1; }
[[ "$(readlink -f /usr/bin/ppvpn)" == "$APP" ]] || { echo "FAIL: /usr/bin/ppvpn does not point at $APP" >&2; exit 1; }
[[ "$(cat /usr/lib/ppvpn/package-format)" == "$format" ]] || { echo "FAIL: package-format is not $format" >&2; exit 1; }
for file in /usr/share/applications/com.peakpassvpn.ppvpn.desktop.desktop \
    /usr/share/icons/hicolor/128x128/apps/com.peakpassvpn.ppvpn.desktop.png \
    /usr/lib/ppvpn/lib/libflutter_linux_gtk.so /usr/lib/ppvpn/lib/libapp.so \
    /usr/lib/ppvpn/lib/libppvpn_flutter.so /usr/lib/ppvpn/data/icudtl.dat \
    /usr/lib/ppvpn/data/flutter_assets /usr/lib/ppvpn/ppvpn-service \
    /usr/lib/ppvpn/ppvpn-service-install /usr/lib/ppvpn/ppvpn-service-uninstall \
    /usr/lib/ppvpn/ppvpn-push-agent /etc/xdg/autostart/com.peakpassvpn.ppvpn.push-agent.desktop; do
  [[ -e "$file" ]] || { echo "FAIL: missing $file" >&2; exit 1; }
done
# The service accepts only a root-owned app that nobody else can write.
[[ "$(stat -c '%U %a' "$APP")" == "root 755" ]] || { echo "FAIL: $APP is $(stat -c '%U %a' "$APP")" >&2; exit 1; }

# The runner by its real path: ld.so takes $ORIGIN (RUNPATH $ORIGIN/lib) from
# /proc/self/exe when it runs, but from the path it is given under ldd. The
# start below goes through /usr/bin/ppvpn. The plugins and the bridge with
# lib/ on the path, as they are loaded next to libflutter_linux_gtk.so.
missing=
check_ldd() {
  local result
  result=$(env "$@" 2>&1) || { echo "$result"; missing=1; return; }
  if grep -E 'not found|not a dynamic' <<<"$result"; then missing=1; fi
}
check_ldd ldd "$APP"
# (libapp.so is the Dart code, which links nothing.)
for lib in /usr/lib/ppvpn/lib/*.so; do
  [[ $lib == */libapp.so ]] && continue
  echo "${lib#/usr/lib/ppvpn/}"
  check_ldd LD_LIBRARY_PATH=/usr/lib/ppvpn/lib ldd "$lib"
done
for bin in ppvpn-push-agent ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  check_ldd ldd "/usr/lib/ppvpn/$bin"
done
[[ -z "$missing" ]] || { echo "FAIL: libraries above are not resolved by the package's dependencies" >&2; exit 1; }
echo "OK: the package's dependencies resolve every library"

# --- 2. start it once ---------------------------------------------------------------------
pkg_install "$pkgs"/desktop/*."$format"
WORK=$(mktemp -d)
trap 'kill "${XVFB_PID:-}" 2>/dev/null || true' EXIT
export HOME="$WORK/home" XDG_RUNTIME_DIR="$WORK/run"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR"
chmod 700 "$XDG_RUNTIME_DIR"
unset XDG_DATA_HOME XDG_STATE_HOME XDG_CONFIG_HOME
Xvfb :91 -screen 0 1280x800x24 >/dev/null 2>&1 &
XVFB_PID=$!
export DISPLAY=:91
sleep 2
cat >"$WORK/start.sh" <<'EOF'
#!/bin/bash
# Inside the session bus: still running after 15 s?
MALLOC_ARENA_MAX=2 /usr/bin/ppvpn >"$1" 2>&1 &
pid=$!
sleep 15
if ! kill -0 "$pid" 2>/dev/null; then
  echo "FAIL: ppvpn exited within 15 s" >&2
  exit 1
fi
kill "$pid"
wait "$pid" 2>/dev/null || true
EOF
chmod +x "$WORK/start.sh"
status=0
dbus-run-session -- "$WORK/start.sh" "$WORK/app.out" || status=$?
logs=$HOME/.local/state/ppvpn/logs
if [[ $status != 0 ]] || ! ls "$logs"/ppvpn-client.*.log >/dev/null 2>&1 \
    || ! ls "$logs"/ppvpn-push-agent.*.log >/dev/null 2>&1; then
  echo "--- app output"; tail -40 "$WORK/app.out" || true
  echo "--- logs"; ls -l "$logs" 2>/dev/null || true
  tail -20 "$logs"/*.log 2>/dev/null || true
  echo "FAIL: ppvpn did not start, or wrote no client or push agent log" >&2
  exit 1
fi
echo "OK: /usr/bin/ppvpn started and ran 15 s; the client and the push agent wrote their logs"

# --- 3. remove ----------------------------------------------------------------------------
pkg_remove
[[ ! -e /usr/lib/ppvpn/ppvpn && ! -e /usr/bin/ppvpn ]] || { echo "FAIL: the app is left after removal" >&2; exit 1; }
echo "OK: removed"

# --- 4. upgrade the native package in place -----------------------------------------------
if [[ $format == deb && -n "$native" ]]; then
  pkg_install "$native" "$pkgs"/native/*.deb
  before=$(dpkg-query -W -f '${Version}' ppvpn)
  [[ -d /usr/lib/ppvpn/runtimes ]] || { echo "FAIL: $native is not the native package" >&2; exit 1; }
  pkg_install "$package" "$pkgs"/app/*.deb
  after=$(dpkg-query -W -f '${Version}' ppvpn)
  [[ "$after" == "$(dpkg-deb -f "$package" Version)" ]] || { echo "FAIL: ppvpn is $after after the upgrade" >&2; exit 1; }
  [[ -e /usr/lib/ppvpn/lib/libflutter_linux_gtk.so && ! -e /usr/lib/ppvpn/runtimes ]] ||
    { echo "FAIL: the native app's files are still in /usr/lib/ppvpn" >&2; exit 1; }
  [[ "$(cat /usr/lib/ppvpn/package-format)" == deb ]] || { echo "FAIL: package-format after the upgrade" >&2; exit 1; }
  echo "OK: upgraded the native package $before in place to $after"
fi
