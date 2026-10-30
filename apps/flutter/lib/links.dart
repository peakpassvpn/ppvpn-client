// Opening links (#473). The desktops start the system's browser from Rust
// (rust/src/hooks.rs open_url). iOS and Android cannot start a process, so
// there the app channel shows the page inside the app: SFSafariViewController
// on iOS (runner/ios PPVPNPlugin), a Custom Tab or else the browser on
// Android (runner/android MainActivity).

import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'mobile.dart';
import 'src/rust/api.dart' as rust;

const _app = MethodChannel('com.peakpassvpn.ppvpn/app');

/// Opens [url]; false when nothing could show it.
Future<bool> openUrl(String url) async {
  if (!mobile) return rust.openUrl(url: url);
  try {
    return await _app.invokeMethod<bool>('openUrl', url) ?? false;
  } on PlatformException catch (error) {
    debugPrint('openUrl: $error');
    return false;
  }
}

/// iOS: closes the page [openUrl] showed, if it is still up (after the
/// sign-in it was opened for). Android's Custom Tab cannot be closed by the
/// app: the user comes back with Back.
Future<void> closeBrowser() async {
  if (!Platform.isIOS) return;
  try {
    await _app.invokeMethod<void>('closeBrowser');
  } on PlatformException catch (error) {
    debugPrint('closeBrowser: $error');
  }
}
