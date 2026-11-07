#!/usr/bin/env python3
"""Compile an UNSIGNED diagnostic .app with exact production Mac receiver/state.

No Flutter/core/auth/store/worker/permission request. Builder owns profile
preparation, Developer ID signing, verification, workflow and real execution.
"""
import argparse
import hashlib
import json
from pathlib import Path
import plistlib
import subprocess
import tempfile

KIND = 'native_apns_os_registration_capability_only'
DELEGATE = '''import Cocoa

// Diagnostic host only: forwards real NSApplicationDelegate callbacks to the
// unchanged production receiver. No tokens or errors are printed by the host.
@MainActor
final class CapabilityDelegate: NSObject, NSApplicationDelegate {
  let receiver = MacApplePushReceiver()
  override init() {
    super.init()
    receiver.prepareForLaunch()
  }
  func applicationWillFinishLaunching(_ notification: Notification) { receiver.start() }
  func application(_ application: NSApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
    receiver.didRegister(token: token)
  }
  func application(_ application: NSApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
    receiver.didFailToRegister(error)
  }
}

@main
struct CapabilityMain {
  @MainActor static func main() {
    let application = NSApplication.shared
    let delegate = CapabilityDelegate()
    application.delegate = delegate
    application.setActivationPolicy(.regular)
    withExtendedLifetime(delegate) { application.run() }
  }
}
'''


def digest(path):
    return hashlib.sha256(path.read_bytes()).hexdigest()


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--source-root', type=Path, default=Path(__file__).resolve().parents[2])
    parser.add_argument('--output', type=Path, required=True, help='new private directory')
    parser.add_argument('--state-sha256', required=True)
    parser.add_argument('--receiver-sha256', required=True)
    parser.add_argument('--build-number', required=True)
    parser.add_argument('--arch', choices=['arm64', 'x86_64'], required=True)
    args = parser.parse_args()
    if not args.build_number.isascii() or not args.build_number.isdigit():
        parser.error('build number must be numeric')
    source = args.source_root.resolve(strict=True)
    paths = [source / 'apps/flutter/runner/apple/ApplePushState.swift',
             source / 'apps/flutter/runner/macos/Runner/ApplePush.swift']
    expected = [args.state_sha256, args.receiver_sha256]
    for path, value in zip(paths, expected):
        if len(value) != 64 or digest(path) != value:
            parser.error('production source does not match reviewed frozen hash')
    args.output.mkdir(mode=0o700, parents=False, exist_ok=False)
    app = args.output / 'PPVPN APNs Capability Probe.app'
    binary = app / 'Contents/MacOS/PPVPNAPNsProbe'
    binary.parent.mkdir(parents=True)
    resources = app / 'Contents/Resources'
    resources.mkdir()
    # Push prepare must set the true environment/flag from the actual profile
    # BEFORE signing. An unsigned build remains OFF and cannot request a token.
    info = {'CFBundleIdentifier': 'com.peakpassvpn.ppvpn.desktop',
            'CFBundleExecutable': binary.name, 'CFBundleName': 'PPVPN APNs Capability Probe',
            'CFBundleDisplayName': 'PPVPN APNs Capability Probe', 'CFBundlePackageType': 'APPL',
            'CFBundleVersion': args.build_number, 'CFBundleShortVersionString': '0.7.4',
            'LSMinimumSystemVersion': '13.0', 'NSPrincipalClass': 'NSApplication',
            'PPVPNApplePushEnabled': False, 'PPVPNAPNsCapabilityProbe': True}
    (app / 'Contents/Info.plist').write_bytes(plistlib.dumps(info))
    entitlements = args.output / 'probe.entitlements'
    entitlements.write_bytes(plistlib.dumps({}))
    with tempfile.TemporaryDirectory(prefix='ppvpn-apns-capability-compile-') as temp:
        work = Path(temp)
        delegate = work / 'CapabilityMain.swift'
        delegate.write_text(DELEGATE)
        sdk = subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip()
        subprocess.run(['xcrun', 'swiftc', '-swift-version', '5', '-warnings-as-errors', '-parse-as-library',
                        '-target', args.arch + '-apple-macos13.0', '-sdk', sdk,
                        '-module-cache-path', str(work / 'modules'), *map(str, paths), str(delegate),
                        '-o', str(binary)], check=True, timeout=120)
    manifest = {'kind': KIND, 'source_sha256': {str(p.relative_to(source)): digest(p) for p in paths},
                'host_source_sha256': hashlib.sha256(DELEGATE.encode()).hexdigest(),
                'architecture': args.arch, 'signature': 'UNSIGNED; builder must prepare/sign/verify',
                'capabilities': ['real_NSApplication_APNs_registration', 'production_receiver_safe_OSLog_receipts'],
                'not_covered': ['Flutter_bridge', 'backend_registration', 'permission_UI', 'real_push_delivery',
                                'foreground_background_cold_click', 'VPN', 'legacy_Agent_coexistence']}
    (resources / 'APNsCapabilityProbe.json').write_text(json.dumps(manifest, indent=2) + '\n')
    (args.output / 'probe-build.json').write_text(json.dumps(manifest, indent=2) + '\n')
    print('Unsigned native capability probe compiled; NO launch/signing/token request performed')


if __name__ == '__main__':
    main()
