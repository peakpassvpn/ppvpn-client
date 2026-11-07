import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/links.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/app');
  const url =
      'https://example.com/dashboard/device/authorize?user_code=TEST-CODE';
  final calls = <MethodCall>[];

  setUp(() {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    calls.clear();
    authenticationPresentationActive.value = false;
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    authenticationPresentationActive.value = false;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
  });

  test('iOS device login uses private authentication for every attempt', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          return true;
        });
    expect(await openAuthenticationUrl(url), isTrue);
    // A subsequent login or reopening the page must use the same private path,
    // not the normal cookie-sharing page that retained the previous account.
    expect(await openAuthenticationUrl(url), isTrue);
    expect(calls.map((call) => call.method), [
      'openAuthenticationUrl',
      'openAuthenticationUrl',
    ]);
    expect(calls.every((call) => call.arguments == url), isTrue);
  });

  test('failed private login never falls back to a shared browser', () async {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call);
          throw PlatformException(code: 'authentication-unavailable');
        });
    expect(await openAuthenticationUrl(url), isFalse);
    expect(authenticationPresentationActive.value, false);
    expect(calls.map((call) => call.method), ['openAuthenticationUrl']);
  });

  test(
    'authentication presentation stays active until native close acknowledges',
    () async {
      final closed = Completer<void>();
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
          .setMockMethodCallHandler(channel, (call) async {
            if (call.method == 'closeBrowser') {
              await closed.future;
              return null;
            }
            return true;
          });
      expect(await openAuthenticationUrl(url), true);
      expect(authenticationPresentationActive.value, true);
      final close = closeBrowser();
      await Future<void>.delayed(Duration.zero);
      expect(authenticationPresentationActive.value, true);
      closed.complete();
      await close;
      expect(authenticationPresentationActive.value, false);
    },
  );
}
