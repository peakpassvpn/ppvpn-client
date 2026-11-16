#!/usr/bin/env bash
# Prepared only. Run on the managed Darwin builder AFTER Root dispatch.
set -euo pipefail
: "${ROOT_AUTO359_DISPATCH:?Root dispatch reference required}"
: "${AUTO359_CHECKOUT:?isolated exact checkout required}"
: "${PPVPN_ACCEPTANCE_FIXTURE_ID:?Desktop/Root fixture ID required}"
: "${PPVPN_API_BASE_DEFAULT:?explicit fixture compile API origin required}"
[ "$(uname -s)" = Darwin ]
cd "$AUTO359_CHECKOUT"
[ "$(git rev-parse HEAD)" = 04789dbccafd245fda71de26a575a7b31d15cfcf ]
[ "$(git rev-parse 'HEAD^{tree}')" = af5f27c52c39d5ffb571ea6cb9bf922f8ace7bd5 ]
[ -z "$(git status --porcelain --untracked-files=all)" ]
python3 - <<'CHECK'
import os,urllib.parse,re
u=urllib.parse.urlsplit(os.environ['PPVPN_API_BASE_DEFAULT'])
assert u.scheme in ('http','https') and u.hostname in ('127.0.0.1','localhost','::1') and not u.username and not u.password
assert re.fullmatch(r'[A-Za-z0-9][A-Za-z0-9._-]{0,95}',os.environ['PPVPN_ACCEPTANCE_FIXTURE_ID'])
CHECK
unset FLUTTER_TARGET PPVPN_SKIP_ANALYZE PPVPN_PREPARE_APP_ROOT PPVPN_ANDROID_RUNNER_PREPARE_ONLY
unset PPVPN_UPDATE_SITE PPVPN_RELEASE_CHANNEL PPVPN_UPDATE_FEED_URL PPVPN_UPDATE_FEED PPVPN_UPDATE_PUBLIC_KEY PPVPN_SPARKLE_PUBLIC_KEY
unset PPVPN_NOTARY_KEY_FILE PPVPN_NOTARY_KEY_ID PPVPN_NOTARY_ISSUER ASC_KEY_ID ASC_ISSUER_ID ASC_KEY_P8 SPARKLE_PRIVATE_KEY_FILE SPARKLE_PRIVATE_KEY
export PPVPN_BUILD_NUMBER=359 PPVPN_CONTROLLED_AUTO_ACCEPTANCE=1
export PPVPN_ACCEPTANCE_BUILD_NUMBER=359
export PPVPN_ACCEPTANCE_SOURCE_COMMIT=04789dbccafd245fda71de26a575a7b31d15cfcf
export PPVPN_ACCEPTANCE_SOURCE_TREE=af5f27c52c39d5ffb571ea6cb9bf922f8ace7bd5
export PPVPN_MACOS_SINGLE_RUNTIME=1 PPVPN_APPLE_PUSH_ENABLED=0 PPVPN_MACOS_APNS_MIGRATION_READY=0 MACOS_ARCHS=arm64
# ci-build performs the one pinned FRB generation and actual Dart analysis.
bash apps/flutter/scripts/ci-build.sh
# Sign/package is a separate stage, reusing the accepted Developer ID/helper recipe.
# Never run the broad CI packaging wrapper: it can import keys and notarize.
