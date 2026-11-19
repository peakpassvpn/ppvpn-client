"""Native cold-start buffering, APNs environment detection and iOS launch wiring (#539)."""
from pathlib import Path
import importlib.util
import plistlib
import shutil
import subprocess
import tempfile
import unittest

FLUTTER = Path(__file__).resolve().parents[1]
spec = importlib.util.spec_from_file_location("install_ios_plugin", FLUTTER / "scripts/install-ios-plugin.py")
installer = importlib.util.module_from_spec(spec)
spec.loader.exec_module(installer)

TEMPLATE = '''import Flutter
import UIKit
@main
@objc class AppDelegate: FlutterAppDelegate, FlutterImplicitEngineDelegate {
  override func application(
    _ application: UIApplication,
    didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
  ) -> Bool {
    return super.application(application, didFinishLaunchingWithOptions: launchOptions)
  }
  func didInitializeImplicitFlutterEngine(_ engineBridge: FlutterImplicitEngineBridge) {
    GeneratedPluginRegistrant.register(with: engineBridge.pluginRegistry)
  }
}
'''


class IOSPushTest(unittest.TestCase):
    def test_installs_receiver_before_launch_returns_and_channel_after_engine(self):
        with tempfile.TemporaryDirectory() as tmp:
            delegate = Path(tmp) / "AppDelegate.swift"
            delegate.write_text(TEMPLATE)
            installer.install(delegate)
            code = delegate.read_text()
            self.assertLess(code.index("PPVPNPush.install()"), code.index("return super.application"))
            self.assertLess(code.index("func didInitializeImplicitFlutterEngine"), code.index("PPVPNPlugin.register(with:"))
            self.assertIn('engineBridge.pluginRegistry.registrar(forPlugin: "PPVPNPlugin")!', code)
            self.assertIn("final class PushInbox", code)
            self.assertEqual(code.count("import UserNotifications"), 1)

    def test_changed_template_fails_without_writing_partial_runner(self):
        with tempfile.TemporaryDirectory() as tmp:
            delegate = Path(tmp) / "AppDelegate.swift"
            source = TEMPLATE.replace("return super.application", "return changed.application")
            delegate.write_text(source)
            with self.assertRaisesRegex(SystemExit, "no didFinishLaunching"):
                installer.install(delegate)
            self.assertEqual(delegate.read_text(), source)

    @unittest.skipUnless(shutil.which("swiftc"), "Swift compiler required for native mailbox regression")
    def test_cold_start_token_rotation_delivery_taps_and_environment_use_actual_swift_mailbox(self):
        with tempfile.TemporaryDirectory() as tmp:
            dev_plist = plistlib.dumps({"Entitlements": {"aps-environment": "development"}})
            prod_plist = plistlib.dumps({"Entitlements": {"aps-environment": "production"}})
            dev_file = Path(tmp) / "dev.mobileprovision"
            prod_file = Path(tmp) / "prod.mobileprovision"
            dev_file.write_bytes(b"\x30\x82CMSHEADER" + dev_plist + b"CMSTRAILER")
            prod_file.write_bytes(b"\x30\x82CMSHEADER" + prod_plist + b"CMSTRAILER")

            main = Path(tmp) / "main.swift"
            main.write_text('''import Foundation
let inbox = PushInbox()
var events: [(String, [String: String])] = []
inbox.receiveToken("0123abcd")
inbox.receiveTap([
  "extra": [
    "delivery_id": "push:77",
    "message_id": 42,
    "account_id": "untrusted-account",
    "deep_link": "file:///ignored",
  ],
  "body": "ignored",
])
inbox.start { events.append(($0, $1)) }
precondition(events.count == 2)
precondition(events[0].0 == "token" && events[0].1 == ["token": "0123abcd"])
precondition(events[1].0 == "opened" && events[1].1 == ["delivery_id": "push:77", "message_id": "42"])
inbox.receiveToken("4567ef00")
precondition(events.last!.1 == ["token": "4567ef00"])
inbox.receiveTap(["delivery_id": "push:88"])
precondition(events.last!.1 == ["delivery_id": "push:88"])
inbox.receiveTap(["body": "untrusted", "deep_link": "https://ignored.test", "account_id": "u1"])
precondition(events.last!.0 == "opened" && events.last!.1.isEmpty)

let devData = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[1]))
let prodData = try Data(contentsOf: URL(fileURLWithPath: CommandLine.arguments[2]))
precondition(PushInbox.apnsEnvironment(provisionData: devData, simulator: false) == "sandbox")
precondition(PushInbox.apnsEnvironment(provisionData: prodData, simulator: false) == "production")
precondition(PushInbox.apnsEnvironment(provisionData: nil, simulator: false) == "production")
precondition(PushInbox.apnsEnvironment(provisionData: nil, simulator: true) == "sandbox")
print("iOS native mailbox: cold start, rotation, delivery_id filtering and APNs environment passed")
''')
            binary = Path(tmp) / "check"
            subprocess.run(["swiftc", str(FLUTTER / "runner/ios/Runner/PushInbox.swift"), str(main), "-o", str(binary)], check=True)
            subprocess.run([str(binary), str(dev_file), str(prod_file)], check=True)


if __name__ == "__main__":
    unittest.main()
