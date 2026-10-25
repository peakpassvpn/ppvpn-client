// Android and iOS (#448): no window to manage, tray, updater or single
// instance (main.dart, shell.dart); the app's directory comes from the
// native side (main.dart).

import 'dart:io';

/// The app runs on a phone or tablet.
final bool mobile = Platform.isAndroid || Platform.isIOS;
