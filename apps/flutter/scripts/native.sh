#!/usr/bin/env bash
# Fetches the native app of release $1 (e.g. v0.6.3) for this runner's
# platform into $2, checks it against the release's SHA256SUMS and unpacks
# it without running an installer: no service is installed. Writes
# `installer`, `root` (the unpacked app) and `exe` to $GITHUB_OUTPUT.
# Linux installs the .deb with apt instead, for its dependencies (GTK 4,
# libadwaita); its postinst touches no service on a fresh system.
set -euo pipefail

tag=$1
dir=$(mkdir -p "$2" && cd "$2" && pwd)
version=${tag#v}

case "$(uname -s)" in
  Linux) asset=PPVPN-$version-linux-x64.deb ;;
  Darwin) asset=PPVPN-$version-macos-arm64.pkg ;;
  *) asset=PPVPN-$version-windows-x64-setup.exe ;;
esac

gh release download "$tag" --repo "$GITHUB_REPOSITORY" -p "$asset" -p SHA256SUMS -D "$dir" --clobber
(cd "$dir" && grep " $asset\$" SHA256SUMS | { sha256sum -c - 2>/dev/null || shasum -a 256 -c -; })

case "$(uname -s)" in
  Linux)
    sudo apt-get install -y --no-install-recommends "$dir/$asset" >/dev/null
    root=/usr/lib/ppvpn
    exe=/usr/lib/ppvpn/ppvpn ;;
  Darwin)
    pkgutil --expand-full "$dir/$asset" "$dir/expanded"
    app=$(find "$dir/expanded" -maxdepth 4 -name 'PPVPN.app' -type d | head -1)
    root=$app
    exe=$app/Contents/MacOS/PPVPN ;;
  *)
    7z x -y -o"$dir/unpacked" "$dir/$asset" >/dev/null
    exe=$(find "$dir/unpacked" -maxdepth 3 -iname ppvpn.exe | head -1)
    root=$(dirname "$exe") ;;
esac
test -x "$exe" || test -f "$exe"
{
  echo "installer=$dir/$asset"
  echo "root=$root"
  echo "exe=$exe"
} >> "${GITHUB_OUTPUT:-/dev/stdout}"
