import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/material.dart' show SelectableText;
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/icons.dart';
import 'package:ppvpn_flutter/phone/sign_in.dart';
import 'package:ppvpn_flutter/node_identity.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/phone/nodes.dart';
import 'package:ppvpn_flutter/phone/messages.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/theme.dart';
import 'package:ppvpn_flutter/widgets.dart' show Pressable, lineNames;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async {
    for (final (family, path) in [
      ('PPVPN Material', 'assets/fonts/MaterialSymbols.ttf'),
      ('PPVPN Material Filled', 'assets/fonts/MaterialSymbolsFilled.ttf'),
      ('packages/forui/Inter', 'packages/forui/assets/fonts/inter/Inter.ttf'),
      ('JetBrains Mono', 'assets/fonts/JetBrainsMono-Regular.ttf'),
    ]) {
      await (FontLoader(family)..addFont(rootBundle.load(path))).load();
    }
    final font = File(Platform.environment['PPVPN_UI_TEST_CJK_FONT']!);
    await (FontLoader(
      'PingFang SC',
    )..addFont(font.readAsBytes().then(ByteData.sublistView))).load();
  });
  setUp(() {
    samplePage = null;
    Settings.language.value = rust.UiLanguage.chinese;
  });
  rust.UiNode node(String name) => rust.UiNode(
    id: 'n',
    name: name,
    entryKey: 'premium',
    entryLabel: 'Premium',
    udp: true,
    lines: const [],
  );
  test(
    'node identity separates valid IPv4 and IPv6 and preserves ordinary names',
    () {
      expect(nodeIdentity(node('Los Angeles Native IP 203.0.113.12')), (
        name: 'Los Angeles Native IP',
        address: '203.0.113.12',
      ));
      expect(nodeIdentity(node('Tokyo [2001:db8::1]')), (
        name: 'Tokyo',
        address: '2001:db8::1',
      ));
      expect(nodeIdentity(node('Hong Kong Route 2')).address, isNull);
      expect(nodeIdentity(node('Los Angeles Native IP 203.0.113.12 ①')), (
        name: 'Los Angeles Native IP ①',
        address: '203.0.113.12',
      ));
      expect(nodeIdentity(node('Tokyo [2001:db8::1] (21)')), (
        name: 'Tokyo (21)',
        address: '2001:db8::1',
      ));
    },
  );
  test('selected symbols keep the same glyph as the outline', () {
    for (final icon in [AppIcons.shield, AppIcons.globe, AppIcons.user]) {
      expect(filledIcon(icon).codePoint, icon.codePoint);
      expect(filledIcon(icon).fontFamily, 'PPVPN Material Filled');
    }
    expect(nodeKind(node('Los Angeles Native IP 203.0.113.12')), 'Native IP');
    expect(
      nodeDisplayName(node('Los Angeles Native IP 203.0.113.12')),
      'Los Angeles',
    );
  });
  test(
    'route names preserve backend labels and never expose endpoint keys',
    () {
      final named = rust.UiNode(
        id: 'n',
        name: 'Test',
        entryKey: 'standard',
        udp: true,
        lines: const [
          rust.UiLine(key: 'internal-a', label: '广州移动'),
          rust.UiLine(key: 'internal-b'),
        ],
      );
      expect(lineNames(named), ['广州移动', '备用线路 1']);
      final unnamed = rust.UiNode(
        id: 'n',
        name: 'Test',
        entryKey: 'standard',
        udp: true,
        lines: const [
          rust.UiLine(key: 'internal-a'),
          rust.UiLine(key: 'internal-b'),
        ],
      );
      expect(lineNames(unnamed), ['首选线路', '备用线路 1']);
    },
  );
  test('traffic starts with a full zero-filled window', () {
    final app = AppState.fixed(sampleState().snapshot.value);
    expect(app.history.value.length, AppState.historyLength);
    expect(app.history.value.every((s) => s.up == 0 && s.down == 0), isTrue);
  });
  Widget host(Widget child) => WidgetsApp(
    localizationsDelegates: FLocalizations.localizationsDelegates,
    color: const Color(0xffffffff),
    pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
      settings: settings,
      pageBuilder: (context, _, _) => builder(context),
    ),
    home: FTheme(
      data: Tokens.light.forui,
      child: DefaultTextStyle(style: Tokens.light.body, child: child),
    ),
  );
  testWidgets(
    'nodes default to Ping and stay selected in their original group',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final app = sampleState();
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      const channel = MethodChannel('com.peakpassvpn.ppvpn/app');
      messenger.setMockMethodCallHandler(
        channel,
        (_) async => throw MissingPluginException(),
      );
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      await tester.pumpWidget(host(PhoneNodes(app: app)));
      await tester.pump();
      expect(find.text('Ping'), findsOneWidget);
      expect(find.text('当前节点'), findsNothing);
      expect(find.text('Hong Kong 01'), findsOneWidget);
      expect(
        tester
            .widget<Pressable>(find.byKey(const ValueKey('node-hk1')))
            .selected,
        isTrue,
      );
      await tester.longPress(find.byKey(const ValueKey('node-hk1')));
      await tester.pumpAndSettle();
      expect(find.text('选择实际线路'), findsOneWidget);
      expect(find.text('自动选择（故障切换）'), findsOneWidget);
      await tester.tap(find.text('HKG-B'));
      await tester.pumpAndSettle();
      expect(app.pinnedLine('hk1'), 'HKG-B');
      await tester.longPress(find.byKey(const ValueKey('node-hk1')));
      await tester.pumpAndSettle();
      await tester.tap(find.text('自动选择（故障切换）'));
      await tester.pumpAndSettle();
      expect(app.pinnedLine('hk1'), isNull);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('tap and long press have distinct mobile feedback', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final calls = <String>[];
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    messenger.setMockMethodCallHandler(SystemChannels.platform, (call) async {
      if (call.method == 'HapticFeedback.vibrate')
        calls.add(call.arguments as String);
      return null;
    });
    addTearDown(
      () => messenger.setMockMethodCallHandler(SystemChannels.platform, null),
    );
    await tester.pumpWidget(
      host(
        Center(
          child: Pressable(
            onPress: () {},
            onLongPress: () {},
            builder: (_, _) =>
                const SizedBox(width: 120, height: 50, child: Text('touch')),
          ),
        ),
      ),
    );
    await tester.tap(find.text('touch'));
    await tester.pumpAndSettle();
    await tester.longPress(find.text('touch'));
    await tester.pumpAndSettle();
    expect(calls, [
      'HapticFeedbackType.selectionClick',
      'HapticFeedbackType.mediumImpact',
    ]);
    debugDefaultTargetPlatformOverride = null;
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets(
    'native route popup applies selection and preserves pins on cancel',
    (tester) async {
      tester.view.physicalSize = const Size(375, 812);
      tester.view.devicePixelRatio = 1;
      addTearDown(tester.view.reset);
      final app = sampleState();
      final channel = const MethodChannel('com.peakpassvpn.ppvpn/app');
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      final requests = <Map<dynamic, dynamic>>[];
      int? answer = 2;
      messenger.setMockMethodCallHandler(channel, (call) async {
        if (call.method == 'chooseLine') {
          requests.add(call.arguments as Map);
          return answer;
        }
        return null;
      });
      addTearDown(() => messenger.setMockMethodCallHandler(channel, null));
      await tester.pumpWidget(host(PhoneNodes(app: app)));
      await tester.longPress(find.byKey(const ValueKey('node-hk1')));
      await tester.pumpAndSettle();
      expect(requests.single['options'], [
        '自动选择（故障切换）',
        'HKG-A',
        'HKG-B',
        'SZX-R',
      ]);
      expect(app.pinnedLine('hk1'), 'HKG-B');
      expect(find.text('选择实际线路'), findsNothing);
      answer = null;
      await tester.longPress(find.byKey(const ValueKey('node-hk1')));
      await tester.pumpAndSettle();
      expect(requests.last['selected'], 2);
      expect(app.pinnedLine('hk1'), 'HKG-B');
      await tester.pumpWidget(const SizedBox());
    },
  );
  testWidgets('restoring a stored session never shows sign-in', (tester) async {
    final app = AppState.fixed(
      const rust.UiSnapshot(
        auth: rust.UiAuth.restoring,
        profile: rust.UiProfile.loading,
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
      ),
    );
    await tester.pumpWidget(host(PhoneShell(app: app)));
    expect(find.byKey(const ValueKey('restoring-session')), findsOneWidget);
    expect(find.byType(PhoneSignIn), findsNothing);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets('node details open by touch without selecting the row', (
    tester,
  ) async {
    var selections = 0;
    await tester.pumpWidget(
      host(
        Center(
          child: SizedBox(
            width: 250,
            child: Pressable(
              onPress: () => selections++,
              builder: (context, _) => NodeIdentity(
                node: node('Los Angeles 203.0.113.12'),
                phone: true,
                tag: true,
              ),
            ),
          ),
        ),
      ),
    );
    expect(find.text('Los Angeles'), findsOneWidget);
    expect(find.text('203.0.113.12'), findsOneWidget);
    final nameRect = tester.getRect(find.text('Los Angeles'));
    final tagRect = tester.getRect(find.text('Premium'));
    expect(tagRect.center.dy, closeTo(nameRect.center.dy, 1));
    expect(
      tester.getRect(find.text('203.0.113.12')).top,
      greaterThanOrEqualTo(nameRect.bottom),
    );

    await tester.tap(find.text('Premium'));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('出口 IP: 203.0.113.12'), findsOneWidget);
    expect(selections, 0);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
  testWidgets(
    'a read-only push never mutates an inbox message with the same id',
    (tester) async {
      final app = sampleState();
      final message = app.messages.view.value.items.first;
      expect(message.read, isFalse);
      app.messages.requestOpen((messageId: null, push: message));
      await tester.pumpWidget(
        host(
          PhoneScaffold(
            title: '消息中心',
            child: PhoneMessagesPage(app: app),
          ),
        ),
      );
      await tester.pump(const Duration(milliseconds: 600));
      await tester.pump(const Duration(milliseconds: 600));
      expect(app.messages.view.value.items.first.read, isFalse);
      expect(find.text('标为已读'), findsNothing);
      expect(find.text('标为未读'), findsNothing);
      expect(tester.takeException(), isNull);
      await tester.pumpWidget(const SizedBox());
    },
  );
  for (final scale in [1.0, 1.6]) {
    testWidgets(
      'phone inbox pushes readable detail and marks read at scale $scale',
      (tester) async {
        tester.view.physicalSize = const Size(375, 812);
        tester.view.devicePixelRatio = 1;
        addTearDown(tester.view.reset);
        final app = sampleState();
        final message = app.messages.view.value.items.first;
        await tester.pumpWidget(
          WidgetsApp(
            localizationsDelegates: FLocalizations.localizationsDelegates,
            color: const Color(0xffffffff),
            pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
              settings: settings,
              pageBuilder: (context, _, _) => builder(context),
            ),
            home: FTheme(
              data: Tokens.light.forui,
              child: MediaQuery(
                data: MediaQueryData(
                  size: const Size(375, 812),
                  textScaler: TextScaler.linear(scale),
                ),
                child: DefaultTextStyle(
                  style: Tokens.light.body,
                  child: PhoneScaffold(
                    title: '消息中心',
                    child: PhoneMessagesPage(app: app),
                  ),
                ),
              ),
            ),
          ),
        );
        await tester.pump(const Duration(milliseconds: 100));
        await tester.tap(find.text(message.title).first);
        await tester.pump(const Duration(milliseconds: 500));
        expect(find.byType(SelectableText), findsWidgets);
        expect(app.messages.view.value.items.first.read, isTrue);
        expect(tester.takeException(), isNull);
        await tester.pumpWidget(const SizedBox());
      },
    );
  }
}
