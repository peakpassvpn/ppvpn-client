#!/usr/bin/env python3
"""Controlled iOS method-body runtime tests and unchanged iOS SDK object compile.

Never starts UIKit/Flutter/APNs. The runtime test swaps only imports/registrar
access for Mac fixture boundaries; the real SDK compilation uses original files.
"""
import argparse
from pathlib import Path
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parents[2]
APP = ROOT / 'apps/flutter'
parser = argparse.ArgumentParser(description=__doc__)
parser.add_argument('--flutter-root', type=Path)
parser.add_argument('--negative-controls', action='store_true')
args = parser.parse_args()


def run(work, common, receiver, bridge):
    r = work / 'IOSReceiver.swift'
    b = work / 'IOSBridge.swift'
    r.write_text(receiver.replace('import UIKit', 'import Cocoa'))
    b.write_text(bridge.replace('import Flutter\n', '').replace('import UIKit', 'import Cocoa').replace('registrar.messenger()', 'registrar.messenger'))
    exe = work / 'tests'
    subprocess.run(['xcrun', 'swiftc', '-swift-version', '5', '-warnings-as-errors',
                    '-target', 'arm64-apple-macos13.0', '-module-cache-path', str(work / 'modules'),
                    str(common), str(r), str(APP / 'checks/apple/MacPushFixture.swift'), str(b),
                    str(APP / 'checks/apple/IOSPushTraceTests.swift'), '-o', str(exe)], check=True, timeout=120)
    return subprocess.run([str(exe)], capture_output=True, text=True, timeout=30)


with tempfile.TemporaryDirectory(prefix='ppvpn-ios-trace-') as temp:
    work = Path(temp)
    common = APP / 'runner/apple/ApplePushState.swift'
    receiver = (APP / 'runner/ios/Runner/ApplePush.swift').read_text()
    bridge = (APP / 'runner/ios/Runner/ApplePushBridge.swift').read_text()
    result = run(work, common, receiver, bridge)
    if result.returncode:
        raise RuntimeError(result.stdout + result.stderr)
    print(result.stdout.strip())
    if args.negative_controls:
        variants = [
            ('off', receiver.replace('guard configured else { return false }', '/* removed OFF admission */'), bridge, common,
             'iOS OFF system/trace/store effects'),
            ('epoch', receiver, bridge.replace('let current = self?.clientEpoch == epoch', 'let current = self?.clientEpoch != nil && !epoch.isEmpty'), common,
             'iOS stale epoch ack was accepted'),
        ]
        bad_common = work / 'LeakingTrace.swift'
        original = common.read_text()
        bad_common.write_text(original.replace('domain=\\(failureDomain.rawValue)', 'domain=SECRET_DOMAIN'))
        variants.append(('privacy', receiver, bridge, bad_common, 'trace schema/privacy boundary'))
        for name, r, b, c, expected in variants:
            result = run(work, c, r, b)
            if result.returncode == 0 or expected not in result.stdout + result.stderr:
                raise RuntimeError('negative control missed expected semantic assertion: ' + name)
            print(name + ' negative rejected by expected assertion')
    if args.flutter_root:
        framework = args.flutter_root / 'bin/cache/artifacts/engine/ios-release/Flutter.xcframework/ios-arm64'
        sdk = subprocess.check_output(['xcrun', '--sdk', 'iphoneos', '--show-sdk-path'], text=True).strip()
        subprocess.run(['xcrun', 'swiftc', '-emit-object', '-whole-module-optimization', '-parse-as-library',
                        '-swift-version', '5', '-warnings-as-errors', '-target', 'arm64-apple-ios15.0',
                        '-sdk', sdk, '-module-cache-path', str(work / 'ios-modules'), '-F', str(framework),
                        str(common), str(APP / 'runner/apple/ApplePushInstallation.swift'),
                        str(APP / 'runner/ios/Runner/ApplePush.swift'), str(APP / 'runner/ios/Runner/ApplePushBridge.swift'),
                        '-o', str(work / 'push.o')], check=True, timeout=120)
        print('Original iOS state/proof/receiver/bridge SDK object compilation passed')
