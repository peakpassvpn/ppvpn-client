import 'dart:io';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/material.dart' show SelectableText;
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/icons.dart';
import 'package:ppvpn_flutter/phone/sign_in.dart';
import 'package:ppvpn_flutter/node_identity.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/phone/messages.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/theme.dart';

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
    await tester.pumpWidget(
      host(
        Center(
          child: SizedBox(
            width: 250,
            child: NodeIdentity(
              node: node('Los Angeles 203.0.113.12'),
              phone: true,
              tag: true,
            ),
          ),
        ),
      ),
    );
    expect(find.text('Los Angeles'), findsOneWidget);
    expect(find.text('203.0.113.12'), findsOneWidget);
    await tester.tap(find.text('Premium'));
    await tester.pump(const Duration(milliseconds: 400));
    expect(find.text('出口 IP: 203.0.113.12'), findsOneWidget);
    expect(tester.takeException(), isNull);
    await tester.pumpWidget(const SizedBox());
  });
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
