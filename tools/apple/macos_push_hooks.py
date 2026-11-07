#!/usr/bin/env python3
"""Wire generated macOS AppDelegate and include all owned Apple push sources.

Run after copying runner templates AND push_config.py sources, before compiling.
No plist, entitlement, signing, permission, registration or app launch operations.
"""
import argparse
from pathlib import Path
import re

SOURCE_BEGIN = '// BEGIN PPVPN GENERATED APPLE PUSH SOURCES'
SOURCE_END = '// END PPVPN GENERATED APPLE PUSH SOURCES'
HOOK_BEGIN = '  // BEGIN PPVPN APPLE PUSH DELEGATE HOOKS'
HOOK_END = '  // END PPVPN APPLE PUSH DELEGATE HOOKS'


class HookError(ValueError):
    pass


def remove_block(text, begin, end):
    if text.count(begin) != text.count(end) or text.count(begin) > 1:
        raise HookError('missing or duplicate generated push markers')
    if begin not in text:
        return text
    before, _, rest = text.partition(begin)
    if end not in rest:
        raise HookError('reversed generated push markers')
    _, _, after = rest.partition(end)
    return before.rstrip() + '\n' + after.lstrip('\n')


def delegate_hooks():
    return '''  // BEGIN PPVPN APPLE PUSH DELEGATE HOOKS
  override init() {
    super.init()
    // Install the weak UN delegate's strong process owner before Flutter/nibs.
    MainActor.assumeIsolated { MacApplePushBridge.shared.prepareForLaunch() }
  }

  override func applicationWillFinishLaunching(_ notification: Notification) {
    MacApplePushBridge.shared.start()
    super.applicationWillFinishLaunching(notification)
  }

  override func application(_ application: NSApplication, didRegisterForRemoteNotificationsWithDeviceToken token: Data) {
    MacApplePushBridge.shared.receiver.didRegister(token: token)
    // Flutter exposes optional NSApplicationDelegate methods in Swift even
    // when its base class has no implementation. Never message an absent super.
    if FlutterAppDelegate.instancesRespond(to: #selector(NSApplicationDelegate.application(_:didRegisterForRemoteNotificationsWithDeviceToken:))) {
      super.application(application, didRegisterForRemoteNotificationsWithDeviceToken: token)
    }
  }

  override func application(_ application: NSApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
    MacApplePushBridge.shared.receiver.didFailToRegister(error)
    if FlutterAppDelegate.instancesRespond(to: #selector(NSApplicationDelegate.application(_:didFailToRegisterForRemoteNotificationsWithError:))) {
      super.application(application, didFailToRegisterForRemoteNotificationsWithError: error)
    }
  }

  override func application(_ application: NSApplication, didReceiveRemoteNotification userInfo: [String: Any]) {
    MacApplePushBridge.shared.receiver.receiveBackground(userInfo)
    if FlutterAppDelegate.instancesRespond(to: #selector(NSApplicationDelegate.application(_:didReceiveRemoteNotification:))) {
      super.application(application, didReceiveRemoteNotification: userInfo)
    }
  }
  // END PPVPN APPLE PUSH DELEGATE HOOKS
'''


def wire(app_root, generated_root):
    app_root, generated_root = app_root.resolve(), generated_root.resolve()
    delegate = (generated_root / 'Runner/AppDelegate.swift').resolve()
    window = (generated_root / 'Runner/MainFlutterWindow.swift').resolve()
    for path in (delegate, window):
        if path.is_relative_to(app_root / 'runner'):
            raise HookError('refusing to rewrite owned runner templates')
        if not path.is_relative_to(generated_root):
            raise HookError('generated runner escapes its root')
    source_paths = [app_root / 'runner/apple/ApplePushState.swift',
                    app_root / 'runner/apple/ApplePushInstallation.swift',
                    app_root / 'runner/macos/Runner/ApplePush.swift',
                    app_root / 'runner/macos/Runner/ApplePushBridge.swift']
    sources = [path.read_text() for path in source_paths]
    app = remove_block(delegate.read_text(), HOOK_BEGIN, HOOK_END)
    main = remove_block(window.read_text(), SOURCE_BEGIN, SOURCE_END)
    match = list(re.finditer(r'^\s*(?:final\s+)?class AppDelegate:\s*FlutterAppDelegate\s*\{', app, re.M))
    if len(match) != 1:
        raise HookError('expected one supported Flutter AppDelegate template')
    if re.search(r'\binit\s*\(|\bapplicationWillFinishLaunching\s*\(|\bdidRegisterForRemoteNotificationsWithDeviceToken\b|\bdidFailToRegisterForRemoteNotificationsWithError\b|\bdidReceiveRemoteNotification\b', app):
        raise HookError('existing native launch/APNs hooks require explicit reconciliation')
    for declaration in ('ApplePushState', 'ApplePushInstallation', 'MacApplePushReceiver', 'MacApplePushBridge'):
        if re.search(r'\b(?:class|struct)\s+' + declaration + r'\b', main):
            raise HookError('unmanaged duplicate push source in generated window')
    before_engine = main.find('applePush.prepareForLaunch()')
    engine = main.find('let flutterViewController = FlutterViewController()')
    if before_engine < 0 or engine < before_engine:
        raise HookError('window must prepare push before Flutter creation')
    required = ['let applePush = MacApplePushBridge.shared',
                'RegisterGeneratedPlugins(registry: flutterViewController)',
                'applePush.showWindow = { [weak self] in',
                'applePush.attach(flutterViewController.registrar(forPlugin: "MacApplePushBridge"))']
    if any(main.count(value) != 1 for value in required):
        raise HookError('window lacks unique registrar/weak activation hooks')
    if not (main.index(required[1]) < main.index(required[2]) < main.index(required[3])):
        raise HookError('window must install activation before attaching after plugins')
    copies = []
    for source, text in zip(source_paths, sources):
        copied = generated_root / 'Runner' / source.name
        if copied.exists():
            if copied.resolve() == source.resolve() or copied.is_symlink() or copied.read_text() != text:
                raise HookError('copied push source differs; refusing to discard it')
            copies.append(copied)
    # Validate everything above before changing either generated Swift file.
    end = match[0].end()
    app = app[:end] + '\n' + delegate_hooks() + app[end:]
    main = main.rstrip() + '\n\n' + SOURCE_BEGIN + '\n' + '\n'.join(sources) + '\n' + SOURCE_END + '\n'
    delegate.write_text(app)
    window.write_text(main)
    for copied in copies:
        copied.unlink()
    return {'delegate_hooks': 5, 'source_files': len(sources), 'rollout_changed': False}


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--app-root', type=Path, required=True)
    parser.add_argument('--generated-root', type=Path, required=True)
    args = parser.parse_args()
    try:
        result = wire(args.app_root, args.generated_root)
    except (HookError, OSError) as error:
        parser.exit(2, f'macOS push hooks: {error}\n')
    print(f'macOS push hooks: {result["delegate_hooks"]} hooks, {result["source_files"]} sources; rollout unchanged')


if __name__ == '__main__':
    main()
