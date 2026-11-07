import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_test/flutter_test.dart';
import 'package:forui/forui.dart';
import 'package:ppvpn_flutter/app_state.dart';
import 'package:ppvpn_flutter/phone/phone.dart';
import 'package:ppvpn_flutter/phone/messages.dart';
import 'package:ppvpn_flutter/sample.dart';
import 'package:ppvpn_flutter/theme.dart';

class _Observer extends NavigatorObserver {
  final pushes = <Route<dynamic>>[];
  final pops = <Route<dynamic>>[];
  @override
  void didPush(Route<dynamic> route, Route<dynamic>? previousRoute) =>
      pushes.add(route);
  @override
  void didPop(Route<dynamic> route, Route<dynamic>? previousRoute) =>
      pops.add(route);
}

void main() {
  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets(
      '$platform notification reuses the existing inbox beneath a detail',
      (tester) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final app = sampleState();
        final observer = _Observer();
        final navigator = GlobalKey<NavigatorState>();
        await tester.pumpWidget(
          WidgetsApp(
            color: const Color(0xffffffff),
            navigatorKey: navigator,
            navigatorObservers: [observer],
            localizationsDelegates: FLocalizations.localizationsDelegates,
            pageRouteBuilder: <T>(settings, builder) => PageRouteBuilder<T>(
              settings: settings,
              pageBuilder: (context, _, _) => builder(context),
            ),
            home: FTheme(
              data: Tokens.light.forui,
              child: DefaultTextStyle(
                style: Tokens.light.body,
                child: PhoneShell(app: app),
              ),
            ),
          ),
        );
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        final items = app.messages.view.value.items;
        // A cold route keeps the pending request until its inbox is mounted.
        app.showPage(WindowPage.messages);
        app.messages.requestOpen((messageId: null, push: items.first));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        await tester.pump(const Duration(seconds: 1));
        expect(
          find.byType(PhoneMessagesPage, skipOffstage: false),
          findsOneWidget,
        );
        final firstInbox = observer.pushes[1];
        expect(observer.pushes.length, 3);
        app.showPage(WindowPage.messages);
        app.messages.requestOpen((messageId: null, push: items.last));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(observer.pushes.length, 4);
        expect(find.text(items.last.content), findsOneWidget);
        expect(observer.pops.length, 1);
        expect(observer.pops, isNot(contains(firstInbox)));
        expect(
          find.byType(PhoneMessagesPage, skipOffstage: false),
          findsOneWidget,
        );
        navigator.currentState!.pop();
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(find.byType(PhoneMessagesPage), findsOneWidget);
        // Repeated page selection itself cannot stack another inbox.
        final before = observer.pushes.length;
        app.showPage(WindowPage.messages);
        app.showPage(WindowPage.messages);
        await tester.pump();
        expect(observer.pushes.length, before);
        // Superseded asynchronous locates may not push an older detail.
        app.messages.requestOpen((messageId: 1, push: items.first));
        app.messages.requestOpen((messageId: 2, push: items.last));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(observer.pushes.length, before + 1);
        navigator.currentState!.pop();
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        final beforeOwnerChange = observer.pushes.length;
        app.messages.requestOpen((messageId: 1, push: items.first));
        app.messages.followOwner(accountId: 'different', teamId: 'different');
        await tester.pump();
        expect(observer.pushes.length, beforeOwnerChange);
        // Leaving the inbox clears its route registration; it can open anew.
        navigator.currentState!.pop();
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        app.showPage(WindowPage.messages);
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(observer.pushes.length, beforeOwnerChange + 1);
        expect(observer.pushes.last, isNot(same(firstInbox)));
        expect(find.byType(PhoneMessagesPage), findsOneWidget);
        await tester.pumpWidget(const SizedBox());
        debugDefaultTargetPlatformOverride = null;
      },
    );
  }
}
