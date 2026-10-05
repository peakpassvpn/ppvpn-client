#!/usr/bin/env bash
# What a pull request or a main push changes: prints `name=true|false` lines
# for $GITHUB_OUTPUT.
#   code  anything outside docs/ and Markdown. ci.yml's jobs skip their
#         build and test steps when it is false (the jobs still run and
#         report, so a docs-only pull request passes quickly).
#   desktop  desktop/, the dependencies or the workflows: what the desktop
#         app builds (the macOS Debug app, the Windows C# bindings and app)
#         depend on. ci.yml skips those builds when it is false.
#   deps  Cargo.toml, Cargo.lock, rust-toolchain.toml, the shared actions or
#         the two workflows: what the build caches are keyed on or built by.
#         release.yml warms the caches on main only when it is true.
# Every other event (workflow_dispatch, a new branch), a compare that fails,
# or one with 300 files or more (the API's page limit) counts as changing
# everything.
set -euo pipefail

all() { printf '%s=true\n' code desktop deps; exit 0; }

case "${GITHUB_EVENT_NAME:-}" in
  pull_request) base=${PR_BASE_SHA:-} ;;
  push) base=${PUSH_BEFORE:-} ;;
  *) all ;;
esac
[[ -n $base && $base != 0000000000000000000000000000000000000000 ]] || all

files=$(gh api "repos/$GITHUB_REPOSITORY/compare/$base...$GITHUB_SHA" \
          --jq '.files[].filename') || all
n=$(grep -c . <<<"$files" || true)
(( n > 0 && n < 300 )) || all

code=false desktop=false deps=false
while IFS= read -r f; do
  case $f in
    docs/* | *.md) ;;
    Cargo.toml | Cargo.lock | rust-toolchain.toml | .github/actions/* | \
    .github/workflows/ci.yml | .github/workflows/release.yml)
      deps=true desktop=true code=true ;;
    desktop/*) desktop=true code=true ;;
    *) code=true ;;
  esac
done <<<"$files"
echo "code=$code"
echo "desktop=$desktop"
echo "deps=$deps"
