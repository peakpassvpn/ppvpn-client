import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:ppvpn_flutter/phone/native_chrome.dart';

void main() {
  TestWidgetsFlutterBinding.ensureInitialized();

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
