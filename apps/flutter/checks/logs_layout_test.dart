// Fixed log fixtures: no client, tunnel, file polling or external actions.
import 'dart:io';

import 'package:flutter/rendering.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/pages/logs.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/strings.dart';
import 'package:ppvpn_flutter/theme.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  setUpAll(() async {
    final inter = FontLoader('packages/forui/Inter')
      ..addFont(rootBundle.load('packages/forui/assets/fonts/inter/Inter.ttf'));
    final mono = FontLoader('JetBrains Mono')
      ..addFont(rootBundle.load('assets/fonts/JetBrainsMono-Regular.ttf'));
    final icons = FontLoader('packages/forui_lucide/ForuiLucideIcons')
      ..addFont(rootBundle.load('packages/forui_lucide/assets/lucide.ttf'));
    final path = Platform.environment['PPVPN_UI_TEST_CJK_FONT'];
    if (path == null || !File(path).existsSync()) {
      throw StateError('PPVPN_UI_TEST_CJK_FONT must point to a real CJK font.');
    }
    final cjk = FontLoader('PingFang SC')
      ..addFont(File(path).readAsBytes().then(ByteData.sublistView));
    await Future.wait([inter.load(), mono.load(), icons.load(), cjk.load()]);
  });
  for (final size in [
    const Size(320, 568),
    const Size(375, 812),
    const Size(820, 1180),
  ]) {
    for (final scale in [1.0, 1.6]) {
      for (final language in [
        rust.UiLanguage.chinese,
        rust.UiLanguage.english,
      ]) {
        for (final brightness in Brightness.values) {
          testWidgets('logs $size text=$scale $language $brightness', (
            tester,
          ) async {
            tester.view.devicePixelRatio = 1;
            tester.view.physicalSize = size;
            tester.view.padding = const FakeViewPadding(top: 50, bottom: 34);
            addTearDown(tester.view.reset);
            samplePage = 'logs';
            forcedBrightness = brightness;
            Settings.language.value = language;
            final s = S.current;
            await tester.pumpWidget(
              AppearanceScope(
                notifier: Settings.appearance,
                child: WidgetsApp(
                  color: Tokens.light.accent,
                  localizationsDelegates: FLocalizations.localizationsDelegates,
                  supportedLocales: FLocalizations.supportedLocales,
                  locale: s.locale,
                  pageRouteBuilder: <T>(settings, builder) =>
                      PageRouteBuilder<T>(
                        settings: settings,
                        pageBuilder: (context, _, __) => builder(context),
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
                            child: PhoneScaffold(
                              title: s('logs'),
                              back: true,
                              child: const LogsPage(),
                            ),
                          ),
                        ),
                      );
                    },
                  ),
                ),
              ),
            );
            await tester.pump(const Duration(milliseconds: 300));
            expect(tester.takeException(), isNull);
            final choice = find.text('${s('client')} · ${s('today')}');
            final following = find.text(s('following'));
            for (final control in [choice, following]) {
              expect(control, findsOneWidget);
              final rect = tester.getRect(control);
              expect(rect.left, greaterThanOrEqualTo(16));
              expect(rect.right, lessThanOrEqualTo(size.width - 16));
              expect(
                tester.renderObject<RenderParagraph>(control).didExceedMaxLines,
                isFalse,
              );
            }
            final path = find.textContaining(
              logFileName(LogKind.client, LogDay.today, sampleLogDate),
            );
            expect(path, findsOneWidget);
            expect(
              tester.getRect(path).bottom,
              lessThanOrEqualTo(size.height - 34),
            );
            expect(find.text(sampleLogLines.first), findsOneWidget);
            if (size.width == 320 && scale == 1.6) {
              await tester.tap(choice);
              await tester.pump(const Duration(milliseconds: 300));
              expect(tester.takeException(), isNull);
              await tester.tapAt(Offset(4, size.height - 40));
              await tester.pump(const Duration(milliseconds: 300));
              expect(tester.takeException(), isNull);
            }
            await tester.drag(find.byType(ListView), const Offset(0, -200));
            await tester.pump();
            expect(tester.takeException(), isNull);
          });
        }
      }
    }
  }
}
