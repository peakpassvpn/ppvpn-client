#!/usr/bin/env bash
# Both stages run the full product build; only canonical build caches persist.
set -euo pipefail
unset IOS_TEAM_ID IOS_APP_PROFILE IOS_TUNNEL_PROFILE IOS_EXPORT_OPTIONS
unset PPVPN_API_BASE PPVPN_FLUTTER_HOME PPVPN_FLUTTER_SAMPLE PPVPN_FLUTTER_SYNTHETIC_TRAFFIC
mode=${1:-primary}
primary_origin=$PPVPN_API_BASE_DEFAULT
paired_manifest="$RUNNER_TEMP/apple-pair.json"
case "$mode" in
  primary)
    git clone --no-hardlinks "$GITHUB_WORKSPACE" "$RUNNER_TEMP/apple-candidate"
    git -C "$RUNNER_TEMP/apple-candidate" checkout --detach "$SOURCE_REF"
    sides=(candidate)
    if [[ "$CANDIDATE_ONLY" != 1 ]]; then
      sides+=(baseline)
      git clone --no-hardlinks "$GITHUB_WORKSPACE" "$RUNNER_TEMP/apple-baseline"
      git -C "$RUNNER_TEMP/apple-baseline" checkout --detach "$BASELINE_REF"
    fi
    ;;
  secondary)
    [[ -f "$paired_manifest" && "$CANDIDATE_ONLY" == 1 && -z "$REUSE_BASELINE_RUN" ]] || exit 2
    [[ "$(git -C "$RUNNER_TEMP/apple-candidate" rev-parse HEAD)" == "$SOURCE_REF" ]] || exit 2
    sides=(secondary)
    ;;
  *) echo 'unknown canonical archive stage' >&2; exit 2 ;;
esac
mkdir -p "$CARGO_TARGET_DIR" "$RUNNER_TEMP/apple-output"
for side in "${sides[@]}"; do
  if [[ "$side" == baseline && -n "$REUSE_BASELINE_RUN" ]]; then
    origin_hash=$(python3 -c 'import os,hashlib; print(hashlib.sha256(os.environ["PPVPN_API_BASE_DEFAULT"].encode()).hexdigest())')
    python3 "$GITHUB_WORKSPACE/tools/apple/reuse_ios_baseline.py" \
      --download "$RUNNER_TEMP/apple-baseline-reuse" --output "$RUNNER_TEMP/apple-output/baseline" \
      --repo "$GITHUB_WORKSPACE" --source "$BASELINE_REF" --build "$PPVPN_BUILD_NUMBER" \
      --origin-hash "$origin_hash" --run "$REUSE_BASELINE_RUN"
    continue
  fi
  checkout="$RUNNER_TEMP/apple-$side"
  export PPVPN_API_BASE_DEFAULT="$primary_origin"
  if [[ "$side" == secondary ]]; then
    checkout="$RUNNER_TEMP/apple-candidate"
    PPVPN_API_BASE_DEFAULT=$(python3 "$GITHUB_WORKSPACE/tools/apple/candidate_variants.py" select --manifest "$paired_manifest" --side secondary)
    # The first lossless archive and generated-source hashes have
    # already been preserved outside this generated runner tree.
    rm -rf "$checkout/apps/flutter/ios" "$checkout/apps/flutter/build/ios"
  elif [[ -f "$paired_manifest" ]]; then
    PPVPN_API_BASE_DEFAULT=$(python3 "$GITHUB_WORKSPACE/tools/apple/candidate_variants.py" select --manifest "$paired_manifest" --side candidate --minimum-remaining 3600)
  fi
  export PPVPN_API_BASE_DEFAULT
  output="$RUNNER_TEMP/apple-output/$side"
  mkdir -p "$output"
  if [[ -L "$checkout/target" ]]; then
    [[ "$(readlink "$checkout/target")" == "$CARGO_TARGET_DIR" ]] || exit 2
  else
    [[ ! -e "$checkout/target" ]] || exit 2
    ln -s "$CARGO_TARGET_DIR" "$checkout/target"
  fi
  variant_started=$SECONDS
  (cd "$checkout" && tools/release-features-check.sh --target aarch64-apple-ios -p ppvpn-flutter)
  (cd "$checkout" && FLUTTER_TARGET=ios IOS_SDK=device bash apps/flutter/scripts/ci-build.sh) >"$output/build.log" 2>&1
  archive="$checkout/apps/flutter/build/ios/archive/Runner.xcarchive"
  (cd "$checkout/apps/flutter" && xcodebuild -workspace ios/Runner.xcworkspace -scheme Runner \
    -configuration Release -sdk iphoneos -destination 'generic/platform=iOS' \
    -archivePath "$archive" -derivedDataPath "$RUNNER_TEMP/apple-derived-$side" archive \
    CODE_SIGNING_ALLOWED=NO CODE_SIGNING_REQUIRED=NO CODE_SIGN_IDENTITY= \
    LD_GENERATE_MAP_FILE=YES COMPILER_INDEX_STORE_ENABLE=NO) >"$output/archive.log" 2>&1
  contract_status=0
  python3 "$checkout/apps/flutter/scripts/check-ios-exports.py" \
    "$archive/Products/Applications/Runner.app/Runner" \
    "$archive/Products/Applications/Runner.app/PlugIns/PacketTunnel.appex/PacketTunnel" || contract_status=$?
  tar -czf "$output/Runner.xcarchive.tar.gz" -C "$(dirname "$archive")" Runner.xcarchive
  CHECKOUT="$checkout" OUTPUT="$output" SIDE="$side" BUILD_SECONDS="$((SECONDS-variant_started))" CONTRACT_STATUS="$contract_status" python3 - <<'PY'
from pathlib import Path
import gzip, hashlib, json, os, shutil, subprocess
checkout, output = Path(os.environ['CHECKOUT']), Path(os.environ['OUTPUT'])
archive = output / 'Runner.xcarchive.tar.gz'
runner = checkout / 'apps/flutter/build/ios/archive/Runner.xcarchive/Products/Applications/Runner.app/Runner'
if os.environ['PPVPN_API_BASE_DEFAULT'].encode() not in runner.read_bytes():
    raise SystemExit('actual Runner lacks this variant compiled origin')
generated = output / 'generated-ios-source'
generated.mkdir()
generated_hashes = {}
for name in ('AppDelegate.swift', 'GeneratedPluginRegistrant.h', 'GeneratedPluginRegistrant.m', 'Info.plist'):
    src = checkout / 'apps/flutter/ios/Runner' / name
    if not src.is_file():
        raise SystemExit(f'missing generated iOS source: {name}')
    shutil.copyfile(src, generated / name)
    generated_hashes[name] = hashlib.sha256(src.read_bytes()).hexdigest()
for src in Path(os.environ['RUNNER_TEMP']).joinpath('apple-derived-' + os.environ['SIDE']).rglob('*LinkMap*.txt'):
    with gzip.open(output / (src.name + '.gz'), 'wb') as f:
        f.write(src.read_bytes())
try:
    host_go = subprocess.check_output(['go', 'version'], text=True, stderr=subprocess.DEVNULL).strip()
except (OSError, subprocess.CalledProcessError):
    host_go = None
data = {'source_commit': subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=checkout, text=True).strip(),
        'generated_ios_source_sha256': generated_hashes,
        'cargo_lock_sha256': hashlib.sha256((checkout / 'Cargo.lock').read_bytes()).hexdigest(),
        'backend_origin_sha256': hashlib.sha256(os.environ['PPVPN_API_BASE_DEFAULT'].encode()).hexdigest(),
        'build_number': os.environ['PPVPN_BUILD_NUMBER'], 'architecture': 'arm64',
        'variant': os.environ['SIDE'], 'canonical_build_seconds': int(os.environ['BUILD_SECONDS']),
        'same_checkout_pair': bool(os.environ.get('PAIRED_CANDIDATE_JSON')),
        'host_go': host_go,
        'host_go_scope': 'parent PATH only; null does not assert the build script Go toolchain',
        'xcode': subprocess.check_output(['xcodebuild', '-version'], text=True).strip(),
        'iphoneos_sdk': subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-version'], text=True).strip(),
        'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
        'flutter': json.loads(subprocess.check_output(['flutter', '--version', '--machine'], text=True)),
        'archive_tar_sha256': hashlib.sha256(archive.read_bytes()).hexdigest(),
        'export_contract_check_exit': int(os.environ['CONTRACT_STATUS']),
        'signing': 'unsigned; local provisioning/signing/export verification follows',
        'apns_enabled': False, 'device_install_or_upload': False,
        'build_purpose': 'isolated-candidate-only' if os.environ['CANDIDATE_ONLY'] == '1' else 'same-conditions-size-comparison'}
(output / 'build-provenance.json').write_text(json.dumps(data, indent=2) + '\n')
PY
  [[ "$contract_status" == 0 ]] || exit "$contract_status"
done
