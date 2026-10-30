#!/usr/bin/env bash
# Installs the pinned Flutter SDK into $1 (kept when it is already there)
# and puts it on GITHUB_PATH. The archive's SHA-256 is
# pinned here, from Flutter's release index for this version.
set -euo pipefail

version=3.47.7
dir=$1
base=https://storage.googleapis.com/flutter_infra_release/releases

case "$(uname -s)/$(uname -m)" in
  Linux/x86_64)
    archive=stable/linux/flutter_linux_$version-stable.tar.xz
    sha=af7455f540df951184c7953fbe3cf884766e7541dacf8571f9f582db03dd6230 ;;
  Darwin/arm64)
    archive=stable/macos/flutter_macos_arm64_$version-stable.zip
    sha=dd9da574c3168460550117eb0e8dd0313ae934fabf64a82a6d557fff5fd1422e ;;
  MINGW*/x86_64 | MSYS*/x86_64)
    archive=stable/windows/flutter_windows_$version-stable.zip
    sha=c2c875da25a1a1448d18ee4e8f398b2f6fce888b4775d8f9130538e16d826cb4 ;;
  *) echo "no pinned Flutter for $(uname -s)/$(uname -m)" >&2; exit 1 ;;
esac

if [ ! -x "$dir/flutter/bin/flutter" ]; then
  tmp=$(mktemp -d)
  curl -fsSL --retry 3 -o "$tmp/sdk" "$base/$archive"
  echo "$sha  $tmp/sdk" | sha256sum -c - 2>/dev/null || echo "$sha  $tmp/sdk" | shasum -a 256 -c -
  mkdir -p "$dir"
  case $archive in
    *.tar.xz) tar -xJf "$tmp/sdk" -C "$dir" ;;
    *) unzip -q "$tmp/sdk" -d "$dir" ;;
  esac
  rm -rf "$tmp"
fi
# The SDK is a git checkout owned by another user on some runners.
git config --global --add safe.directory '*'
echo "$dir/flutter/bin" >> "$GITHUB_PATH"
"$dir/flutter/bin/flutter" --version
