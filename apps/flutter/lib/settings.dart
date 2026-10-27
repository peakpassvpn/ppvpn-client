// The app's own settings (#401 step 3): appearance, language and the API
// override, kept by the bridge in settings.json in the data directory
// (rust/src/settings.rs). The routing mode is the client's and comes with
// the snapshot. Theme (Tokens.of) and strings (S.current) follow the
// notifiers here; main.dart rebuilds the app when they change. Launch at
// login (#401 step 4) is the system's, read back and never saved here:
// rust/src/autostart.rs on Windows and Linux, the runner's LoginItem
// (MainFlutterWindow.swift, channel ppvpn/login-item) on macOS.

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'src/rust/api.dart' as rust;

const _loginItem = MethodChannel('ppvpn/login-item');

class Settings {
  Settings._();

  static final ValueNotifier<rust.UiAppearance> appearance = ValueNotifier(rust.UiAppearance.system);
  static final ValueNotifier<rust.UiLanguage> language = ValueNotifier(rust.UiLanguage.system);

  /// As saved; the client read it when it started (it takes effect on the
  /// next launch).
  static final ValueNotifier<String> apiOverride = ValueNotifier('');

  /// As the system has it; [refreshLaunchAtLogin] reads it again.
  static final ValueNotifier<rust.UiLaunchAtLogin> launchAtLogin = ValueNotifier(
    const rust.UiLaunchAtLogin(enabled: false, needsApproval: false),
  );

  /// The bridge's version, for the settings page.
  static String version = '';

  /// Off in the screenshot mode: nothing is read or written.
  static bool _persist = false;

  /// Reads the saved settings; before the client starts.
  static void load() {
    _persist = true;
    try {
      final saved = rust.loadSettings();
      appearance.value = saved.appearance;
      language.value = saved.language;
      apiOverride.value = saved.apiOverride;
    } catch (error) {
      debugPrint('load_settings: $error');
    }
    version = rust.appVersion();
  }

  static String? _save({rust.UiAppearance? appearance, rust.UiLanguage? language, String? apiOverride}) {
    if (!_persist) return null;
    try {
      rust.saveSettings(
        settings: rust.UiSettings(
          appearance: appearance ?? Settings.appearance.value,
          language: language ?? Settings.language.value,
          apiOverride: apiOverride ?? Settings.apiOverride.value,
        ),
      );
      return null;
    } catch (error) {
      debugPrint('save_settings: $error');
      return '$error';
    }
  }

  static void setAppearance(rust.UiAppearance value) {
    appearance.value = value;
    _save();
  }

  static void setLanguage(rust.UiLanguage value) {
    language.value = value;
    _save();
  }

  /// Saves the override (trimmed); false when it is not empty and not an
  /// http(s) URL, and nothing changed.
  static bool setApiOverride(String value) {
    var text = value.trim();
    while (text.endsWith('/')) {
      text = text.substring(0, text.length - 1);
    }
    if (text.isNotEmpty && !text.startsWith('https://') && !text.startsWith('http://')) return false;
    if (_save(apiOverride: text) != null) return false;
    apiOverride.value = text;
    return true;
  }

  /// Reads launch at login from the system (the settings page, when it
  /// shows and when the window gains focus); the error's text when the
  /// system raised one, the last state kept.
  static Future<String?> refreshLaunchAtLogin() async {
    if (!_persist) return null;
    try {
      if (Platform.isMacOS) {
        final state = (await _loginItem.invokeMapMethod<String, bool>('state'))!;
        launchAtLogin.value = rust.UiLaunchAtLogin(
          enabled: state['enabled'] ?? false,
          needsApproval: state['needsApproval'] ?? false,
        );
      } else {
        launchAtLogin.value = rust.launchAtLogin();
      }
      return null;
    } catch (error) {
      debugPrint('launch_at_login: $error');
      return _text(error);
    }
  }

  /// Turns launch at login on or off, then reads it back; the error's text
  /// when the system refused.
  static Future<String?> setLaunchAtLogin(bool enabled) async {
    if (!_persist) return null;
    String? failure;
    try {
      if (Platform.isMacOS) {
        await _loginItem.invokeMethod<void>('set', enabled);
      } else {
        rust.setLaunchAtLogin(enabled: enabled);
      }
    } catch (error) {
      debugPrint('set_launch_at_login: $error');
      failure = _text(error);
    }
    return failure ?? await refreshLaunchAtLogin();
  }

  /// Opens where the system holds the entry back; the error's text when
  /// that failed.
  static Future<String?> openLaunchAtLoginSettings() async {
    try {
      if (Platform.isMacOS) {
        await _loginItem.invokeMethod<void>('openSettings');
      } else {
        rust.openLaunchAtLoginSettings();
      }
      return null;
    } catch (error) {
      debugPrint('open_launch_at_login_settings: $error');
      return _text(error);
    }
  }

  /// The runner's error is the system's description alone.
  static String _text(Object error) => error is PlatformException ? (error.message ?? error.code) : '$error';
}
