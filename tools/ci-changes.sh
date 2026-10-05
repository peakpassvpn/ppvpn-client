#!/usr/bin/env bash
# Which parts of the repository a pull request or main push changes, for the
# workflows' `changes` job: prints `engine=true|false` and
# `desktop=true|false` for $GITHUB_OUTPUT. Jobs skipped through these
# outputs still satisfy required checks (a skipped job counts as passed), so
# a workflow can skip work without leaving a required check pending.
#   engine   anything outside desktop/, docs/, Markdown and the desktop
#            workflows
#   desktop  anything under desktop/ or a desktop workflow
# Every other event (schedule, workflow_dispatch, a new branch), a compare
# that fails, or one with 300 files or more (the API's page limit) counts as
# changing everything.
set -euo pipefail

all() { echo "engine=true"; echo "desktop=true"; exit 0; }

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

engine=false desktop=false
while IFS= read -r f; do
  case $f in
    desktop/* | .github/workflows/desktop*) desktop=true ;;
    docs/* | *.md) ;;
    *) engine=true ;;
  esac
done <<<"$files"
echo "engine=$engine"
echo "desktop=$desktop"
