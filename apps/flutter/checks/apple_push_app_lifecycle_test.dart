import 'dart:async';
import 'dart:convert';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/links.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';

import 'apple_push_transport_test.dart' show proof, response;
import 'messages_owner_test.dart' show ownerSnapshot;

class _Api extends Fake implements RustLibApi {
  final events = <Completer<rust.UiEvents>>[];
  @override
  Future<rust.UiEvents> crateApiNextEvents() {
    final c = Completer<rust.UiEvents>();
    events.add(c);
    return c.future;
  }

  @override
  void crateApiShutdown() {
    for (final c in events) {
      if (!c.isCompleted) c.complete(const rust.UiEvents(probes: []));
    }
    events.clear();
  }

  @override
  Future<String> crateApiApplePushHttp({
    required String owner,
    required String operation,
    required String body,
  }) async {
    final r = response(operation);
    return jsonEncode({
      'status': r.status,
      'body': operation == 'updates'
          ? {'items': [], 'has_more': false}
          : r.body,
    });
  }

  @override
  Future<rust.UiMessagePage> crateApiNotifications({
    required int page,
    required int pageSize,
  }) async => const rust.UiMessagePage(items: [], total: 0);
}

void main() {
  final binding = TestWidgetsFlutterBinding.ensureInitialized();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const pushChannel = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  const appChannel = MethodChannel('com.peakpassvpn.ppvpn/app');
  final api = _Api();
  var epoch = '0';
  var prompts = 0;
  setUpAll(() => RustLib.initMock(api: api));
  setUp(() {
    epoch = '0';
    prompts = 0;
    authenticationPresentationActive.value = false;
    binding.handleAppLifecycleStateChanged(AppLifecycleState.resumed);
    messenger.setMockMethodCallHandler(pushChannel, (call) async {
      if (call.method == 'account')
        epoch = (call.arguments as Map)['clientEpoch'] as String;
      if (call.method == 'configured' || call.method == 'permission') {
        if (call.method == 'permission') prompts++;
        return true;
      }
      if (call.method == 'installation') return proof;
      return null;
    });
  });
  tearDown(() {
    debugDefaultTargetPlatformOverride = null;
    authenticationPresentationActive.value = false;
    messenger.setMockMethodCallHandler(pushChannel, null);
    messenger.setMockMethodCallHandler(appChannel, null);
  });
  Future<Object?> register() {
    final done = Completer<Object?>();
    messenger.handlePlatformMessage(
      pushChannel.name,
      const StandardMethodCodec().encodeMethodCall(
        MethodCall('register', {
          'clientEpoch': epoch,
          'accountID': 'A',
          'accountGeneration': '1',
          'platform': 'macos',
          'bundleID': 'test.mac',
          'environment': 'production',
          'token': 'aabb',
        }),
      ),
      (reply) =>
          done.complete(const StandardMethodCodec().decodeEnvelope(reply!)),
    );
    return done.future;
  }

  test(
    'production AppState observes inactive paused resumed and retries committed permission',
    skip: !Platform.isMacOS,
    () async {
      binding.handleAppLifecycleStateChanged(AppLifecycleState.inactive);
      final app = AppState(ownerSnapshot('A', 'Owner'));
      addTearDown(app.stop);
      await Future<void>.delayed(Duration.zero);
      expect(await register(), true);
      await Future<void>.delayed(Duration.zero);
      expect(prompts, 0);
      for (final state in [
        AppLifecycleState.hidden,
        AppLifecycleState.paused,
        AppLifecycleState.hidden,
        AppLifecycleState.inactive,
        AppLifecycleState.resumed,
      ])
        binding.handleAppLifecycleStateChanged(state);
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(prompts, 1);
    },
  );

  test(
    'production AppState waits for authentication close ACK even without a resumed event',
    skip: !Platform.isMacOS,
    () async {
      final closing = Completer<void>();
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      messenger.setMockMethodCallHandler(appChannel, (call) async {
        if (call.method == 'closeBrowser') {
          await closing.future;
          return null;
        }
        return true;
      });
      expect(
        await openAuthenticationUrl(
          'https://example.com/dashboard/device/authorize?user_code=FIXTURE',
        ),
        true,
      );
      final app = AppState(ownerSnapshot('A', 'Owner'));
      addTearDown(app.stop);
      await Future<void>.delayed(Duration.zero);
      expect(await register(), true);
      await Future<void>.delayed(Duration.zero);
      expect(prompts, 0);
      final close = closeBrowser();
      await Future<void>.delayed(Duration.zero);
      expect(prompts, 0);
      closing.complete();
      await close;
      await Future<void>.delayed(Duration.zero);
      await Future<void>.delayed(Duration.zero);
      expect(prompts, 1);
    },
  );
}
