import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/pages/logs.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/settings.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/src/rust/frb_generated.dart';
import 'package:ppvpn_flutter/theme.dart';

class _Api extends Fake implements RustLibApi {
  late String clientLogs;
  @override
  String crateApiLogDir() => clientLogs;
}

Future<void> _visible(WidgetTester tester, String text) async {
  for (var n = 0; n < 40; n++) {
    await tester.pump(const Duration(milliseconds: 100));
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 10)),
    );
    await tester.pump();
    if (find.text(text).evaluate().isNotEmpty) return;
  }
  expect(find.text(text), findsOneWidget);
}

Future<void> _select(WidgetTester tester, String selected, String next) async {
  await tester.tap(find.text(selected).first);
  await tester.pump(const Duration(milliseconds: 300));
  await tester.tap(find.text(next).last);
  await tester.pump(const Duration(milliseconds: 300));
}

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();
  final api = _Api();
  setUpAll(() async {
    RustLib.initMock(api: api);
    for (final font in [
      ('PPVPN Material', 'assets/fonts/MaterialSymbols.ttf'),
      ('PPVPN Material Filled', 'assets/fonts/MaterialSymbolsFilled.ttf'),
      ('packages/forui/Inter', 'packages/forui/assets/fonts/inter/Inter.ttf'),
      ('JetBrains Mono', 'assets/fonts/JetBrainsMono-Regular.ttf'),
      (
        'packages/forui_lucide/ForuiLucideIcons',
        'packages/forui_lucide/assets/lucide.ttf',
      ),
    ]) {
      await (FontLoader(font.$1)..addFont(rootBundle.load(font.$2))).load();
    }
  });

  test(
    'mobile core names are size archives, client and desktop stay UTC daily',
    () {
      final now = DateTime.utc(2026, 11, 8);
      expect(logChoices(LogKind.core, mobile: true), [
        LogDay.current,
        LogDay.previous,
      ]);
      expect(logChoices(LogKind.client, mobile: true), [
        LogDay.today,
        LogDay.yesterday,
      ]);
      expect(logChoices(LogKind.core, mobile: false), [
        LogDay.today,
        LogDay.yesterday,
      ]);
      expect(logFileName(LogKind.core, LogDay.current, now), 'ppvpn-core.log');
      expect(
        logFileName(LogKind.core, LogDay.previous, now),
        'ppvpn-core.log.1',
      );
      expect(logDayKey(LogDay.previous), 'logPrevious');
      expect(
        logFileName(LogKind.client, LogDay.today, now),
        'ppvpn-client.2026-11-08.log',
      );
      expect(
        logFileName(LogKind.core, LogDay.yesterday, now),
        'ppvpn-core.2026-11-07.log',
      );
      for (final base in [
        '/data/user/0/com.peakpassvpn.ppvpn/files',
        '/private/var/mobile/Containers/Shared/AppGroup/group',
      ]) {
        expect(hostCoreLogDirectory(base), '$base/tunnel-state/logs');
      }
    },
  );

  test('real files distinguish missing, empty, readable and unreadable; disappearance clears stale data', () async {
    final dir = await Directory.systemTemp.createTemp('ppvpn-log-states-');
    try {
      final file = File('${dir.path}/ppvpn-core.log');
      final tail = LogTail(file.path);
      expect((await tail.poll()).state, LogReadState.missing);
      await file.writeAsString('');
      expect((await tail.poll()).state, LogReadState.empty);
      await file.writeAsString('real core entry\n');
      final ready = await tail.poll();
      expect(ready.state, LogReadState.ready);
      expect(ready.lines, ['real core entry']);
      await file.delete();
      final missing = await tail.poll();
      expect(missing.state, LogReadState.missing);
      expect(missing.reset, isTrue);
      await Directory(file.path).create();
      expect((await tail.poll()).state, LogReadState.unreadable);
    } finally {
      await dir.delete(recursive: true);
    }
  });

  test('same-prefix larger replacement and equal-size archive rotation reset the real tail', () async {
    final dir = await Directory.systemTemp.createTemp('ppvpn-log-rotation-');
    try {
      final current = File('${dir.path}/ppvpn-core.log');
      final archive = File('${current.path}.1');
      final header = '${'shared header ' * 12}\n';
      final old = '$header${'OLD' * 40}\n';
      await current.writeAsString(old);
      final tail = LogTail(current.path);
      expect((await tail.poll()).lines.last, 'OLD' * 40);
      await current.rename(archive.path);
      await current.writeAsString('$header${'NEW' * 60}\n');
      final replaced = await tail.poll();
      expect(replaced.reset, isTrue);
      expect(replaced.lines.last, 'NEW' * 60);
      final previous = LogTail(archive.path);
      expect((await previous.poll()).lines.last, 'OLD' * 40);
      await archive.writeAsString('$header${'ALT' * 40}\n');
      final rolled = await previous.poll();
      expect(rolled.reset, isTrue);
      expect(rolled.lines.last, 'ALT' * 40);
      await archive.writeAsString('next\n', mode: FileMode.append);
      final appended = await previous.poll();
      expect(appended.reset, isFalse);
      expect(appended.lines, ['next']);
    } finally {
      await dir.delete(recursive: true);
    }
  });

  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets(
      '$platform shows real host core files, switches source and distinguishes missing/empty/error',
      (tester) async {
        final root = await tester.runAsync(
          () => Directory.systemTemp.createTemp('ppvpn-log-view-'),
        );
        final directory = root!;
        api.clientLogs = '${directory.path}/client-logs';
        final files = '${directory.path}/native-files';
        final coreDir = hostCoreLogDirectory(files);
        final current = File('$coreDir/ppvpn-core.log');
        final archive = File('${current.path}.1');
        final now = DateTime.now().toUtc();
        final client = File(
          '${api.clientLogs}/${logFileName(LogKind.client, LogDay.today, now)}',
        );
        await tester.runAsync(() async {
          await current.parent.create(recursive: true);
          await client.parent.create(recursive: true);
          await client.writeAsString('ACTUAL_CLIENT_DAILY\n');
          await current.writeAsString('ACTUAL_HOST_CORE\n');
          await archive.writeAsString('ACTUAL_PREVIOUS_CORE\n');
          await File(
            '${api.clientLogs}/${logFileName(LogKind.core, LogDay.today, now)}',
          ).writeAsString('STALE_APP_CORE_DAILY\n');
        });
        var nativeDirectoryReads = 0;
        const channel = MethodChannel('com.peakpassvpn.ppvpn/app');
        final messenger =
            TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
        messenger.setMockMethodCallHandler(channel, (call) async {
          expect(call.method, 'filesDir');
          nativeDirectoryReads++;
          return files;
        });
        debugDefaultTargetPlatformOverride = platform;
        samplePage = null;
        Settings.language.value = rust.UiLanguage.english;
        tester.view.devicePixelRatio = 1;
        tester.view.physicalSize = const Size(320, 568);
        try {
          await tester.pumpWidget(
            AppearanceScope(
              notifier: Settings.appearance,
              child: WidgetsApp(
                color: Tokens.light.accent,
                localizationsDelegates: FLocalizations.localizationsDelegates,
                supportedLocales: FLocalizations.supportedLocales,
                locale: const Locale('en'),
                pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
                  settings: settings,
                  pageBuilder: (context, _, __) => builder(context),
                ),
                home: Builder(
                  builder: (context) {
                    final t = Tokens.of(context);
                    return MediaQuery(
                      data: MediaQuery.of(context).copyWith(
                        textScaler: TextScaler.linear(1.6),
                        disableAnimations: true,
                      ),
                      child: FTheme(
                        data: t.forui,
                        child: DefaultTextStyle(
                          style: t.body,
                          child: const LogsPage(),
                        ),
                      ),
                    );
                  },
                ),
              ),
            ),
          );
          await _visible(tester, 'ACTUAL_CLIENT_DAILY');
          await _select(tester, 'Client · Today', 'Core · Current');
          await _visible(tester, 'ACTUAL_HOST_CORE');
          expect(find.text(current.path), findsOneWidget);
          expect(find.text('STALE_APP_CORE_DAILY'), findsNothing);
          await _select(tester, 'Core · Current', 'Core · Previous file');
          await _visible(tester, 'ACTUAL_PREVIOUS_CORE');
          expect(find.text('Core · Yesterday'), findsNothing);
          await _select(tester, 'Core · Previous file', 'Client · Today');
          await _visible(tester, 'ACTUAL_CLIENT_DAILY');
          await _select(tester, 'Client · Today', 'Core · Current');
          await _visible(tester, 'ACTUAL_HOST_CORE');
          await tester.runAsync(() => current.delete());
          await _visible(tester, 'This log has not been written yet');
          expect(find.text('ACTUAL_HOST_CORE'), findsNothing);
          await tester.runAsync(() => current.writeAsString(''));
          await _visible(tester, 'This log file is empty');
          await tester.runAsync(() async {
            await current.delete();
            await Directory(current.path).create();
          });
          await _visible(tester, 'Unable to read this log');
          expect(nativeDirectoryReads, 1);
          expect(tester.takeException(), isNull);
        } finally {
          await tester.pumpWidget(const SizedBox.shrink());
          await tester.runAsync(() => directory.delete(recursive: true));
          messenger.setMockMethodCallHandler(channel, null);
          debugDefaultTargetPlatformOverride = null;
          tester.view.reset();
        }
      },
    );
  }
}
