import 'dart:io';
import 'dart:ui' as ui;

import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/phone/nodes.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
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
  });

  for (final dark in [false, true]) {
    for (final position in ['first', 'middle', 'last', 'single']) {
      testWidgets(
        'selected $position outline stays inside ${dark ? 'dark' : 'light'} card',
        (tester) async {
          tester.view.physicalSize = const Size(375, 812);
          tester.view.devicePixelRatio = 1;
          addTearDown(tester.view.reset);
          final previousLanguage = Settings.language.value;
          final previousBrightness = forcedBrightness;
          final previousSamplePage = samplePage;
          addTearDown(() {
            Settings.language.value = previousLanguage;
            forcedBrightness = previousBrightness;
            samplePage = previousSamplePage;
          });
          Settings.language.value = rust.UiLanguage.english;
          forcedBrightness = dark ? Brightness.dark : Brightness.light;
          samplePage = null;
          final app = sampleState();
          rust.UiNode node(String id) => rust.UiNode(
            id: id,
            name: id == 'hk1' ? 'Selected Node' : 'Other Node',
            exitRegion: 'Test Region',
            exitCountryCode: 'HK',
            entryKey: 'premium',
            entryLabel: 'Premium',
            udp: true,
            lines: const [rust.UiLine(key: 'test-line', label: 'Test line')],
          );
          final order = switch (position) {
            'first' => ['hk1', 'second', 'third'],
            'middle' => ['first', 'hk1', 'third'],
            'last' => ['first', 'second', 'hk1'],
            _ => ['hk1'],
          };
          app.nodes.value = order.map(node).toList();
          final tokens = dark ? Tokens.dark : Tokens.light;
          const pageKey = ValueKey('node-shape-page');
          await tester.pumpWidget(
            WidgetsApp(
              color: tokens.bg,
              localizationsDelegates: FLocalizations.localizationsDelegates,
              pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
                settings: settings,
                pageBuilder: (context, _, _) => builder(context),
              ),
              home: FTheme(
                data: tokens.forui,
                child: DefaultTextStyle(
                  style: tokens.body,
                  child: RepaintBoundary(
                    key: pageKey,
                    child: PhoneNodes(app: app),
                  ),
                ),
              ),
            ),
          );
          await tester.pump(const Duration(milliseconds: 300));

          final row = find.byKey(const ValueKey('node-hk1'));
          final selected = find
              .descendant(of: row, matching: find.byType(AnimatedContainer))
              .first;
          final card = find
              .ancestor(of: row, matching: find.byType(PhoneCard))
              .first;
          final cardBox = find
              .descendant(of: card, matching: find.byType(Container))
              .first;
          final rowDecoration = tester
              .widget<AnimatedContainer>(selected)
              .decoration!;
          final cardDecoration = tester.widget<Container>(cardBox).decoration!;
          expect((cardDecoration as BoxDecoration).color, tokens.card);
          final offset =
              tester.getTopLeft(selected) - tester.getTopLeft(cardBox);
          final rowPath = rowDecoration.getClipPath(
            Offset.zero & tester.getSize(selected),
            TextDirection.ltr,
          );
          final cardPath = cardDecoration.getClipPath(
            Offset.zero & tester.getSize(cardBox),
            TextDirection.ltr,
          );

          // Follow the actual selected outline in its rendered position. A square
          // first/last outline leaves the card's curved clip and loses its stroke.
          for (final metric in rowPath.computeMetrics()) {
            for (var point = 0; point < 240; point++) {
              final tangent = metric.getTangentForOffset(
                metric.length * point / 240,
              )!;
              expect(
                cardPath.contains(tangent.position + offset),
                isTrue,
                reason:
                    '$position selection outline is cut by the card at ${tangent.position}',
              );
            }
          }
          expect(
            find.descendant(of: row, matching: find.text('Selected Node')),
            findsOneWidget,
          );
          expect(tester.takeException(), isNull);

          final output = Platform.environment['PPVPN_NODE_SHAPE_SCREENSHOTS'];
          if (output != null) {
            await tester.runAsync(() async {
              final boundary = tester.renderObject<RenderRepaintBoundary>(
                find.byKey(pageKey),
              );
              final picture = await boundary.toImage(pixelRatio: 2);
              final bytes = await picture.toByteData(
                format: ui.ImageByteFormat.png,
              );
              await Directory(output).create(recursive: true);
              await File('$output/${dark ? 'dark' : 'light'}-$position.png')
                  .writeAsBytes(bytes!.buffer.asUint8List());
              picture.dispose();
            });
          }
          await tester.pumpWidget(const SizedBox());
        },
      );
    }
  }
}
