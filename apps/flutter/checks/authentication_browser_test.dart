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
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
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
    expect(calls.map((call) => call.method), ['openAuthenticationUrl']);
  });
}
