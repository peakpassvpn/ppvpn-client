import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/node_identity.dart';
import 'package:ppvpn_flutter/src/rust/api.dart' as rust;
import 'package:ppvpn_flutter/theme.dart';

void main() {
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
  }
}
