import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/node_identity.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/theme.dart';

void main() {
  setUp(() {
    Settings.language.value = rust.UiLanguage.chinese;
  });

  test('nodeDisplayName collapses double spaces after removing exit class', () {
    rust.UiNode make(String name) => rust.UiNode(
      id: 'n',
      name: name,
      entryKey: 'standard',
      udp: true,
      lines: const [],
    );

    expect(
      nodeDisplayName(make('Los Angeles Dual ISP CN Optimized 203.0.113.7')),
      'Los Angeles CN Optimized',
    );
    expect(
      nodeDisplayName(
        make('🇺🇸Los Angeles Residential IP Economy 203.0.113.8'),
      ),
      'Los Angeles Economy',
    );
    expect(nodeDisplayName(make('🇺🇸洛杉矶 双 ISP 三网优化 203.0.113.7')), '洛杉矶 三网优化');
    expect(nodeDisplayName(make('🇯🇵东京 原生 IP 经济 198.51.100.9 ①')), '东京 经济 ①');
  });

  test('nodeEntryLabel prefers entry_label, falls back to name match and entry_key, and distinguishes tray titles', () {
    final zhOpt = rust.UiNode(
      id: 'la-opt',
      name: '🇺🇸洛杉矶 双 ISP 203.0.113.7',
      entryKey: 'cn-optimized',
      entryLabel: '三网优化',
      udp: true,
      lines: const [],
    );
    final zhEco = rust.UiNode(
      id: 'la-eco',
      name: '🇺🇸洛杉矶 双 ISP 203.0.113.7',
      entryKey: 'lite',
      entryLabel: '经济',
      udp: true,
      lines: const [],
    );
    expect(nodeTagLabels(zhOpt, includeEntry: true), ['双 ISP', '三网优化']);
    expect(nodeTagLabels(zhEco, includeEntry: true), ['双 ISP', '经济']);
    expect(nodeTagLabels(zhOpt), ['双 ISP']);
    expect(nodeTagLabels(zhOpt, type: false), ['三网优化']);
    expect(trayNodeTitle(zhOpt), '洛杉矶 双 ISP 203.0.113.7 · 三网优化');
    expect(trayNodeTitle(zhEco), '洛杉矶 双 ISP 203.0.113.7 · 经济');
    expect(trayNodeTitle(zhOpt), isNot(trayNodeTitle(zhEco)));

    final enOpt = rust.UiNode(
      id: 'la-en-opt',
      name: '🇺🇸Los Angeles Dual ISP 203.0.113.7',
      entryKey: 'cn-optimized',
      entryLabel: 'CN Optimized',
      udp: true,
      lines: const [],
    );
    final enEco = rust.UiNode(
      id: 'la-en-eco',
      name: '🇺🇸Los Angeles Dual ISP 203.0.113.7',
      entryKey: 'lite-direct',
      entryLabel: 'Economy',
      udp: true,
      lines: const [],
    );
    expect(nodeTagLabels(enOpt, includeEntry: true), [
      'Dual ISP',
      'CN Optimized',
    ]);
    expect(nodeTagLabels(enEco, includeEntry: true), ['Dual ISP', 'Economy']);
    expect(
      trayNodeTitle(enOpt),
      'Los Angeles Dual ISP 203.0.113.7 · CN Optimized',
    );
    expect(trayNodeTitle(enEco), 'Los Angeles Dual ISP 203.0.113.7 · Economy');

    // Missing entry_label falls back to known entry words in the name first.
    final nameFallbackZh = rust.UiNode(
      id: 'fb-name-zh',
      name: '🇺🇸洛杉矶 双 ISP 三网优化 203.0.113.7',
      entryKey: 'standard',
      udp: true,
      lines: const [],
    );
    final nameFallbackEn = rust.UiNode(
      id: 'fb-name-en',
      name: 'Los Angeles Dual ISP Economy 203.0.113.7',
      entryKey: 'standard',
      udp: true,
      lines: const [],
    );
    expect(nodeEntryLabel(nameFallbackZh), '三网优化');
    expect(nodeEntryLabel(nameFallbackEn), 'Economy');
    // Tray title does not duplicate an entry word already present in the name.
    expect(trayNodeTitle(nameFallbackZh), '洛杉矶 双 ISP 三网优化 203.0.113.7');
    expect(
      trayNodeTitle(nameFallbackEn),
      'Los Angeles Dual ISP Economy 203.0.113.7',
    );

    // When both entry_label and name entry word are absent, finalized entry_key maps localize.
    Settings.language.value = rust.UiLanguage.chinese;
    expect(
      nodeEntryLabel(
        const rust.UiNode(
          id: 'k1',
          name: '🇺🇸洛杉矶 双 ISP 203.0.113.7',
          entryKey: 'cn-optimized',
          udp: true,
          lines: [],
        ),
      ),
      '三网优化',
    );
    expect(
      nodeEntryLabel(
        const rust.UiNode(
          id: 'k2',
          name: '🇺🇸洛杉矶 双 ISP 203.0.113.7',
          entryKey: 'lite-direct',
          udp: true,
          lines: [],
        ),
      ),
      '经济',
    );
    Settings.language.value = rust.UiLanguage.english;
    expect(
      nodeEntryLabel(
        const rust.UiNode(
          id: 'k3',
          name: '🇺🇸Los Angeles Dual ISP 203.0.113.7',
          entryKey: 'cn-optimized',
          udp: true,
          lines: [],
        ),
      ),
      'CN Optimized',
    );
    expect(
      nodeEntryLabel(
        const rust.UiNode(
          id: 'k4',
          name: '🇺🇸Los Angeles Dual ISP 203.0.113.7',
          entryKey: 'lite',
          udp: true,
          lines: [],
        ),
      ),
      'Economy',
    );
  });

  for (final width in [180.0, 260.0]) {
    testWidgets('node tag stays beside a single-line name at width $width', (
      tester,
    ) async {
      const name = 'Los Angeles Very Long Node Name';
      final node = rust.UiNode(
        id: 'n',
        name: '$name Native IP 203.0.113.1',
        entryKey: 'standard',
        udp: true,
        lines: const [],
      );
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
            child: Center(
              child: SizedBox(
                width: width,
                child: NodeIdentity(node: node, phone: true, tag: true),
              ),
            ),
          ),
        ),
      );
      await tester.pumpAndSettle();
      final title = find.text(name);
      final tag = find.text('Native IP');
      expect(tester.widget<Text>(title).maxLines, 1);
      expect(
        tester.getCenter(title).dy,
        closeTo(tester.getCenter(tag).dy, 0.01),
      );
      expect(
        tester.getRect(tag).left - tester.getRect(title).right,
        closeTo(13, 0.01),
      );
      expect(tester.takeException(), isNull);
    });

    testWidgets(
      'class and entry tags render side by side without overflow at width $width',
      (tester) async {
        final node = rust.UiNode(
          id: 'n-dual',
          name: 'Los Angeles Dual ISP 203.0.113.7',
          entryKey: 'cn-optimized',
          entryLabel: 'CN Optimized',
          udp: true,
          lines: const [],
        );
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
              child: Center(
                child: SizedBox(
                  width: width,
                  child: NodeIdentity(node: node, phone: true, tag: true),
                ),
              ),
            ),
          ),
        );
        await tester.pumpAndSettle();
        final title = find.text('Los Angeles');
        final classTag = find.text('Dual ISP');
        final entryTag = find.text('CN Optimized');
        expect(title, findsOneWidget);
        expect(classTag, findsOneWidget);
        expect(entryTag, findsOneWidget);
        expect(
          tester.getCenter(classTag).dy,
          closeTo(tester.getCenter(entryTag).dy, 0.01),
        );
        expect(
          tester.getRect(entryTag).left,
          greaterThan(tester.getRect(classTag).right),
        );
        expect(tester.takeException(), isNull);
      },
    );
  }
}
