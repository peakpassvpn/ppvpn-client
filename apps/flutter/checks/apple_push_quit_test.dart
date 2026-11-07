import 'dart:async';
import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';

import 'messages_owner_test.dart' show ownerSnapshot, message;

class _Api extends Fake implements RustLibApi {
  late Future<rust.UiMessage> Function() lookup;
  late Future<rust.UiMessagePage> Function() reload;
  final events = <Completer<rust.UiEvents>>[];
  int reloads = 0;
  @override
  Future<rust.UiEvents> crateApiNextEvents() {
    final event = Completer<rust.UiEvents>();
    events.add(event);
    return event.future;
  }

  @override
  void crateApiShutdown() {
    for (final event in events) {
      if (!event.isCompleted) event.complete(const rust.UiEvents(probes: []));
    }
    events.clear();
  }

  @override
  Future<rust.UiMessage> crateApiApplePushMessage({
    required String owner,
    required String deliveryId,
    int? messageId,
  }) => lookup();
  @override
  Future<rust.UiMessagePage> crateApiNotifications({
    required int page,
    required int pageSize,
  }) {
    reloads++;
    return reload();
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  final api = _Api();
  setUpAll(() => RustLib.initMock(api: api));
  setUp(() {
    api.reloads = 0;
    messenger.setMockMethodCallHandler(channel, (_) async => null);
  });
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));
  Future<Object?> openedNotice() {
    final result = Completer<Object?>();
    messenger.handlePlatformMessage(
      channel.name,
      const StandardMethodCodec().encodeMethodCall(
        const MethodCall('notice', {
          'accountID': 'A',
          'messageID': '1',
          'deliveryID': 'push:1',
          'opened': true,
        }),
      ),
      (reply) => result.complete(
        reply == null
            ? false
            : const StandardMethodCodec().decodeEnvelope(reply),
      ),
    );
    return result.future;
  }

  test(
    'stop during authorized lookup cannot queue message UI or success',
    skip: !Platform.isMacOS,
    () async {
      final entered = Completer<void>();
      final lookup = Completer<rust.UiMessage>();
      api.lookup = () {
        entered.complete();
        return lookup.future;
      };
      api.reload = () async => const rust.UiMessagePage(items: [], total: 0);
      final app = AppState(ownerSnapshot('A', 'Owner'));
      addTearDown(() async {
        app.stop();
        await Future<void>.delayed(Duration.zero);
      });
      final reply = openedNotice();
      await entered.future;
      app.stop();
      lookup.complete(message('authorized'));
      expect(await reply, false);
      expect(api.reloads, 0);
      expect(app.messages.takeOpen(), isNull);
      expect(app.pageRequests.value, isNull);
    },
  );
  test(
    'stop during inbox refresh cannot request page or success',
    skip: !Platform.isMacOS,
    () async {
      final entered = Completer<void>();
      final reload = Completer<rust.UiMessagePage>();
      api.lookup = () async => message('authorized');
      api.reload = () {
        entered.complete();
        return reload.future;
      };
      final app = AppState(ownerSnapshot('A', 'Owner'));
      addTearDown(() async {
        app.stop();
        await Future<void>.delayed(Duration.zero);
      });
      final reply = openedNotice();
      await entered.future;
      final requestsBeforeStop = app.messages.openRequests.value;
      app.stop();
      reload.complete(const rust.UiMessagePage(items: [], total: 0));
      expect(await reply, false);
      expect(app.messages.openRequests.value, requestsBeforeStop);
      expect(app.pageRequests.value, isNull);
    },
  );
}
