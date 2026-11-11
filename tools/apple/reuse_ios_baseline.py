#!/usr/bin/env python3
"""Reuse an immutable unsigned baseline only with matching measured build inputs."""
import argparse
import hashlib
import json
from pathlib import Path
import shutil
import subprocess

FLUTTER_KEYS = ('frameworkVersion', 'frameworkRevision', 'engineRevision', 'dartSdkVersion')


def validate(saved, actual, source, build, origin_hash, cargo_hash, archive_hash):
    expected = {'source_commit': source, 'build_number': build,
                'backend_origin_sha256': origin_hash, 'cargo_lock_sha256': cargo_hash,
                'archive_tar_sha256': archive_hash, 'architecture': 'arm64',
                'export_contract_check_exit': 0, 'apns_enabled': False}
    for key, value in expected.items():
        if saved.get(key) != value:
            raise ValueError('baseline provenance mismatch: ' + key)
    if not saved.get('signing', '').startswith('unsigned;'):
        raise ValueError('baseline is not an unsigned archive')
    for key in ('xcode', 'iphoneos_sdk', 'rust'):
        if saved.get(key) != actual[key]:
            raise ValueError('baseline toolchain mismatch: ' + key)
    for key in FLUTTER_KEYS:
        if not saved.get('flutter', {}).get(key) or saved['flutter'][key] != actual['flutter'].get(key):
            raise ValueError('baseline Flutter mismatch: ' + key)


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--download', type=Path, required=True)
    p.add_argument('--output', type=Path, required=True)
    p.add_argument('--repo', type=Path, required=True)
    p.add_argument('--source', required=True)
    p.add_argument('--build', required=True)
    p.add_argument('--origin-hash', required=True)
    p.add_argument('--run', required=True)
    a = p.parse_args()
    base = a.download / 'baseline'
    saved = json.loads((base / 'build-provenance.json').read_text())
    archive = base / 'Runner.xcarchive.tar.gz'
    actual = {'xcode': subprocess.check_output(['xcodebuild', '-version'], text=True).strip(),
              'iphoneos_sdk': subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-version'], text=True).strip(),
              'rust': subprocess.check_output(['rustc', '--version'], text=True).strip(),
              'flutter': json.loads(subprocess.check_output(['flutter', '--version', '--machine'], text=True))}
    lock = subprocess.check_output(['git', 'show', a.source + ':Cargo.lock'], cwd=a.repo)
    digest = hashlib.file_digest(archive.open('rb'), 'sha256').hexdigest()
    validate(saved, actual, a.source, a.build, a.origin_hash, hashlib.sha256(lock).hexdigest(), digest)
    if a.output.exists():
        raise ValueError('baseline reuse output must be new')
    a.output.mkdir(parents=True)
    # Keep the original artifact immutable and referenced. Do not upload a
    # second copy of its archive; the original run and SHA identify it.
    shutil.copyfile(base / 'build-provenance.json', a.output / 'build-provenance.json')
    result = {'original_run': a.run, 'original_artifact': 'ios-unsigned-archive-comparison',
              'source_commit': a.source, 'archive_tar_sha256': digest,
              'compiler_sdk_flutter_origin_build_and_lock_verified': True,
              'archive_recompiled': False, 'archive_reuploaded': False}
    (a.output / 'reuse-verification.json').write_text(json.dumps(result, indent=2) + '\n')
    print(json.dumps(result))


if __name__ == '__main__':
    main()
