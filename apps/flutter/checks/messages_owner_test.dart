import 'dart:async' show Completer;

import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/messages_state.dart';
import 'package:ppvpn_flutter/pages/messages.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';
import 'package:ppvpn_flutter/theme.dart';

class _Api extends Fake implements RustLibApi {
  late Future<rust.UiMessagePage> Function(int page) load;
  late Future<void> Function() write;
  final events = <Completer<rust.UiEvents>>[];
  final writes = <String>[];
  Future<void> writeKind(String kind) {
    writes.add(kind);
    return write();
  }

  @override
  Future<rust.UiEvents> crateApiNextEvents() {
    final next = Completer<rust.UiEvents>();
    events.add(next);
    return next.future;
  }

  @override
  void crateApiShutdown() {
    for (final next in events) {
      if (!next.isCompleted) next.complete(const rust.UiEvents(probes: []));
    }
    events.clear();
  }

  @override
  Future<rust.UiMessagePage> crateApiNotifications({
    required int page,
    required int pageSize,
  }) => load(page);
  @override
  Future<void> crateApiMarkNotificationRead({required int id}) =>
      writeKind('read');
  @override
  Future<void> crateApiMarkNotificationUnread({required int id}) =>
      writeKind('unread');
  @override
  Future<void> crateApiMarkAllNotificationsRead() => writeKind('all');
}

rust.UiMessage message(String title, {bool read = false}) => rust.UiMessage(
  id: 1,
  title: title,
  content: '$title content',
  category: rust.UiMessageCategory.other,
  severity: rust.UiMessageSeverity.normal,
  read: read,
  createdAt: '2026-11-01T00:00:00Z',
);
MessagesView inbox(rust.UiMessage item) =>
    MessagesView(items: [item], total: 1, loaded: true);

rust.UiSnapshot ownerSnapshot(String accountId, String name) => rust.UiSnapshot(
  auth: rust.UiAuth.signedIn,
  accountId: accountId,
  accountName: name,
  teamId: 'same-team',
  teamName: 'Same team',
  profile: rust.UiProfile.ready,
  profilePrevious: false,
  phase: rust.UiPhase.off,
  standardReady: false,
  unread: 0,
  routingMode: rust.UiRoutingMode.rules,
  serviceInstalled: false,
  connectionMode: rust.UiConnectionMode.enhanced,
  canTakeOver: false,
  suggestCompatible: false,
  apiUnreachable: false,
);

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final api = _Api();
  final errors = <String>[];
  setUpAll(() => RustLib.initMock(api: api));
  setUp(() {
    expect(api.events, isEmpty);
    errors.clear();
    api.writes.clear();
    api.load = (_) async => const rust.UiMessagePage(items: [], total: 0);
    api.write = () async {};
  });
  MessagesState state() => MessagesState(fixed: false, onError: errors.add);

  test('read mutation survives an older reload response', () async {
    final listed = Completer<rust.UiMessagePage>();
    final written = Completer<void>();
    api.load = (_) => listed.future;
    api.write = () => written.future;
    final m = state()..followOwner(accountId: 'account-a');
    final item = message('race');
    m.view.value = inbox(item);
    final loading = m.reload();
    final writing = m.setRead(item, true);
    written.complete();
    await writing;
    listed.complete(rust.UiMessagePage(items: [item], total: 1));
    await loading;
    expect(m.view.value.items.single.read, true);
    api.load = (_) async => rust.UiMessagePage(items: [item], total: 1);
    await m.reload();
    expect(
      m.view.value.items.single.read,
      false,
      reason: 'A subsequent fresh snapshot remains authoritative',
    );
  });
  test('read pending at reload start survives its late snapshot', () async {
    final listed = Completer<rust.UiMessagePage>();
    final written = Completer<void>();
    api.load = (_) => listed.future;
    api.write = () => written.future;
    final m = state()..followOwner(accountId: 'account-a');
    final item = message('pending');
    m.view.value = inbox(item);
    final writing = m.setRead(item, true);
    final loading = m.reload();
    written.complete();
    await writing;
    listed.complete(rust.UiMessagePage(items: [item], total: 1));
    await loading;
    expect(m.view.value.items.single.read, true);
  });
  test('read failure rolls back despite a newly started reload', () async {
    final listed = Completer<rust.UiMessagePage>();
    final written = Completer<void>();
    api.load = (_) => listed.future;
    api.write = () => written.future;
    final m = state()..followOwner(accountId: 'account-a');
    final item = message('failure');
    m.view.value = inbox(item);
    final writing = m.setRead(item, true);
    final loading = m.reload();
    written.completeError(StateError('write refused'));
    await writing;
    expect(m.view.value.items.single.read, false);
    expect(errors, hasLength(1));
    listed.complete(rust.UiMessagePage(items: [item], total: 1));
    await loading;
  });
  test(
    'read intent uses current list instead of a stale message object',
    () async {
      var writes = 0;
      api.write = () async {
        writes++;
      };
      final m = state()..followOwner(accountId: 'account-a');
      m.view.value = inbox(message('current unread'));
      await m.setRead(message('stale read object', read: true), true);
      expect(writes, 1);
      expect(m.view.value.items.single.read, true);
    },
  );
  for (final firstFails in [false, true]) {
    test('read queue preserves order when first failure=$firstFails', () async {
      final first = Completer<void>();
      final second = Completer<void>();
      var calls = 0;
      api.write = () => ++calls == 1 ? first.future : second.future;
      final m = state()..followOwner(accountId: 'account-a');
      m.view.value = inbox(message('original'));
      final a = m.setRead(m.view.value.items.single, true);
      final b = m.setRead(m.view.value.items.single, false);
      expect(api.writes, ['read']);
      if (firstFails) {
        first.completeError(StateError('old failed'));
      } else {
        first.complete();
      }
      await a;
      await Future<void>.delayed(Duration.zero);
      expect(api.writes, ['read', 'unread']);
      second.complete();
      await b;
      expect(m.view.value.items.single.read, false);
      expect(errors, isEmpty);
    });
  }
  test(
    'read queue rolls newest failure back to last confirmed value',
    () async {
      final first = Completer<void>();
      final second = Completer<void>();
      var calls = 0;
      api.write = () => ++calls == 1 ? first.future : second.future;
      final m = state()..followOwner(accountId: 'account-a');
      m.view.value = inbox(message('original'));
      final a = m.setRead(m.view.value.items.single, true);
      final b = m.setRead(m.view.value.items.single, false);
      first.complete();
      await a;
      await Future<void>.delayed(Duration.zero);
      second.completeError(StateError('new failed'));
      await b;
      expect(m.view.value.items.single.read, true);
      expect(errors, hasLength(1));
      expect(m.writeErrors.value[1], contains('new failed'));
    },
  );
  test('read queue drops old queued writes after an owner switch', () async {
    final old = Completer<void>();
    final fresh = Completer<void>();
    var calls = 0;
    api.write = () => ++calls == 1 ? old.future : fresh.future;
    final m = state()..followOwner(accountId: 'account-a');
    m.view.value = inbox(message('old'));
    final a = m.setRead(m.view.value.items.single, true);
    final queued = m.setRead(m.view.value.items.single, false);
    m.followOwner(accountId: 'account-b');
    m.view.value = inbox(message('new owner, same id'));
    final b = m.setRead(m.view.value.items.single, true);
    old.complete();
    await a;
    await queued;
    expect(api.writes, ['read', 'read']);
    fresh.complete();
    await b;
    expect(m.view.value.items.single.read, true);
    expect(errors, isEmpty);
  });
  for (final bulkFirst in [true, false]) {
    test(
      'read queue sequences bulk and single, bulkFirst=$bulkFirst',
      () async {
        final first = Completer<void>();
        final second = Completer<void>();
        var calls = 0;
        api.write = () => ++calls == 1 ? first.future : second.future;
        final m = state()..followOwner(accountId: 'account-a');
        m.view.value = inbox(message('original'));
        final a = bulkFirst
            ? m.markAllRead()
            : m.setRead(m.view.value.items.single, true);
        final b = bulkFirst
            ? m.setRead(m.view.value.items.single, false)
            : m.markAllRead();
        expect(api.writes, [bulkFirst ? 'all' : 'read']);
        first.complete();
        await a;
        await Future<void>.delayed(Duration.zero);
        expect(api.writes, bulkFirst ? ['all', 'unread'] : ['read', 'all']);
        second.complete();
        await b;
        expect(m.view.value.items.single.read, !bulkFirst);
      },
    );
  }
  test('bulk read survives an old snapshot after ACK', () async {
    final listed = Completer<rust.UiMessagePage>();
    final written = Completer<void>();
    api.load = (_) => listed.future;
    api.write = () => written.future;
    final m = state()..followOwner(accountId: 'account-a');
    final item = message('bulk');
    m.view.value = inbox(item);
    final loading = m.reload();
    final writing = m.markAllRead();
    written.complete();
    await writing;
    listed.complete(rust.UiMessagePage(items: [item], total: 1));
    await loading;
    expect(m.view.value.items.single.read, true);
  });
  test(
    'snapshot pump separates equal-name accounts without a logout',
    () async {
      final app = AppState(ownerSnapshot('account-a', 'Same name'));
      addTearDown(() async {
        app.stop();
        await Future<void>.delayed(Duration.zero);
      });
      final item = message('old account data');
      app.messages.view.value = inbox(item);
      app.messages.requestOpen((messageId: item.id, push: item));
      api.events
          .removeAt(0)
          .complete(
            rust.UiEvents(
              snapshot: ownerSnapshot('account-b', 'Same name'),
              probes: const [],
            ),
          );
      await Future<void>.delayed(Duration.zero);
      expect(app.messages.view.value.items, isEmpty);
      expect(app.messages.takeOpen(), isNull);
    },
  );

  test(
    'snapshot pump retains same-account cache after a display rename',
    () async {
      final app = AppState(ownerSnapshot('account-a', 'Old name'));
      addTearDown(() async {
        app.stop();
        await Future<void>.delayed(Duration.zero);
      });
      final item = message('same account data');
      app.messages.view.value = inbox(item);
      app.messages.requestOpen((messageId: item.id, push: item));
      api.events
          .removeAt(0)
          .complete(
            rust.UiEvents(
              snapshot: ownerSnapshot('account-a', 'New name'),
              probes: const [],
            ),
          );
      await Future<void>.delayed(Duration.zero);
      expect(app.messages.view.value.items.single, item);
      expect(app.messages.takeOpen()?.push, item);
    },
  );

  test(
    'initial restoration retains a cold tap until the stable owner resolves',
    () {
      final m = state();
      final item = message('cold tap');
      m.requestOpen((messageId: item.id, push: item));
      m.followOwner(restoring: true);
      m.followOwner(accountId: 'account-a', teamId: 'team');
      expect(m.takeOpen()?.push, item);
    },
  );

  test('switching accounts without sign-out clears list and pending tap', () {
    final m = state()..followOwner(accountId: 'account-a', teamId: 'same-team');
    final item = message('old owner');
    m.view.value = inbox(item);
    m.requestOpen((messageId: item.id, push: item));
    m.followOwner(accountId: 'account-b', teamId: 'same-team');
    expect(m.view.value.items, isEmpty);
    expect(m.view.value.loaded, isFalse);
    expect(m.takeOpen(), isNull);
  });

  test('a replacement restoration cannot retarget an unscoped pending tap', () {
    final m = state()..followOwner(accountId: 'account-a');
    m.followOwner(restoring: true);
    final item = message('late old tap');
    m.requestOpen((messageId: item.id, push: item));
    m.followOwner(accountId: 'account-b');
    expect(m.takeOpen(), isNull);
  });

  test('an unchanged stable identity retains cache and pending navigation', () {
    final m = state()..followOwner(accountId: 'account-a', teamId: 'team');
    final item = message('same owner');
    m.view.value = inbox(item);
    m.requestOpen((messageId: item.id, push: item));
    final epoch = m.ownerEpoch;
    m.followOwner(accountId: 'account-a', teamId: 'team');
    expect(m.ownerEpoch, epoch);
    expect(m.view.value.items.single, item);
    expect(m.takeOpen()?.push, item);
  });

  test('initial team hydration retains the same-owner cold tap and cache', () {
    final m = state()..followOwner(accountId: 'account-a');
    final item = message('known owner cold tap');
    m.view.value = inbox(item);
    m.requestOpen((messageId: item.id, push: item));
    final epoch = m.ownerEpoch;
    m.followOwner(accountId: 'account-a', teamId: 'resolved-team');
    expect(m.ownerEpoch, epoch);
    expect(m.view.value.items.single, item);
    expect(m.takeOpen()?.push, item);
  });

  test('logout and team replacement clear old inbox data', () {
    final m = state()..followOwner(accountId: 'account-a', teamId: 'team-a');
    m.view.value = inbox(message('old team'));
    m.followOwner(accountId: 'account-a', teamId: 'team-b');
    expect(m.view.value.items, isEmpty);
    m.view.value = inbox(message('signed-in owner'));
    m.followOwner();
    expect(m.view.value.items, isEmpty);
  });

  test('late list response cannot populate the replacement owner', () async {
    final response = Completer<rust.UiMessagePage>();
    api.load = (_) => response.future;
    final m = state()..followOwner(accountId: 'account-a');
    final loading = m.reload();
    m.followOwner(accountId: 'account-b');
    final replacement = message('new owner');
    m.view.value = inbox(replacement);
    response.complete(
      rust.UiMessagePage(items: [message('old owner')], total: 1),
    );
    await loading;
    expect(m.view.value.items.single, replacement);
  });

  test(
    'late read failure cannot roll back a colliding new-owner message id',
    () async {
      final response = Completer<void>();
      api.write = () => response.future;
      final m = state()..followOwner(accountId: 'account-a');
      final old = message('old owner');
      m.view.value = inbox(old);
      final writing = m.setRead(old, true);
      m.followOwner(accountId: 'account-b');
      final replacement = message('new owner', read: true);
      m.view.value = inbox(replacement);
      response.completeError(StateError('old write failed'));
      await writing;
      expect(m.view.value.items.single, replacement);
      expect(errors, isEmpty);
    },
  );

  test('late mark-all failure cannot restore the old owner list', () async {
    final response = Completer<void>();
    api.write = () => response.future;
    final m = state()..followOwner(accountId: 'account-a');
    m.view.value = inbox(message('old owner'));
    final writing = m.markAllRead();
    m.followOwner(accountId: 'account-b');
    final replacement = message('new owner');
    m.view.value = inbox(replacement);
    response.completeError(StateError('old write failed'));
    await writing;
    expect(m.view.value.items.single, replacement);
    expect(errors, isEmpty);
  });

  test(
    'an old lookup cannot continue paging under the replacement owner',
    () async {
      final response = Completer<rust.UiMessagePage>();
      var calls = 0;
      api.load = (_) {
        calls++;
        if (calls != 1) throw StateError('old lookup crossed account boundary');
        return response.future;
      };
      final m = state()..followOwner(accountId: 'account-a');
      final lookup = m.locate(99);
      m.followOwner(accountId: 'account-b');
      m.view.value = MessagesView(
        items: [message('new owner')],
        total: 100,
        loaded: true,
      );
      response.complete(
        rust.UiMessagePage(items: [message('old owner')], total: 100),
      );
      expect(await lookup, isNull);
      expect(calls, 1);
      expect(m.view.value.items.single.title, 'new owner');
    },
  );

  testWidgets('a mounted inbox drops a late old-owner push fallback', (
    tester,
  ) async {
    tester.view.physicalSize = const Size(1280, 900);
    tester.view.devicePixelRatio = 1;
    addTearDown(tester.view.resetPhysicalSize);
    addTearDown(tester.view.resetDevicePixelRatio);
    final app = sampleState();
    final m = app.messages;
    m.followOwner(accountId: 'account-a');
    m.view.value = inbox(message('old list'));
    await tester.pumpWidget(
      AppearanceScope(
        notifier: Settings.appearance,
        child: WidgetsApp(
          color: Tokens.light.accent,
          localizationsDelegates: FLocalizations.localizationsDelegates,
          supportedLocales: FLocalizations.supportedLocales,
          locale: const Locale('en'),
          onGenerateRoute: (settings) => PageRouteBuilder<void>(
            pageBuilder: (context, _, _) {
              final t = Tokens.of(context);
              return FTheme(
                data: t.forui,
                child: DefaultTextStyle(
                  style: t.body,
                  child: MessagesPage(app: app),
                ),
              );
            },
          ),
        ),
      ),
    );
    final old = message('old private push');
    m.requestOpen((messageId: old.id, push: old));
    m.followOwner(accountId: 'account-b');
    m.view.value = inbox(message('new owner inbox'));
    await tester.pump();
    expect(find.text('old private push'), findsNothing);
    expect(find.text('new owner inbox'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox.shrink());
  });
}
