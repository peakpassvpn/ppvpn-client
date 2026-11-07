import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/apple_push.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  Future<Object?> native(String method, Map<String, Object?> arguments) {
    final result = Completer<Object?>();
    messenger.handlePlatformMessage(
      channel.name,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall(method, arguments),
      ),
      (reply) =>
          result.complete(const StandardMethodCodec().decodeEnvelope(reply!)),
    );
    return result.future;
  }

  setUp(() => messenger.setMockMethodCallHandler(channel, (_) async => null));
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  test('absent backend adapter never confirms native registration', () async {
    final push = ApplePush(notice: (_, _, _) async => true)..start();
    await push.followAccount(state: 'signedIn', id: 'one');
    expect(
      await native('register', {'accountID': 'one', 'token': 'fresh'}),
      false,
    );
    push.dispose();
  });
  test(
    'acknowledgement waits for HTTP settlement and excludes old owner',
    () async {
      final committed = Completer<bool>();
      var calls = 0;
      final push = ApplePush(
        notice: (_, _, _) async => true,
        register: (_) {
          calls++;
          return committed.future;
        },
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      expect(await native('register', {'accountID': 'old'}), false);
      final result = native('register', {'accountID': 'one'});
      await Future<void>.delayed(Duration.zero);
      expect(calls, 1);
      committed.complete(true);
      expect(await result, true);
      push.dispose();
    },
  );
  test(
    'notice carries only signed owner lookup and rejects foreign owner',
    () async {
      int? lookedUp;
      final push = ApplePush(
        notice: (_, id, _) async {
          lookedUp = id;
          return true;
        },
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      expect(
        await native('notice', {'accountID': 'two', 'messageID': '7'}),
        false,
      );
      expect(lookedUp, null);
      expect(
        await native('notice', {
          'accountID': 'one',
          'messageID': '7',
          'body': 'untrusted',
          'url': 'untrusted',
        }),
        true,
      );
      expect(lookedUp, 7);
      await native('notice', {'messageID': '7.1'});
      expect(lookedUp, null);
      push.dispose();
    },
  );
  test(
    'logout waits for serialized unregister before bearer revocation',
    () async {
      final settled = Completer<bool>();
      final push = ApplePush(
        notice: (_, _, _) async => false,
        unregister: (_) => settled.future,
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      final effect = native('unregister', {'accountID': 'one'});
      var done = false;
      final logout = push.beforeSignOut().then((_) => done = true);
      await Future<void>.delayed(Duration.zero);
      expect(done, false);
      settled.complete(true);
      await effect;
      await logout;
      expect(done, true);
      expect(await native('notice', {'accountID': 'one'}), false);
      push.dispose();
    },
  );
  test('restoration blocks previous owner delivery', () async {
    final push = ApplePush(notice: (_, _, _) async => true)..start();
    await push.followAccount(state: 'signedIn', id: 'one');
    await push.followAccount(state: 'restoring');
    expect(await native('notice', {'accountID': 'one'}), false);
    push.dispose();
  });
  test('disabled package does not retry push registration on resume', () async {
    final calls = <String>[];
    messenger.setMockMethodCallHandler(channel, (call) async {
      calls.add(call.method);
      return call.method == 'configured' ? false : null;
    });
    final push = ApplePush(notice: (_, _, _) async => true)..start();
    expect(await push.resume(), false);
    expect(calls, ['configured']);
    push.dispose();
  });
  test(
    'configured package requests a fresh OS registration on resume',
    () async {
      final calls = <String>[];
      messenger.setMockMethodCallHandler(channel, (call) async {
        calls.add(call.method);
        return call.method == 'configured' ? true : null;
      });
      final push = ApplePush(notice: (_, _, _) async => true)..start();
      expect(await push.resume(), true);
      expect(calls, ['configured', 'retry']);
      push.dispose();
    },
  );
}
