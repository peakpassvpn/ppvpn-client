#!/usr/bin/env bash
# What a pull request or a main push changes: prints `name=true|false` lines
# for $GITHUB_OUTPUT.
#   code  anything outside docs/ and Markdown. ci.yml's jobs skip their
#         build and test steps when it is false (the jobs still run and
#         report, so a docs-only pull request passes quickly).
#   desktop  apps/, crates/{desktop,engine-host,service}, tools/desktop/,
#         the dependencies or the workflows: what the desktop
#         app builds (the macOS Debug app, the Windows C# bindings and app)
#         depend on. ci.yml skips those builds when it is false.
#   flutter  apps/flutter/ (the Flutter desktop app, #401), the crates it
#         links (crates/{desktop,account,core,engine-host}) and the service
#         its installers ship (crates/service), apps/linux/packaging/ (its deb and rpm take the
#         native package's scripts), what its macOS packaging takes from
#         apps/macos (the scripts, the push agent and what it builds from,
#         the LaunchAgent, the .pkg's postinstall), the Windows installer
#         script it shares with the native app (apps/windows/installer/,
#         scripts/sign-file.ps1), the dependencies or the workflows. ci.yml's
#         flutter job builds only when it is true. apps/flutter/ alone
#         does not set desktop: the native apps do not depend on it.
#         And outside apps/flutter/rust/ it does not set code either: a
#         change to the Flutter app's Dart, scripts or assets runs only the
#         flutter job (and lint's sensitive check, which always runs). Its
#         Rust bridge (apps/flutter/rust/) and push agent
#         (apps/flutter/push-agent/) are workspace members, so they still
#         set code for the workspace's clippy and tests.
#   deps  Cargo.toml, Cargo.lock, rust-toolchain.toml, the shared actions or
#         the two workflows: what the build caches are keyed on or built by.
#         release.yml warms the caches on main only when it is true; ci.yml's
#         advisories job fails on an advisory only then.
#   tunnel  the mobile tunnel host (crates/tunnel-host/, #448),
#         crates/engine-host/, the dependencies (Cargo.toml, crates/core's
#         target-specific ones, Cargo.lock) or ci.yml, and the only core code
#         that iOS compiles and macOS does not (the fallbacks of
#         hostipv6/mod.rs and localdns/source.rs; the rest builds the same
#         as on macOS, which the macos job's clippy covers): ci.yml's
#         `tunnel (ios)` builds it for iOS only then (~25 min cold, on the
#         scarce macOS runners).
#   memory  the memory harness (ne-mem, tools/perf/{ne_mem,dhat_top}.py, test/perf/)
#         or Cargo.lock (a sail upgrade): ci.yml's macos and linux jobs
#         then build it in release and measure (#388).
# Every other event (workflow_dispatch, a new branch), a compare that fails,
# or one with 300 files or more (the API's page limit) counts as changing
# everything.
set -euo pipefail

all() { printf '%s=true\n' code desktop flutter deps memory tunnel; exit 0; }

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

code=false desktop=false flutter=false deps=false memory=false tunnel=false
while IFS= read -r f; do
  case $f in
    Cargo.lock | crates/core-lab/src/bin/ne-mem.rs | tools/perf/ne_mem.py | tools/perf/dhat_top.py | \
    test/perf/*) memory=true ;;
  esac
  case $f in
    *.md) ;;
    Cargo.toml | Cargo.lock | rust-toolchain.toml | .github/workflows/ci.yml | crates/core/Cargo.toml | \
    crates/core/src/hostipv6/mod.rs | crates/core/src/localdns/source.rs | \
    crates/engine-host/* | crates/tunnel-host/*) tunnel=true ;;
  esac
  case $f in
    docs/* | *.md) ;;
    Cargo.toml | Cargo.lock | rust-toolchain.toml | .github/actions/* | \
    .github/workflows/ci.yml | .github/workflows/release.yml)
      deps=true desktop=true flutter=true code=true ;;
    apps/flutter/rust/* | apps/flutter/push-agent/*) flutter=true code=true ;;
    apps/flutter/*) flutter=true ;;
    crates/desktop/* | crates/engine-host/* | crates/service/*) desktop=true flutter=true code=true ;;
    crates/account/* | crates/core/*) flutter=true code=true ;;
    # The maintainer scripts and autostart entry both Linux packages ship.
    apps/linux/packaging/*) desktop=true flutter=true code=true ;;
    apps/macos/scripts/* | apps/macos/PPVPNAgent/* | apps/macos/AppLogic/* | \
    apps/macos/LaunchAgents/* | apps/macos/packaging/* | apps/macos/project.yml | \
    apps/windows/installer/* | apps/windows/scripts/sign-file.ps1)
      desktop=true flutter=true code=true ;;
    apps/* | tools/desktop/*)
      desktop=true code=true ;;
    *) code=true ;;
  esac
done <<<"$files"
echo "code=$code"
echo "desktop=$desktop"
echo "flutter=$flutter"
echo "deps=$deps"
echo "memory=$memory"
echo "tunnel=$tunnel"
