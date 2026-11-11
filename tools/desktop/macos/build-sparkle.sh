#!/usr/bin/env bash
# Build the existing standard Sparkle framework from its fixed upstream source.
set -euo pipefail
work=${1:?usage: build-sparkle.sh work-dir arm64-or-x86_64}
arch=${2:?usage: build-sparkle.sh work-dir arm64-or-x86_64}
case "$arch" in arm64|x86_64) ;; *) echo 'Unsupported Sparkle architecture' >&2; exit 2;; esac
revision=ab7d71291238df6e0492143b120c9b696a35f603
here=$(cd "$(dirname "$0")" && pwd)
patch="$here/sparkle-2.10.1-fullscreen.patch"
mkdir -p "$work"
work=$(cd "$work" && pwd)
source="$work/source"
if [ ! -d "$source/.git" ]; then
  git clone --depth 1 --branch 2.10.1 https://github.com/sparkle-project/Sparkle.git "$source"
fi
[ "$(git -C "$source" rev-parse HEAD)" = "$revision" ] || { echo 'Unexpected Sparkle source revision' >&2; exit 1; }
marker="$work/applied-patch.sha256"
expected=$(shasum -a 256 "$patch" | cut -d ' ' -f 1)
if [ ! -f "$marker" ]; then
  [ -z "$(git -C "$source" status --porcelain)" ] || { echo 'Refusing modified Sparkle source' >&2; exit 1; }
  git -C "$source" apply --unidiff-zero --check "$patch"
  git -C "$source" apply --unidiff-zero "$patch"
  printf '%s\n' "$expected" > "$marker"
fi
[ "$(cat "$marker")" = "$expected" ] || { echo 'Sparkle patch changed; use a fresh owned build directory' >&2; exit 1; }
git -C "$source" diff --unified=0 -- Sparkle/SPUStandardUserDriver.m Sparkle/InstallerProgress/InstallerProgressAppController.m > "$work/applied.patch"
cmp "$patch" "$work/applied.patch"
xcodebuild -project "$source/Sparkle.xcodeproj" -scheme Sparkle -configuration Release \
  -derivedDataPath "$work/derived" -destination 'generic/platform=macOS' \
  ARCHS="$arch" ONLY_ACTIVE_ARCH=NO CODE_SIGNING_ALLOWED=NO build
framework="$work/derived/Build/Products/Release/Sparkle.framework"
[ -f "$framework/Versions/B/Sparkle" ] || { echo 'Patched Sparkle.framework missing' >&2; exit 1; }
printf '%s\n' "$framework" > "$work/framework.path"
