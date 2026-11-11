import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/phone/native_chrome.dart';
import 'package:ppvpn_flutter/theme.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';

import 'messages_owner_test.dart' show message, inbox;

rust.UiSnapshot counted(int n, {String owner = 'A'}) => rust.UiSnapshot(
  auth: rust.UiAuth.signedIn,
  accountId: owner,
  accountName: owner,
  teamId: 'team',
  teamName: 'Team',
  profile: rust.UiProfile.ready,
  profilePrevious: false,
  phase: rust.UiPhase.off,
  standardReady: false,
  unread: n,
  routingMode: rust.UiRoutingMode.rules,
  serviceInstalled: false,
  connectionMode: rust.UiConnectionMode.enhanced,
  canTakeOver: false,
  suggestCompatible: false,
  apiUnreachable: false,
);

class _Api extends Fake implements RustLibApi {
  int count = 3;
  int status = 200;
  String owner = 'A';
  final events = <Completer<rust.UiEvents>>[];
  void publish() => events
      .removeAt(0)
      .complete(
        rust.UiEvents(
          snapshot: counted(count, owner: owner),
          probes: const [],
        ),
      );
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

  Future<void> change(int next) async {
    if (status != 200) throw StateError('HTTP $status');
    count = next;
    publish();
  }

  @override
  Future<void> crateApiMarkNotificationRead({required int id}) =>
      change(count - 1);
  @override
  Future<void> crateApiMarkNotificationUnread({required int id}) =>
      change(count + 1);
  @override
  Future<void> crateApiMarkAllNotificationsRead() => change(0);
  @override
  Future<rust.UiMessagePage> crateApiNotifications({
    required int page,
    required int pageSize,
  }) async => rust.UiMessagePage(items: [message('message')], total: 1);
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final api = _Api();
  final messenger =
      TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
  const push = MethodChannel('com.peakpassvpn.ppvpn/apple-push');
  setUpAll(() async {
    RustLib.initMock(api: api);
    for (final (family, path) in [
      ('PPVPN Material', 'assets/fonts/MaterialSymbols.ttf'),
      ('PPVPN Material Filled', 'assets/fonts/MaterialSymbolsFilled.ttf'),
      ('packages/forui/Inter', 'packages/forui/assets/fonts/inter/Inter.ttf'),
    ]) {
      await (FontLoader(family)..addFont(rootBundle.load(path))).load();
    }
  });
  for (final native in [false, true]) {
    testWidgets('server count reaches Overview and Me badge; native=$native', (
      tester,
    ) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      NativeChrome.enabled = native;
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      tester.view.padding = FakeViewPadding(top: 47, bottom: 34);
      tester.view.viewPadding = FakeViewPadding(top: 47, bottom: 34);
      addTearDown(tester.view.reset);
      api.count = 3;
      api.status = 200;
      api.owner = 'A';
      messenger.setMockMethodCallHandler(push, (_) async => null);
      final channels = <MethodChannel>[];
      final nativeUpdates = <Map<Object?, Object?>>[];
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
        call,
      ) async {
        if (call.method == 'create') {
          final id = (call.arguments as Map)['id'] as int;
          final channel = MethodChannel(
            'com.peakpassvpn.ppvpn/native-chrome/$id',
          );
          channels.add(channel);
          messenger.setMockMethodCallHandler(channel, (call) async {
            if (call.method == 'update')
              nativeUpdates.add(
                Map<Object?, Object?>.from(call.arguments as Map),
              );
            return null;
          });
        }
        return null;
      });
      final app = AppState(counted(3));
      final navigator = GlobalKey<NavigatorState>();
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xffffffff),
          navigatorKey: navigator,
          localizationsDelegates: FLocalizations.localizationsDelegates,
          pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
            settings: settings,
            pageBuilder: (context, _, _) => builder(context),
          ),
          home: FTheme(
            data: Tokens.light.forui,
            child: DefaultTextStyle(
              style: Tokens.light.body,
              child: PhoneShell(app: app),
            ),
          ),
        ),
      );
      Future<void> settle() async {
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        await tester.pump();
      }

      int badge() {
        if (!native)
          return tester.widget<PhoneBell>(find.byType(PhoneBell)).unread;
        final actions = tester
            .widgetList<NativeChromeView>(find.byType(NativeChromeView))
            .expand((v) => (v.configuration['actions'] as List? ?? const []))
            .whereType<Map>()
            .where((a) => a['id'] == 'messages')
            .toList();
        expect(actions, hasLength(1));
        return actions.single['badge'] as int;
      }

      await settle();
      expect(badge(), 3);
      for (final action in ['read', 'unread', 'all']) {
        app.showPage(WindowPage.messages);
        await settle();
        app.messages.view.value = inbox(
          message('message', read: action == 'unread'),
        );
        if (action == 'all') {
          await app.messages.markAllRead();
        } else {
          await app.messages.setRead(
            app.messages.view.value.items.single,
            action == 'read',
          );
        }
        await settle();
        navigator.currentState!.pop();
        await settle();
        expect(badge(), api.count);
        app.showPage(WindowPage.settings);
        await settle();
        expect(badge(), api.count);
      }
      api.count = 4;
      api.publish();
      await settle();
      for (final status in [403, 500]) {
        api.status = status;
        app.messages.view.value = inbox(message('failure'));
        await app.messages.setRead(app.messages.view.value.items.single, true);
        await settle();
        expect(badge(), 4);
        expect(app.messages.writeErrors.value[1], contains('HTTP $status'));
      }
      api.owner = 'B';
      api.count = 7;
      api.publish();
      await settle();
      expect(badge(), 7);
      expect(app.messages.writeErrors.value, isEmpty);
      if (native) {
        final messages = nativeUpdates
            .expand((v) => (v['actions'] as List? ?? const []))
            .whereType<Map>()
            .where((a) => a['id'] == 'messages');
        expect(messages.map((a) => a['badge']), containsAll([3, 2, 0, 4, 7]));
      }
      await tester.pumpWidget(const SizedBox());
      app.stop();
      await tester.pump();
      messenger.setMockMethodCallHandler(push, null);
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
      for (final channel in channels) {
        messenger.setMockMethodCallHandler(channel, null);
      }
      NativeChrome.enabled = false;
      debugDefaultTargetPlatformOverride = null;
    });
  }
}
