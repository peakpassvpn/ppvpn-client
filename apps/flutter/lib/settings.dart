// The app's own settings (#401 step 3): appearance, language and the API
// override, kept by the bridge in settings.json in the data directory
// (rust/src/settings.rs). The routing mode is the client's and comes with
// the snapshot. Theme (Tokens.of) and strings (S.current) follow the
// notifiers here; main.dart rebuilds the app when they change.

import 'package:flutter/foundation.dart';

import 'src/rust/api.dart' as rust;

class Settings {
  Settings._();

  static final ValueNotifier<rust.UiAppearance> appearance = ValueNotifier(rust.UiAppearance.system);
  static final ValueNotifier<rust.UiLanguage> language = ValueNotifier(rust.UiLanguage.system);

  /// As saved; the client read it when it started (it takes effect on the
  /// next launch).
  static final ValueNotifier<String> apiOverride = ValueNotifier('');

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
}
