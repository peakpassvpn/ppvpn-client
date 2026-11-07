#!/usr/bin/env python3
"""Real Flutter template transformation regressions and Apple SDK object compilation.

No engine/app launch, notification registration, entitlement or network change.
"""
import argparse
import importlib.util
import os
from pathlib import Path
import shutil
import subprocess
import tempfile
import unittest

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent
spec = importlib.util.spec_from_file_location('mac_hooks', HERE / 'macos_push_hooks.py')
hooks = importlib.util.module_from_spec(spec)
spec.loader.exec_module(hooks)
FLUTTER = None
SDK_CHECK = False


class TemplateTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory(prefix='ppvpn-mac-push-hooks-')
        self.addCleanup(self.temp.cleanup)
        self.work = Path(self.temp.name)
        self.app = ROOT / 'apps/flutter'
        self.generated = self.work / 'macos'
        self.runner = self.generated / 'Runner'
        self.runner.mkdir(parents=True)
        self.delegate = self.runner / 'AppDelegate.swift'
        self.window = self.runner / 'MainFlutterWindow.swift'
        template = FLUTTER / 'packages/flutter_tools/templates/app/macos.tmpl/Runner/AppDelegate.swift'
        self.delegate.write_text(template.read_text())
        self.window.write_text((self.app / 'runner/macos/Runner/MainFlutterWindow.swift').read_text())

    def unchanged_failure(self, error=hooks.HookError):
        before = {p: p.read_bytes() for p in self.runner.iterdir() if p.is_file()}
        with self.assertRaises(error):
            hooks.wire(self.app, self.generated)
        self.assertEqual(before, {p: p.read_bytes() for p in self.runner.iterdir() if p.is_file()})

    def test_real_template_is_idempotent_and_all_native_sources_are_included(self):
        for source in ['runner/apple/ApplePushState.swift', 'runner/apple/ApplePushInstallation.swift',
                       'runner/macos/Runner/ApplePush.swift', 'runner/macos/Runner/ApplePushBridge.swift']:
            shutil.copy2(self.app / source, self.runner / Path(source).name)
        result = hooks.wire(self.app, self.generated)
        self.assertEqual(result, {'delegate_hooks': 5, 'source_files': 4, 'rollout_changed': False})
        first = self.delegate.read_bytes(), self.window.read_bytes()
        hooks.wire(self.app, self.generated)
        self.assertEqual(first, (self.delegate.read_bytes(), self.window.read_bytes()))
        self.assertEqual(len(list(self.runner.iterdir())), 2)
        for name in ['ApplePushState', 'ApplePushInstallation', 'MacApplePushReceiver', 'MacApplePushBridge']:
            self.assertIn(name, self.window.read_text())
        self.assertIn('applicationShouldTerminateAfterLastWindowClosed', self.delegate.read_text())
        self.assertIn('applicationSupportsSecureRestorableState', self.delegate.read_text())
        # The old Agent/service/updater code must survive the source conversion.
        for text in ['pushAgent.start()', 'pushAgent?.prepareForUpdate()', 'loginItem = LoginItem']:
            self.assertIn(text, self.window.read_text())

    def test_upgrade_of_previous_push_config_source_block(self):
        with self.window.open('a') as f:
            f.write('\n' + hooks.SOURCE_BEGIN + '\n')
            f.write((self.app / 'runner/apple/ApplePushState.swift').read_text())
            f.write((self.app / 'runner/macos/Runner/ApplePush.swift').read_text())
            f.write('\n' + hooks.SOURCE_END + '\n')
        hooks.wire(self.app, self.generated)
        self.assertEqual(self.window.read_text().count('final class MacApplePushReceiver'), 1)
        self.assertEqual(self.window.read_text().count('final class MacApplePushBridge'), 1)

    def test_old_window_without_attach_is_rejected(self):
        self.window.write_text(self.window.read_text().replace('applePush.attach(', 'missing.attach('))
        self.unchanged_failure()

    def test_late_delegate_preparation_is_rejected(self):
        self.window.write_text(self.window.read_text().replace('applePush.prepareForLaunch()', '/* omitted */'))
        self.unchanged_failure()

    def test_existing_init_is_not_overwritten(self):
        self.delegate.write_text(self.delegate.read_text().replace('class AppDelegate: FlutterAppDelegate {', 'class AppDelegate: FlutterAppDelegate {\n  override init() { super.init() }'))
        self.unchanged_failure()

    def test_existing_native_callback_is_not_overwritten(self):
        self.delegate.write_text(self.delegate.read_text().replace('class AppDelegate: FlutterAppDelegate {', 'class AppDelegate: FlutterAppDelegate {\n  func application(_ app: NSApplication, didReceiveRemoteNotification info: [String: Any]) {}'))
        self.unchanged_failure()

    def test_unknown_delegate_template_is_rejected(self):
        self.delegate.write_text(self.delegate.read_text().replace('FlutterAppDelegate', 'NSObject'))
        self.unchanged_failure()

    def test_partial_generated_marker_is_rejected(self):
        self.window.write_text(self.window.read_text() + '\n' + hooks.SOURCE_BEGIN)
        self.unchanged_failure()

    def test_unmanaged_duplicate_source_is_rejected(self):
        self.window.write_text(self.window.read_text() + '\nclass MacApplePushBridge {}\n')
        self.unchanged_failure()

    def test_modified_copy_is_preserved_and_rejected(self):
        (self.runner / 'ApplePushBridge.swift').write_text('// unrelated user modification')
        self.unchanged_failure()

    def test_owned_template_path_is_refused(self):
        with self.assertRaises(hooks.HookError):
            hooks.wire(self.app, self.app / 'runner/macos')

    def test_real_transformed_template_against_macos_sdk(self):
        if not SDK_CHECK:
            self.skipTest('pass --sdk to compile with actual macOS SDK and Flutter framework')
        hooks.wire(self.app, self.generated)
        stub = self.work / 'GeneratedPluginRegistrant.swift'
        stub.write_text('import FlutterMacOS\nfunc RegisterGeneratedPlugins(registry: FlutterPluginRegistry) {}\n')
        framework = FLUTTER / 'bin/cache/artifacts/engine/darwin-x64/FlutterMacOS.xcframework/macos-arm64_x86_64'
        subprocess.run(['xcrun', 'swiftc', '-emit-object', '-whole-module-optimization', '-parse-as-library', '-swift-version', '5',
                        '-warnings-as-errors', '-target', 'arm64-apple-macos13.0', '-sdk', subprocess.check_output(['xcrun', '--sdk', 'macosx', '--show-sdk-path'], text=True).strip(),
                        '-module-cache-path', str(self.work / 'modules'), '-F', str(framework),
                        str(self.delegate), str(self.window), str(stub), '-o', str(self.work / 'hooks.o')], check=True, timeout=120)


if __name__ == '__main__':
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--flutter-root', type=Path)
    parser.add_argument('--sdk', action='store_true')
    args, remaining = parser.parse_known_args()
    executable = shutil.which('flutter')
    FLUTTER = args.flutter_root or (Path(os.environ['FLUTTER_ROOT']) if 'FLUTTER_ROOT' in os.environ else None)
    if FLUTTER is None and executable:
        FLUTTER = Path(executable).resolve().parent.parent
    if FLUTTER is None:
        parser.error('supply --flutter-root or FLUTTER_ROOT for the actual template')
    SDK_CHECK = args.sdk
    unittest.main(argv=[__file__, *remaining])
