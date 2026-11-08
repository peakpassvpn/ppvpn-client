import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/phone/messages.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/strings.dart';
import 'package:ppvpn_flutter/theme.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';

import 'messages_owner_test.dart' show message, ownerSnapshot, inbox;

class _Api extends Fake implements RustLibApi {
  int status = 200;
  bool read = false;
  final ids = <int>[];
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
  Future<rust.UiMessage> crateApiApplePushMessage({
    required String owner,
    required String deliveryId,
    int? messageId,
  }) async {
    expect(deliveryId, 'push:2');
    expect(messageId, 1);
    return message('owned message', read: read);
  }

  @override
  Future<rust.UiMessagePage> crateApiNotifications({
    required int page,
    required int pageSize,
  }) async => rust.UiMessagePage(
    items: [message('owned message', read: read)],
    total: 1,
  );
  @override
  Future<void> crateApiMarkNotificationRead({required int id}) async {
    ids.add(id);
    if (status != 200) throw StateError('HTTP $status');
    read = true;
  }
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final api = _Api();
  const channel = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  setUpAll(() => RustLib.initMock(api: api));
  setUp(() {
    api.read = false;
    api.ids.clear();
    Settings.language.value = rust.UiLanguage.english;
    messenger.setMockMethodCallHandler(channel, (_) async => null);
  });
  tearDown(() => messenger.setMockMethodCallHandler(channel, null));
  for (final language in [rust.UiLanguage.english, rust.UiLanguage.chinese]) {
    for (final status in [200, 403, 500]) {
      testWidgets(
        '$language tap message 1 from delivery 2 and read result HTTP $status',
        (tester) async {
          api.status = status;
          Settings.language.value = language;
          final app = AppState(ownerSnapshot('A', 'Owner'));
          app.messages.view.value = inbox(message('owned message'));
          await tester.pumpWidget(
            WidgetsApp(
              color: const Color(0xffffffff),
              localizationsDelegates: FLocalizations.localizationsDelegates,
              pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
                settings: settings,
                pageBuilder: (context, _, _) => builder(context),
              ),
              home: FTheme(
                data: Tokens.light.forui,
                child: DefaultTextStyle(
                  style: Tokens.light.body,
                  child: PhoneScaffold(
                    title: 'Inbox',
                    child: PhoneMessagesPage(app: app),
                  ),
                ),
              ),
            ),
          );
          final reply = Completer<Object?>();
          messenger.handlePlatformMessage(
            channel.name,
            const StandardMethodCodec().encodeMethodCall(
              const MethodCall('notice', {
                'accountID': 'A',
                'messageID': '1',
                'deliveryID': 'push:2',
                'opened': true,
              }),
            ),
            (bytes) => reply.complete(
              const StandardMethodCodec().decodeEnvelope(bytes!),
            ),
          );
          await tester.pump();
          await tester.pump();
          await tester.pump(const Duration(seconds: 1));
          expect(await reply.future, true);
          expect(api.ids, [
            1,
          ]); // user_message_id, never the outbox/delivery id 2
          expect(app.messages.view.value.items.single.read, status == 200);
          if (status == 200) {
            await app.messages.reload();
            await tester.pump();
            expect(app.messages.view.value.items.single.read, true);
            expect(find.text(S.current('markUnread')), findsOneWidget);
            expect(app.messages.writeErrors.value, isEmpty);
          } else {
            expect(find.text(S.current('messageReadFailed')), findsOneWidget);
            expect(find.textContaining('HTTP $status'), findsNothing);
            expect(app.actionError.value, contains('HTTP $status'));
            await tester.tap(find.text(S.current('markRead')));
            await tester.pump(const Duration(milliseconds: 100));
            expect(api.ids, [1, 1]);
            expect(app.messages.view.value.items.single.read, false);
            expect(find.text(S.current('messageReadFailed')), findsOneWidget);
            expect(find.textContaining('HTTP $status'), findsNothing);
          }
          await tester.pumpWidget(const SizedBox());
          app.stop();
          await tester.pump();
        },
      );
    }
  }
}
