import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/phone/native_chrome.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

  testWidgets('both routes use Flutter chrome during push and pop', (
    tester,
  ) async {
    final navigator = GlobalKey<NavigatorState>();
    Widget page(String name) => RouteChrome(
      builder: (_, settled) => Text(
        '$name-${settled ? 'native' : 'flutter'}',
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
    expect(find.text('root-flutter'), findsOneWidget);
    expect(find.text('detail-flutter'), findsOneWidget);
    await tester.pumpAndSettle();
    expect(find.text('detail-native'), findsOneWidget);
    navigator.currentState!.pop();
    await tester.pump();
    await tester.pump(const Duration(milliseconds: 100));
    expect(find.text('root-flutter'), findsOneWidget);
    expect(find.text('detail-flutter'), findsOneWidget);
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
