import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/vpn.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/vpn');
  final calls = <String>[];
  var allowed = true;
  var startFails = false;
  var stopFails = false;
  setUp(() {
    calls.clear();
    allowed = true;
    startFails = false;
    stopFails = false;
    SystemVpn.state.value = VpnState.off;
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, (call) async {
          calls.add(call.method);
          switch (call.method) {
            case 'prepare':
              return allowed;
            case 'start':
              if (startFails) throw PlatformException(code: 'start-rejected');
              SystemVpn.state.value = VpnState.on;
              return null;
            case 'stop':
              if (stopFails) throw PlatformException(code: 'stop-rejected');
              SystemVpn.state.value = VpnState.off;
              return null;
          }
          throw StateError('unexpected native method');
        });
  });
  tearDown(() {
    TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger
        .setMockMethodCallHandler(channel, null);
    SystemVpn.state.value = VpnState.off;
  });

  test('rejected native start restores the last reported state', () async {
    startFails = true;
    await expectLater(SystemVpn.start(), throwsA(isA<PlatformException>()));
    expect(SystemVpn.state.value, VpnState.off);
  });

  test(
    'denied permission does not start a provider or activate the core',
    () async {
      allowed = false;
      var activated = false;
      await SystemVpn.connect(() async {
        activated = true;
      }, keepServing: () => false);
      expect(calls, ['prepare']);
      expect(activated, isFalse);
    },
  );

  test('a partial native start failure still requests cleanup', () async {
    startFails = true;
    await expectLater(
      SystemVpn.connect(() async {}, keepServing: () => false),
      throwsA(isA<PlatformException>()),
    );
    expect(calls, ['prepare', 'start', 'stop']);
  });

  test('profile rejection cannot leave a newly started provider up', () async {
    await expectLater(
      SystemVpn.connect(() async {
        throw StateError('PROFILE_INVALID');
      }, keepServing: () => false),
      throwsA(isA<StateError>()),
    );
    expect(calls, ['prepare', 'start', 'stop']);
    expect(SystemVpn.state.value, VpnState.off);
  });

  test(
    'a rejected update preserves an existing verified serving VPN',
    () async {
      SystemVpn.state.value = VpnState.on;
      await expectLater(
        SystemVpn.connect(() async {
          throw StateError('PROFILE_INVALID');
        }, keepServing: () => true),
        throwsA(isA<StateError>()),
      );
      expect(calls, ['prepare']);
      expect(SystemVpn.state.value, VpnState.on);
    },
  );

  test(
    'an unverified existing provider is stopped after activation fails',
    () async {
      SystemVpn.state.value = VpnState.on;
      await expectLater(
        SystemVpn.connect(() async {
          throw StateError('PROFILE_INVALID');
        }, keepServing: () => false),
        throwsA(isA<StateError>()),
      );
      expect(calls, ['prepare', 'stop']);
      expect(SystemVpn.state.value, VpnState.off);
    },
  );

  test(
    'cleanup failure does not replace the original activation error',
    () async {
      stopFails = true;
      await expectLater(
        SystemVpn.connect(() async {
          throw StateError('original');
        }, keepServing: () => false),
        throwsA(
          isA<StateError>().having((e) => e.message, 'cause', 'original'),
        ),
      );
      expect(calls, ['prepare', 'start', 'stop']);
    },
  );

  test('reattaching to an on provider does not restart it', () async {
    SystemVpn.state.value = VpnState.on;
    var activated = false;
    await SystemVpn.connect(() async {
      activated = true;
    }, keepServing: () => true);
    expect(calls, ['prepare']);
    expect(activated, isTrue);
    expect(SystemVpn.state.value, VpnState.on);
  });

  test(
    'a state-read failure cannot hide the original activation failure',
    () async {
      SystemVpn.state.value = VpnState.on;
      await expectLater(
        SystemVpn.connect(
          () async {
            throw StateError('original');
          },
          keepServing: () {
            throw StateError('state unavailable');
          },
        ),
        throwsA(
          isA<StateError>().having((e) => e.message, 'cause', 'original'),
        ),
      );
      expect(calls, ['prepare', 'stop']);
      expect(SystemVpn.state.value, VpnState.off);
    },
  );
}
