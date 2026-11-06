import 'dart:ui' as ui;
import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/cupertino.dart'
    show CupertinoPageRoute, DefaultCupertinoLocalizations;
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/phone/native_chrome.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/theme.dart';
import 'package:ppvpn_flutter/widgets.dart';
import 'package:forui/forui.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets(
    'secondary navigation starts while native image work is pending',
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      NativeChrome.enabled = true;
      addTearDown(() {
        NativeChrome.enabled = false;
        debugDefaultTargetPlatformOverride = null;
      });
      final pending = Completer<Object?>();
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      final ids = <int>[];
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
        call,
      ) async {
        if (call.method == 'create') {
          final id = (call.arguments as Map)['id'] as int;
          ids.add(id);
          messenger.setMockMethodCallHandler(
            MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id'),
            (call) async {
              if (call.method == 'snapshot') return pending.future;
              return null;
            },
          );
        }
        return null;
      });
      addTearDown(() {
        messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
        for (final id in ids) {
          messenger.setMockMethodCallHandler(
            MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id'),
            null,
          );
        }
      });
      await tester.pumpWidget(
        WidgetsApp(
          color: const Color(0xff000000),
          localizationsDelegates: [
            ...FLocalizations.localizationsDelegates,
            DefaultCupertinoLocalizations.delegate,
          ],
          pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
            settings: settings,
            pageBuilder: (context, _, _) => builder(context),
          ),
          home: FTheme(
            data: Tokens.light.forui,
            child: Builder(
              builder: (context) => PhoneScaffold(
                title: 'Me',
                child: Center(
                  child: Pressable(
                    onPress: () => pushPhonePage<void>(
                      context,
                      const PhoneScaffold(
                        title: 'Detail',
                        back: true,
                        child: Text('detail content'),
                      ),
                    ),
                    builder: (_, _) => const Text('open'),
                  ),
                ),
              ),
            ),
          ),
        ),
      );
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 16));
      await tester.tap(find.text('open'));
      await tester.pump();
      await tester.pump(const Duration(milliseconds: 16));
      expect(pending.isCompleted, isFalse);
      expect(find.text('detail content'), findsOneWidget);
      final detailContext = tester.element(find.text('detail content'));
      final route = ModalRoute.of(detailContext)!;
      expect(route, isA<CupertinoPageRoute<void>>());
      expect(route.barrierColor, isNull);
      expect(route.allowSnapshotting, isFalse);
      pending.complete(null);
      await tester.pumpAndSettle();
      await tester.pumpWidget(const SizedBox());
      NativeChrome.enabled = false;
      debugDefaultTargetPlatformOverride = null;
    },
  );

  testWidgets('tabbar prewarms in the background and retains its UIKit view', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
    addTearDown(() => debugDefaultTargetPlatformOverride = null);
    final bytes = await tester.runAsync(() async {
      final recorder = ui.PictureRecorder();
      ui.Canvas(recorder).drawColor(const Color(0xff123456), ui.BlendMode.src);
      final picture = recorder.endRecording();
      final image = await picture.toImage(4, 4);
      final data = await image.toByteData(format: ui.ImageByteFormat.png);
      image.dispose();
      picture.dispose();
      return data!.buffer.asUint8List();
    });
    final messenger =
        TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
    var creates = 0;
    var snapshots = 0;
    int? id;
    messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
      call,
    ) async {
      if (call.method == 'create') {
        creates++;
        id = (call.arguments as Map)['id'] as int;
        messenger.setMockMethodCallHandler(
          MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id'),
          (call) async {
            if (call.method == 'snapshot') {
              snapshots++;
              return bytes;
            }
            return null;
          },
        );
      }
      return null;
    });
    addTearDown(() {
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
      messenger.setMockMethodCallHandler(
        MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id'),
        null,
      );
    });
    Widget view(bool frozen) => Directionality(
      textDirection: TextDirection.ltr,
      child: SizedBox(
        width: 375,
        height: 100,
        child: NativeChromeView(
          kind: 'tabs',
          configuration: const {'selected': 2},
          frozen: frozen,
          onAction: (_) {},
        ),
      ),
    );
    await tester.pumpWidget(view(false));
    await tester.pumpAndSettle();
    await tester.runAsync(
      () => Future<void>.delayed(const Duration(milliseconds: 50)),
    );
    await tester.pump();
    expect(snapshots, greaterThan(0));
    await tester.pumpWidget(view(true));
    await tester.pumpAndSettle();
    expect(find.byType(Image), findsOneWidget);
    expect(find.byType(UiKitView, skipOffstage: false), findsOneWidget);
    expect(creates, 1);
    await tester.pumpWidget(view(false));
    await tester.pumpAndSettle();
    expect(find.byType(Image), findsNothing);
    expect(creates, 1);
    await tester.pumpWidget(const SizedBox());
    debugDefaultTargetPlatformOverride = null;
  });

  testWidgets('both routes freeze native appearance during push and pop', (
    tester,
  ) async {
    final navigator = GlobalKey<NavigatorState>();
    Widget page(String name) => RouteChrome(
      builder: (_, settled) => Text(
        '$name-${settled ? 'native' : 'image'}',
        textDirection: TextDirection.ltr,
      ),
    );
    await tester.pumpWidget(
      WidgetsApp(
        color: const Color(0xff000000),
        navigatorKey: navigator,
        pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
          settings: settings,
          pageBuilder: (context, _, _) => builder(context),
        ),
        home: page('root'),
      ),
    );
    await tester.pumpAndSettle();
    expect(find.text('root-native'), findsOneWidget);
    navigator.currentState!.push(
      PageRouteBuilder<void>(
        transitionDuration: const Duration(milliseconds: 400),
        reverseTransitionDuration: const Duration(milliseconds: 400),
        pageBuilder: (_, _, _) => page('detail'),
      ),
    );
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('root-image'), findsOneWidget);
    expect(find.text('detail-image'), findsOneWidget);
    await tester.pumpAndSettle();
    expect(find.text('detail-native'), findsOneWidget);
    navigator.currentState!.pop();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('root-image'), findsOneWidget);
    expect(find.text('detail-image'), findsOneWidget);
    await tester.pumpAndSettle();
    expect(find.text('root-native'), findsOneWidget);
    await tester.pumpWidget(const SizedBox());
  });

  testWidgets(
    'native chrome updates configuration and dispatches current callbacks',
    (tester) async {
      debugDefaultTargetPlatformOverride = TargetPlatform.iOS;
      addTearDown(() => debugDefaultTargetPlatformOverride = null);
      final messenger =
          TestDefaultBinaryMessengerBinding.instance.defaultBinaryMessenger;
      int? viewId;
      final updates = <Map<dynamic, dynamic>>[];
      messenger.setMockMethodCallHandler(SystemChannels.platform_views, (
        call,
      ) async {
        if (call.method == 'create') {
          viewId = (call.arguments as Map)['id'] as int;
          messenger.setMockMethodCallHandler(
            MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$viewId'),
            (call) async {
              if (call.method == 'update') updates.add(call.arguments as Map);
              return null;
            },
          );
        }
        return null;
      });
      addTearDown(() {
        messenger.setMockMethodCallHandler(SystemChannels.platform_views, null);
        messenger.setMockMethodCallHandler(
          MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$viewId'),
          null,
        );
      });
      final first = <String>[];
      final second = <String>[];
      Widget view(int selection, ValueChanged<String> callback) =>
          Directionality(
            textDirection: TextDirection.ltr,
            child: SizedBox(
              width: 375,
              height: 84,
              child: NativeChromeView(
                kind: 'tabs',
                configuration: {'selected': selection},
                onAction: callback,
              ),
            ),
          );
      Future<void> action(Object? value) async {
        await messenger.handlePlatformMessage(
          'com.peakpassvpn.ppvpn/native-chrome/$viewId',
          const StandardMethodCodec().encodeMethodCall(
            MethodCall('action', value),
          ),
          (_) {},
        );
      }

      await tester.pumpWidget(view(0, first.add));
      await tester.pump();
      expect(viewId, isNotNull);
      expect(updates.last['selected'], 0);
      await action('tab:1');
      expect(first, ['tab:1']);
      await tester.pumpWidget(view(2, second.add));
      await tester.pump();
      expect(updates.last['selected'], 2);
      await action('tab:2');
      await action(1); // Malformed native events must not reach the callback.
      expect(first, ['tab:1']);
      expect(second, ['tab:2']);
      await tester.pumpWidget(const SizedBox());
      await action('tab:0');
      expect(second, ['tab:2']);
      expect(tester.takeException(), isNull);
      debugDefaultTargetPlatformOverride = null;
    },
  );
}
