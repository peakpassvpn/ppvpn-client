"""Execute the real archive shell with tiny fake build tools; no native build/run."""
import json
import os
from pathlib import Path
import shutil
import subprocess
import tarfile
import tempfile
import time
import unittest

from candidate_variants import plan


@unittest.skipUnless(shutil.which('ruby') and shutil.which('bash'), 'requires workflow shell/YAML reader')
class PairedArchiveShell(unittest.TestCase):
    def execute(self, stale=False, expired=False):
        repo = Path(__file__).resolve().parents[2]
        workflow = json.loads(subprocess.check_output(
            ['ruby', '-rjson', '-ryaml', '-e', 'puts JSON.generate(YAML.load_file(ARGV[0]))',
             str(repo / '.github/workflows/apple-archive-acceptance.yml')]))
        script = next(s['run'] for s in workflow['jobs']['archives']['steps']
                      if s.get('name') == 'Serial Release archives on one compiler and SDK')
        with tempfile.TemporaryDirectory(prefix='ppvpn-paired-controller-') as directory:
            work = Path(directory)
            source, runner, target, bins = [work / n for n in ['source', 'runner', 'target', 'bin']]
            for p in [source, runner, target, bins]:
                p.mkdir()
            def write(path, text):
                path.parent.mkdir(parents=True, exist_ok=True)
                path.write_text(text)
                path.chmod(0o755)
            write(source / 'tools/release-features-check.sh', '#!/bin/sh\nexit 0\n')
            write(source / 'apps/flutter/scripts/check-ios-exports.py', '#!/usr/bin/env python3\n')
            write(source / 'tools/apple/candidate_variants.py', (repo / 'tools/apple/candidate_variants.py').read_text())
            write(source / 'Cargo.lock', '# fixture native inputs remain unchanged\n')
            write(source / 'runtime.rs', '// retained native source sentinel\n')
            write(source / 'apps/flutter/scripts/ci-build.sh',
                  '#!/bin/bash\nexec python3 "$(dirname "$0")/fixture-build.py"\n')
            write(source / 'apps/flutter/scripts/fixture-build.py', '''from pathlib import Path
import os
p=Path('apps/flutter')
assert not (p/'ios').exists()
assert Path('runtime.rs').read_text()=='// retained native source sentinel\\n'
cache=Path(os.environ['CARGO_TARGET_DIR'])/'fixture-cache'
old=cache.read_text() if cache.exists() else None
origin=old if old and os.environ.get('FIXTURE_STALE_ORIGIN') else os.environ['PPVPN_API_BASE_DEFAULT']
cache.write_text(os.environ['PPVPN_API_BASE_DEFAULT'])
out=p/'build/ios/archive/Runner.xcarchive/Products/Applications/Runner.app';out.mkdir(parents=True)
(out/'Runner').write_text(origin)
g=p/'ios/Runner';g.mkdir(parents=True)
for name in ['AppDelegate.swift','GeneratedPluginRegistrant.h','GeneratedPluginRegistrant.m','Info.plist']:(g/name).write_text(name)
with (Path(os.environ['RUNNER_TEMP'])/'build-calls').open('a') as f:f.write(os.environ['PPVPN_API_BASE_DEFAULT']+'\\n')
''')
            for name, output in [('xcodebuild', 'Xcode fixture'), ('rustc', 'rustc fixture'),
                                 ('go', 'go fixture'), ('xcrun', 'fixture SDK'),
                                 ('flutter', '{"frameworkVersion":"fixture"}')]:
                write(bins / name, '#!/bin/sh\nprintf \'%s\\n\' \'' + output + '\'\n')
            for command in [['git', 'init', '-q'], ['git', 'add', '.'],
                            ['git', '-c', 'user.name=Fixture', '-c', 'user.email=fixture@example.invalid',
                             'commit', '-qm', 'controller fixture']]:
                subprocess.run(command, cwd=source, check=True, capture_output=True)
            ref = subprocess.check_output(['git', 'rev-parse', 'HEAD'], cwd=source, text=True).strip()
            primary, secondary = 'https://controlled.example', 'https://www.peakpass.dev'
            config = {'origin': secondary, 'primary_expires_unix': time.time() + 7200}
            if expired:
                config['secondary_expires_unix'] = time.time() + 1700
            manifest = plan(json.dumps(config), primary, True, 'ios', '')
            (runner / 'apple-pair.json').write_text(json.dumps(manifest))
            env = {**os.environ, 'PATH': str(bins) + os.pathsep + os.environ['PATH'],
                   'GITHUB_WORKSPACE': str(source), 'RUNNER_TEMP': str(runner), 'CARGO_TARGET_DIR': str(target),
                   'SOURCE_REF': ref, 'BASELINE_REF': ref, 'CANDIDATE_ONLY': '1', 'REUSE_BASELINE_RUN': '',
                   'PPVPN_API_BASE_DEFAULT': primary, 'PPVPN_BUILD_NUMBER': '345',
                   'PAIRED_CANDIDATE_JSON': json.dumps(config)}
            if stale:
                env['FIXTURE_STALE_ORIGIN'] = '1'
            result = subprocess.run(['bash', '-c', script], cwd=source, env=env, capture_output=True, text=True)
            first = runner / 'apple-output/candidate/Runner.xcarchive.tar.gz'
            self.assertTrue(first.is_file(), result.stderr)
            with tarfile.open(first) as archive:
                data = archive.extractfile('Runner.xcarchive/Products/Applications/Runner.app/Runner').read()
            self.assertEqual(data.decode(), primary)
            calls = (runner / 'build-calls').read_text().splitlines()
            second = runner / 'apple-output/secondary/build-provenance.json'
            if stale or expired:
                self.assertNotEqual(result.returncode, 0)
                self.assertFalse(second.exists())
                self.assertIn('compiled origin' if stale else 'remaining lifetime', result.stderr)
            else:
                self.assertEqual(result.returncode, 0, result.stderr)
                proof = json.loads(second.read_text())
                self.assertEqual(proof['source_commit'], ref)
                self.assertEqual(proof['variant'], 'secondary')
                self.assertTrue(proof['same_checkout_pair'])
            self.assertEqual(calls, [primary] if expired else [primary, secondary])

    def test_canonical_pair_preserves_first_archive_and_uses_shared_target(self):
        self.execute()

    def test_stale_native_origin_cannot_produce_second_provenance(self):
        self.execute(stale=True)

    def test_expired_second_origin_stops_before_build_and_preserves_first(self):
        self.execute(expired=True)


if __name__ == '__main__':
    unittest.main()
