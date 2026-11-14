// Rendering-only regression coverage. Fixed fixtures: no VPN or backend.
import 'dart:io';
import 'dart:ui' show ImageByteFormat;

import 'package:flutter/widgets.dart';
import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/pages/overview.dart' show statusTitle;
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/theme.dart';
import 'package:ppvpn_flutter/node_identity.dart' show NodeInfoTag;
import 'package:ppvpn_flutter/widgets.dart' show linesText;

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async {
    await (FontLoader(
      'PPVPN Material',
    )..addFont(rootBundle.load('assets/fonts/MaterialSymbols.ttf'))).load();
    await (FontLoader('PPVPN Material Filled')
          ..addFont(rootBundle.load('assets/fonts/MaterialSymbolsFilled.ttf')))
        .load();
    final inter = FontLoader('packages/forui/Inter')
      ..addFont(rootBundle.load('packages/forui/assets/fonts/inter/Inter.ttf'));
    final mono = FontLoader('JetBrains Mono')
      ..addFont(rootBundle.load('assets/fonts/JetBrainsMono-Regular.ttf'));
    final icons = FontLoader('packages/forui_lucide/ForuiLucideIcons')
      ..addFont(rootBundle.load('packages/forui_lucide/assets/lucide.ttf'));
    await Future.wait([inter.load(), mono.load(), icons.load()]);
    final cjkPath = Platform.environment['PPVPN_UI_TEST_CJK_FONT'];
    if (cjkPath == null || !File(cjkPath).existsSync()) {
      throw StateError(
        'Set PPVPN_UI_TEST_CJK_FONT to a real CJK font; Ahem is not a layout baseline.',
      );
    }
    {
      final cjk = FontLoader('PingFang SC')
        ..addFont(
          File(cjkPath)
              .readAsBytes()
              .then((bytes) => ByteData.sublistView(bytes)),
        );
      await cjk.load();
    }
  });
  for (final size in [
    const Size(320, 568),
    const Size(375, 812),
    const Size(820, 1180),
  ]) {
    for (final brightness in Brightness.values) {
      for (final scale in [1.0, 1.6]) {
        for (final page in ['overview', 'nodes', 'settings']) {
          for (final tierLabel
              in page == 'nodes'
                  ? ['Premium', 'Premium · 优选全球网络线路']
                  : ['Premium · 优选全球网络线路']) {
            testWidgets('$page $size $brightness text=$scale tier=$tierLabel', (
              tester,
            ) async {
              final previousError = FlutterError.onError;
              FlutterError.onError = (details) {
                previousError?.call(details);
              };
              addTearDown(() => FlutterError.onError = previousError);
              tester.view.devicePixelRatio = 1;
              tester.view.physicalSize = size;
              tester.view.padding = const FakeViewPadding(top: 50, bottom: 34);
              addTearDown(tester.view.reset);
              samplePage = page;
              forcedBrightness = brightness;
              Settings.language.value = rust.UiLanguage.chinese;
              final app = sampleState();
              final n = app.nodes.value.first;
              app.nodes.value = [
                rust.UiNode(
                  id: n.id,
                  name: '香港高级线路 超长节点名称 Hong Kong Long Display Name',
                  exitRegion: n.exitRegion,
                  exitCountryCode: n.exitCountryCode,
                  entryLabel: tierLabel,
                  entryKey: n.entryKey,
                  udp: n.udp,
                  lines: n.lines,
                ),
                ...app.nodes.value.skip(1),
              ];
              final screenshotKey = GlobalKey();
              await tester.pumpWidget(
                RepaintBoundary(
                  key: screenshotKey,
                  child: AppearanceScope(
                    notifier: Settings.appearance,
                    child: WidgetsApp(
                      color: Tokens.light.accent,
                      debugShowCheckedModeBanner: false,
                      localizationsDelegates:
                          FLocalizations.localizationsDelegates,
                      supportedLocales: FLocalizations.supportedLocales,
                      locale: const Locale('zh'),
                      pageRouteBuilder: <T>(settings, builder) =>
                          PageRouteBuilder<T>(
                            settings: settings,
                            pageBuilder: (context, _, _) => builder(context),
                          ),
                      home: Builder(
                        builder: (context) {
                          final t = Tokens.of(context);
                          return MediaQuery(
                            data: MediaQuery.of(context).copyWith(
                              textScaler: TextScaler.linear(scale),
                              disableAnimations: true,
                            ),
                            child: FTheme(
                              data: t.forui,
                              child: DefaultTextStyle(
                                style: t.body,
                                child: Center(
                                  child: ConstrainedBox(
                                    constraints: const BoxConstraints(
                                      maxWidth: Tokens.phoneMaxWidth,
                                    ),
                                    child: PhoneShell(app: app),
                                  ),
                                ),
                              ),
                            ),
                          );
                        },
                      ),
                    ),
                  ),
                ),
              );
              await tester.pump(const Duration(milliseconds: 300));
              final screenshotDir =
                  Platform.environment['PPVPN_UI_TEST_SCREENSHOT_DIR'];
              if (screenshotDir != null &&
                  size.width == 320 &&
                  scale == 1.6 &&
                  page != 'settings') {
                final boundary =
                    screenshotKey.currentContext!.findRenderObject()!
                        as RenderRepaintBoundary;
                await tester.runAsync(() async {
                  final image = await boundary.toImage();
                  final bytes = await image.toByteData(
                    format: ImageByteFormat.png,
                  );
                  await Directory(screenshotDir).create(recursive: true);
                  await File('$screenshotDir/$page-${brightness.name}.png')
                      .writeAsBytes(bytes!.buffer.asUint8List());
                  image.dispose();
                });
              }
              expect(tester.takeException(), isNull);
              final tabs = find.byKey(const ValueKey('tab-me'));
              expect(tabs, findsOneWidget);
              expect(
                tester.getRect(tabs).bottom,
                lessThanOrEqualTo(size.height - 34),
              );
              if (page == 'overview') {
                final status = find.text(statusTitle(app.snapshot.value));
                expect(status, findsOneWidget);
                final paragraph = tester.renderObject<RenderParagraph>(status);
                expect(
                  paragraph.didExceedMaxLines,
                  isFalse,
                  reason: 'The connection status must remain readable.',
                );
                final timer = find.textContaining(
                  RegExp(r'^\d{2,}:\d{2}:\d{2}$'),
                );
                expect(timer, findsOneWidget);
                expect(
                  tester.getRect(status).right,
                  lessThanOrEqualTo(tester.getRect(timer).left),
                );
              }
              if (page == 'nodes') {
                final rowTitle = find.text(app.nodes.value.first.name);
                final row = find
                    .ancestor(of: rowTitle, matching: find.byType(Row))
                    .last;
                final tier = find.descendant(
                  of: row,
                  matching: find.byType(NodeInfoTag),
                );
                final lines = find.descendant(
                  of: row,
                  matching: find.text(linesText(app.nodes.value.first)),
                );
                expect(tier, findsOneWidget);
                expect(lines, findsOneWidget);
                final tierRect = tester.getRect(tier);
                final lineRect = tester.getRect(lines);
                debugPrint(
                  'metadata tier=$tierLabel tierWidth=${tierRect.width} linesWidth=${lineRect.width}',
                );
                expect(tierRect.width, greaterThan(0));
                final lineParagraph = tester.renderObject<RenderParagraph>(
                  lines,
                );
                final firstLine = app.nodes.value.first.lines.first.label!;
                final firstLineBoxes = lineParagraph.getBoxesForSelection(
                  TextSelection(baseOffset: 0, extentOffset: firstLine.length),
                );
                expect(
                  firstLineBoxes,
                  isNotEmpty,
                  reason: 'The first line name must remain visible beside the tier.',
                );
                expect(
                  firstLineBoxes.last.right,
                  lessThanOrEqualTo(lineRect.width),
                );
                final firstLinePainter = TextPainter(
                  text: TextSpan(
                    text: firstLine,
                    style: tester.widget<Text>(lines).style,
                  ),
                  textDirection: TextDirection.ltr,
                  textScaler: lineParagraph.textScaler,
                )..layout();
                final visibleFirstLineWidth = firstLineBoxes.fold<double>(
                  0,
                  (width, box) => width + box.right - box.left,
                );
                expect(
                  visibleFirstLineWidth,
                  closeTo(firstLinePainter.width, 0.1),
                  reason: 'The complete first line name must be visible, not just an ellipsis.',
                );
                firstLinePainter.dispose();
                expect(lineRect.width, greaterThan(0));
                expect(
                  tierRect.right <= lineRect.left ||
                      tierRect.bottom <= lineRect.top,
                  isTrue,
                  reason:
                      'Metadata must be side by side or wrap without overlap.',
                );
                final input = find.byType(EditableText);
                await tester.enterText(input, 'Hong Kong Long');
                await tester.pump();
                expect(
                  find.text('香港高级线路 超长节点名称 Hong Kong Long Display Name'),
                  findsOneWidget,
                );
                expect(tester.takeException(), isNull);
              }
              await tester.pumpWidget(const SizedBox.shrink());
              await tester.pump();
            });
          }
        }
      }
    }
  }
}
