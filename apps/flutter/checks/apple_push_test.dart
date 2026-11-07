import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/apple_push.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  var epoch = "0";
  Future<Object?> native(String method, Map<String, Object?> arguments) {
    if (method == 'register') arguments = {'clientEpoch': epoch, ...arguments};
    if (method == 'notice') arguments = {'deliveryID': 'push:1', ...arguments};
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

  setUp(() {
    epoch = '0';
    messenger.setMockMethodCallHandler(channel, (call) async {
      if (call.method == 'account')
        epoch = (call.arguments as Map)['clientEpoch'] as String;
      return null;
    });
  });
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));

  test(
    'dispose rejects an already running notice before native activation',
    () async {
      final entered = Completer<void>();
      final response = Completer<bool>();
      final push = ApplePush(
        notice: (_, _, _, _) {
          entered.complete();
          return response.future;
        },
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      final pending = native('notice', {'accountID': 'one', 'opened': true});
      await entered.future;
      push.dispose();
      response.complete(true);
      expect(await pending, false);
    },
  );

  test('owner replacement rejects an already running notice result', () async {
    final entered = Completer<void>();
    final response = Completer<bool>();
    final push = ApplePush(
      notice: (_, _, _, _) {
        entered.complete();
        return response.future;
      },
    )..start();
    await push.followAccount(state: 'signedIn', id: 'one');
    final pending = native('notice', {'accountID': 'one', 'opened': true});
    await entered.future;
    await push.followAccount(state: 'signedIn', id: 'two');
    response.complete(true);
    expect(await pending, false);
    push.dispose();
  });

  test('absent backend adapter never confirms native registration', () async {
    final push = ApplePush(notice: (_, _, _, _) async => true)..start();
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
        notice: (_, _, _, _) async => true,
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
        notice: (_, id, _, _) async {
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
        notice: (_, _, _, _) async => false,
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
    final push = ApplePush(notice: (_, _, _, _) async => true)..start();
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
    final push = ApplePush(notice: (_, _, _, _) async => true)..start();
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
      final push = ApplePush(notice: (_, _, _, _) async => true)..start();
      expect(await push.resume(), true);
      expect(calls, ['configured', 'retry']);
      push.dispose();
    },
  );
  test(
    'late registration success cannot acknowledge a replacement owner',
    () async {
      final committed = Completer<bool>();
      final push = ApplePush(
        notice: (_, _, _, _) async => true,
        register: (_) => committed.future,
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      final result = native('register', {'accountID': 'one'});
      await Future<void>.delayed(Duration.zero);
      await push.followAccount(state: 'signedIn', id: 'two');
      committed.complete(true);
      expect(await result, false);
      push.dispose();
    },
  );
  test(
    'native old epoch cannot register after relogin to the same public id',
    () async {
      var calls = 0;
      final push = ApplePush(
        notice: (_, _, _, _) async => true,
        register: (_) async {
          calls++;
          return true;
        },
      )..start();
      await push.followAccount(state: 'signedIn', id: 'one');
      final old = epoch;
      await push.followAccount(state: 'signedOut');
      await push.followAccount(state: 'signedIn', id: 'one');
      expect(
        await native('register', {'accountID': 'one', 'clientEpoch': old}),
        false,
      );
      expect(calls, 0);
      expect(await native('register', {'accountID': 'one'}), true);
      push.dispose();
    },
  );
}
