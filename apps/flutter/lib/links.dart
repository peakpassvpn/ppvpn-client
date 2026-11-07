// Opening links (#473). The desktops start the system's browser from Rust
// (rust/src/hooks.rs open_url). iOS and Android cannot start a process, so
// there the app channel shows the page inside the app: SFSafariViewController
// as a page sheet on iOS (runner/ios PPVPNPlugin), a Custom Tab or else the
// browser on Android (runner/android MainActivity).

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

import 'mobile.dart';
import 'src/rust/api.dart' as rust;

const _app = MethodChannel('com.peakpassvpn.ppvpn/app');
// Session presentation can remain on top while UIKit reports the app active.
// Permission intent must wait for the normal close acknowledgement as well.
final authenticationPresentationActive = ValueNotifier<bool>(false);

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

/// Device authorization must not inherit another account's browser cookies.
/// iOS uses a private system authentication session; ordinary links retain
/// their normal browsing session. Android keeps its Custom Tab flow.
Future<bool> openAuthenticationUrl(String url) async {
  if (defaultTargetPlatform != TargetPlatform.iOS) return openUrl(url);
  authenticationPresentationActive.value = true;
  try {
    final opened =
        await _app.invokeMethod<bool>('openAuthenticationUrl', url) ?? false;
    if (!opened) authenticationPresentationActive.value = false;
    return opened;
  } on PlatformException catch (error) {
    authenticationPresentationActive.value = false;
    debugPrint('openAuthenticationUrl: $error');
    // Never fall back to the cookie-sharing browser for authentication.
    return false;
  }
}

/// Closes the page [openUrl] showed, if it is still up (after the sign-in
/// it was opened for): iOS dismisses the sheet; Android brings the app back
/// over its Custom Tab, which finishes it.
Future<void> closeBrowser() async {
  if (!mobile && defaultTargetPlatform != TargetPlatform.iOS) return;
  try {
    await _app.invokeMethod<void>('closeBrowser');
    authenticationPresentationActive.value = false;
  } on PlatformException catch (error) {
    debugPrint('closeBrowser: $error');
  }
}
