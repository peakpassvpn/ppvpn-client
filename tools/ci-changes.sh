#!/usr/bin/env bash
# Which parts of the repository a pull request, merge queue entry or main
# push changes, for ci.yml's `changes` job: prints `name=true|false` lines
# for $GITHUB_OUTPUT. Jobs skipped through these outputs still satisfy
# required checks (a skipped job counts as passed), so ci.yml can skip work
# without leaving a required check pending.
#   engine     anything outside desktop/, docs/, Markdown and the desktop
#              release workflows
#   desktop    anything under desktop/ or a desktop workflow
#   packaging  the installers' inputs (desktop-package.yml's own scripts and
#              files, tools/release-features-check.sh)
#   deps       Cargo.toml, Cargo.lock or rust-toolchain.toml: what the build
#              caches are keyed on
# Every other event (workflow_dispatch, a new branch), a compare that fails,
# or one with 300 files or more (the API's page limit) counts as changing
# everything.
set -euo pipefail

all() { printf '%s=true\n' engine desktop packaging deps; exit 0; }

case "${GITHUB_EVENT_NAME:-}" in
  pull_request) base=${PR_BASE_SHA:-} ;;
  merge_group) base=${MERGE_BASE_SHA:-} ;;
  push) base=${PUSH_BEFORE:-} ;;
  *) all ;;
esac
[[ -n $base && $base != 0000000000000000000000000000000000000000 ]] || all

files=$(gh api "repos/$GITHUB_REPOSITORY/compare/$base...$GITHUB_SHA" \
          --jq '.files[].filename') || all
n=$(grep -c . <<<"$files" || true)
(( n > 0 && n < 300 )) || all

engine=false desktop=false packaging=false deps=false
while IFS= read -r f; do
  case $f in
    desktop/scripts/* | desktop/apps/*/scripts/* | desktop/apps/linux/packaging/* | \
    desktop/apps/windows/installer/* | desktop/crates/ppvpn-client/scripts/* | \
    desktop/vendor/* | .github/workflows/desktop-package.yml)
      packaging=true desktop=true ;;
    tools/release-features-check.sh) packaging=true engine=true ;;
    desktop/* | .github/workflows/desktop*) desktop=true ;;
    docs/* | *.md) ;;
    Cargo.toml | Cargo.lock | rust-toolchain.toml) deps=true engine=true ;;
    *) engine=true ;;
  esac
done <<<"$files"
echo "engine=$engine"
echo "desktop=$desktop"
echo "packaging=$packaging"
echo "deps=$deps"
