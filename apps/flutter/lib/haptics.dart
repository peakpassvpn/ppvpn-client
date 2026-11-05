// Discrete feedback on mobile; desktop and web stay quiet.
import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

abstract final class AppHaptics {
  static bool get available =>
      !kIsWeb &&
      (defaultTargetPlatform == TargetPlatform.iOS ||
          defaultTargetPlatform == TargetPlatform.android);
  static Future<void> selection() async {
    if (available) await HapticFeedback.selectionClick();
  }

  static Future<void> impact() async {
    if (available) await HapticFeedback.mediumImpact();
  }

  static Future<void> connected() async {
    if (available) await HapticFeedback.lightImpact();
  }
}
