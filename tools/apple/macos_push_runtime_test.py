#!/usr/bin/env python3
"""Run real Mac receiver/bridge bodies with fake system/messenger/store edges.

Only the Flutter import is replaced. Cocoa/UN adapters are real compiled source;
all system effects are injected, and no app/Flutter engine/store/APNs is started.
"""
import argparse
from pathlib import Path
import subprocess
import sys
import tempfile

root = Path(__file__).resolve().parents[2]
app = root / 'apps/flutter'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--receiver', type=Path, default=app / 'runner/macos/Runner/ApplePush.swift')
parser.add_argument('--bridge', type=Path, default=app / 'runner/macos/Runner/ApplePushBridge.swift')
parser.add_argument('--negative-controls', action='store_true')
args = parser.parse_args()
with tempfile.TemporaryDirectory(prefix='ppvpn-mac-push-runtime-') as directory:
    work = Path(directory)
    bridge = work / 'MacBridge.swift'
    source = args.bridge.read_text()
    bridge.write_text(source.replace('import FlutterMacOS\n', ''))
    executable = work / 'tests'
    subprocess.run(['xcrun', 'swiftc', '-swift-version', '5', '-target', 'arm64-apple-macos13.0',
                    '-module-cache-path', str(work / 'modules'),
                    str(app / 'runner/apple/ApplePushState.swift'),
                    str(args.receiver),
                    str(app / 'checks/apple/MacPushFixture.swift'), str(bridge),
                    str(app / 'checks/apple/MacPushHooksTests.swift'), '-o', str(executable)],
                   check=True, timeout=120)
    subprocess.run([str(executable)], check=True, timeout=30)

if args.negative_controls:
    with tempfile.TemporaryDirectory(prefix='ppvpn-mac-push-negatives-') as directory:
        work = Path(directory)
        variants = [
            ('off', '--receiver', args.receiver,
             'func prepareForLaunch() {\n    guard configured else { return }',
             'func prepareForLaunch() {',
             'disabled/missing capability caused system or store side effects'),
            ('epoch', '--bridge', args.bridge,
             'let current = self?.clientEpoch == epoch',
             'let current = self?.clientEpoch != nil && !epoch.isEmpty',
             'late old epoch activated replacement account window'),
        ]
        for name, flag, source, old, new, message in variants:
            text = source.read_text()
            if text.count(old) != 1:
                raise RuntimeError('negative-control source guard changed: ' + name)
            bad = work / (name + '.swift')
            bad.write_text(text.replace(old, new, 1))
            result = subprocess.run([sys.executable, __file__, flag, str(bad)],
                                    capture_output=True, text=True, timeout=150)
            if result.returncode == 0 or message not in result.stdout + result.stderr:
                raise RuntimeError('negative control did not hit semantic assertion: ' + name)
            print(name + ' guard-removal negative rejected by expected runtime assertion')
