#!/usr/bin/env bash
# Vendors a ppvpn-core release: downloads the desktop binaries from the
# GitHub Release of tag vX.Y.Z (macOS as one file per architecture, from
# 0.5.15), checks each against the release's SHA256SUMS,
# their build provenance (built by this repository's release.yml on that tag)
# and build-info.json (version, commit, desktop build tags), then replaces
# vendor/ppvpn-core/<old> with vendor/ppvpn-core/<version> and points CURRENT
# at it. Only CURRENT and the manifest are tracked: the binaries stay in the
# ignored build/ directory, where fetch-vendored-core.sh puts them on a fresh
# checkout.
#
#   scripts/vendor-core-release.sh 0.5.14
#
# Needs gh (signed in), jq,
# shasum and node.
set -euo pipefail

VERSION="${1:?usage: vendor-core-release.sh X.Y.Z}"
VERSION="${VERSION#v}"
TAG="v${VERSION}"
REPO="peakpassvpn/ppvpn-core"
DESKTOP_TAGS="with_utls,with_gvisor,with_dhcp"
ROOT_DIR="$(cd "$(dirname "$0")/.." && pwd)"
VENDOR_ROOT="${ROOT_DIR}/vendor/ppvpn-core"

# Manifest key → release file name.
ARTIFACTS=(
  "windows-x86_64:ppvpn-core-windows-amd64.exe"
  "macos-cli-arm64:ppvpn-core-darwin-arm64"
  "macos-cli-x86_64:ppvpn-core-darwin-amd64"
  "linux-x86_64:ppvpn-core-linux-amd64"
)

fail() {
  echo "vendor-core-release: $*" >&2
  exit 1
}

for tool in gh jq shasum node; do
  command -v "$tool" >/dev/null || fail "$tool not found"
done

OLD="$(cat "${VENDOR_ROOT}/CURRENT")"
[ "$OLD" != "$VERSION" ] || fail "vendor/ppvpn-core is already at ${VERSION}"

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT

files=(SHA256SUMS build-info.json)
for entry in "${ARTIFACTS[@]}"; do files+=("${entry#*:}"); done
patterns=()
for file in "${files[@]}"; do patterns+=(--pattern "$file"); done
gh release download "$TAG" --repo "$REPO" --dir "$WORK" "${patterns[@]}"

(cd "$WORK" && shasum -a 256 --check --ignore-missing --strict SHA256SUMS) >/dev/null \
  || fail "SHA256SUMS check failed"
for file in "${files[@]:1}"; do
  grep -q "  ${file}\$" "$WORK/SHA256SUMS" || fail "${file} is not listed in SHA256SUMS"
done

info="$WORK/build-info.json"
[ "$(jq -r .tag "$info")" = "$TAG" ] || fail "build-info tag is not ${TAG}"
[ "$(jq -r .core_version "$info")" = "$VERSION" ] || fail "build-info core_version is not ${VERSION}"
[ "$(jq -r .desktop_tags "$info")" = "$DESKTOP_TAGS" ] || fail "build-info desktop_tags is not ${DESKTOP_TAGS}"
COMMIT="$(jq -r .commit "$info")"
[[ "$COMMIT" =~ ^[0-9a-f]{40}$ ]] || fail "build-info commit is not a full SHA"

# Provenance (per downloaded file; the universal macOS binary that
# stage-macos-core.sh makes from the two has none): built by release.yml on this tag (gh prints nothing on success
# when not interactive, so the exit status is what counts).
for entry in "${ARTIFACTS[@]}"; do
  file="${entry#*:}"
  gh attestation verify "$WORK/$file" --repo "$REPO" \
    --signer-workflow "${REPO}/.github/workflows/release.yml" \
    --source-ref "refs/tags/${TAG}" >/dev/null \
    || fail "attestation check failed for ${file}"
done

# The binary itself reports the version (the host's own build only).
case "$(uname -s)" in
  Darwin) host="ppvpn-core-darwin-$([ "$(uname -m)" = arm64 ] && echo arm64 || echo amd64)" ;;
  Linux) host="ppvpn-core-linux-amd64" ;;
  *) host="" ;;
esac
if [ -n "$host" ]; then
  chmod +x "$WORK/$host"
  "$WORK/$host" version | grep -q "$VERSION" || fail "${host} does not report version ${VERSION}"
fi

NEW_DIR="${VENDOR_ROOT}/${VERSION}"
git -C "$ROOT_DIR" mv "${VENDOR_ROOT}/${OLD}" "$NEW_DIR"
# Drop binaries the release no longer ships (0.5.15: darwin-universal).
for path in "$NEW_DIR"/build/*; do
  keep=""
  for entry in "${ARTIFACTS[@]}"; do [ "$(basename "$path")" = "${entry#*:}" ] && keep=1; done
  [ -n "$keep" ] || rm -f "$path"
done
mkdir -p "$NEW_DIR/build"
artifacts_json="{}"
for entry in "${ARTIFACTS[@]}"; do
  key="${entry%%:*}"
  file="${entry#*:}"
  cp "$WORK/$file" "$NEW_DIR/build/$file"
  case "$file" in *.exe) ;; *) chmod +x "$NEW_DIR/build/$file" ;; esac
  sha="$(shasum -a 256 "$NEW_DIR/build/$file" | cut -d' ' -f1)"
  size="$(wc -c <"$NEW_DIR/build/$file" | tr -d ' ')"
  artifacts_json="$(jq --arg k "$key" --arg p "build/$file" --arg s "$sha" --argjson n "$size" \
    '.[$k] = {path: $p, sha256: $s, size: $n}' <<<"$artifacts_json")"
done
jq -n --arg v "$VERSION" --arg c "$COMMIT" --arg t "$TAG" \
  --arg u "https://github.com/${REPO}/releases/tag/${TAG}" --argjson a "$artifacts_json" \
  '{schema_version: 1, core_version: $v,
    source: {repository: "peakpassvpn/ppvpn-core", commit: $c, tag: $t, release_url: $u},
    artifacts: $a}' >"$NEW_DIR/manifest.json"
echo "$VERSION" >"${VENDOR_ROOT}/CURRENT"
git -C "$ROOT_DIR" add "$NEW_DIR" "${VENDOR_ROOT}/CURRENT"

for entry in "${ARTIFACTS[@]}"; do
  node "${ROOT_DIR}/scripts/verify-vendored-core.mjs" --vendor-dir "$NEW_DIR" \
    --artifact "${entry%%:*}" --expected-version "$VERSION"
done
echo "vendored ppvpn-core ${VERSION} (${COMMIT}) from ${TAG}; replaced ${OLD}"
