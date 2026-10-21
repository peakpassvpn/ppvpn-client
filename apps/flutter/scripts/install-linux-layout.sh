#!/usr/bin/env bash
# Installs the Flutter Linux bundle in the native packages' layout
# (apps/linux/packaging/nfpm.yaml), as root (#401). Linux only; no /usr/bin
# link, desktop entry or icons (the deb and rpm: scripts/package-linux.sh).
#
#   /usr/lib/ppvpn/ppvpn          the bundle's ppvpn: the
#                                 only client ppvpn-service accepts is
#                                 /usr/lib/ppvpn/ppvpn, root-owned and not
#                                 group or world writable
#                                 (crates/service/src/ipc.rs)
#   /usr/lib/ppvpn/lib/, data/    the rest of the bundle: the runner loads
#                                 them from next to its executable
#                                 (lib/libppvpn_flutter.so is the bridge)
#   /usr/lib/ppvpn/ppvpn-service{,-install,-uninstall}
#                                 next to the app, where rust/src/hooks.rs
#                                 looks for the helpers; ppvpn-service-install
#                                 copies the service into /usr/lib/ppvpn-service
#
# Usage: sudo apps/flutter/scripts/install-linux-layout.sh <bundle> <service-bin-dir>
#   <bundle>           build/linux/x64/release/bundle from scripts/ci-build.sh
#   <service-bin-dir>  the three service binaries, e.g. target/service from
#                      `cargo build --locked -p ppvpn-service --profile service --bins`
#
# Replaces an earlier install of this layout. Refuses to touch a native
# package's /usr/lib/ppvpn (it has package-format; remove the package first).
# Remove it again with /usr/lib/ppvpn/ppvpn-service-uninstall (the service)
# and rm -rf /usr/lib/ppvpn.
set -euo pipefail

[ "$(uname -s)" = Linux ] || { echo "install-linux-layout.sh: Linux only" >&2; exit 2; }
[ "$(id -u)" = 0 ] || { echo "install-linux-layout.sh: run as root" >&2; exit 2; }
bundle=$(realpath "${1:?usage: install-linux-layout.sh <bundle> <service-bin-dir>}")
service=$(realpath "${2:?usage: install-linux-layout.sh <bundle> <service-bin-dir>}")
dest=/usr/lib/ppvpn

for path in "$bundle/ppvpn" "$bundle/lib/libppvpn_flutter.so" "$bundle/data" \
    "$service/ppvpn-service" "$service/ppvpn-service-install" "$service/ppvpn-service-uninstall"; do
  [ -e "$path" ] || { echo "install-linux-layout.sh: $path is missing" >&2; exit 1; }
done
if [ -e "$dest/package-format" ]; then
  echo "install-linux-layout.sh: $dest belongs to the native package; remove it first" >&2
  exit 1
fi

# Staged next to the destination, then swapped in.
stage=$(mktemp -d /usr/lib/.ppvpn-stage.XXXXXX)
trap 'rm -rf "$stage"' EXIT
cp -R "$bundle/lib" "$bundle/data" "$stage/"
install -m 0755 "$bundle/ppvpn" "$stage/ppvpn"
for bin in ppvpn-service ppvpn-service-install ppvpn-service-uninstall; do
  install -m 0755 "$service/$bin" "$stage/$bin"
done
chown -R root:root "$stage"
chmod -R u+rwX,go+rX,go-w "$stage"
chmod 0755 "$stage"

rm -rf "$dest"
mv "$stage" "$dest"
trap - EXIT
echo "installed the Flutter app in $dest"
