import 'package:flutter/foundation.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter/material.dart' show SelectableText;
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

Widget _host(
  AppState app,
  _Observer observer,
  GlobalKey<NavigatorState> navigator,
) => WidgetsApp(
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
);

void main() {
  testWidgets('zero-duration completed push reveals the inbox before any pop', (
    tester,
  ) async {
    debugDefaultTargetPlatformOverride = TargetPlatform.android;
    tester.platformDispatcher.accessibilityFeaturesTestValue =
        const FakeAccessibilityFeatures(disableAnimations: true);
    addTearDown(() {
      debugDefaultTargetPlatformOverride = null;
      tester.platformDispatcher.clearAccessibilityFeaturesTestValue();
    });
    final app = sampleState();
    final observer = _Observer();
    final navigator = GlobalKey<NavigatorState>();
    await tester.pumpWidget(_host(app, observer, navigator));
    await tester.pump();
    app.showPage(WindowPage.messages, openingMessage: true);
    app.messages.requestOpen((
      messageId: null,
      push: app.messages.view.value.items.first,
    ));
    await tester.pump();
    await tester.pump();
    final detail = observer.pushes.last as PageRoute;
    expect(detail.transitionDuration, Duration.zero);
    expect(detail.animation!.status, AnimationStatus.completed);
    expect(detail.isCurrent, true);
    expect(observer.pops, isEmpty);
    final opacity = find.ancestor(
      of: find.byType(PhoneMessagesPage, skipOffstage: false),
      matching: find.byType(Opacity, skipOffstage: false),
    );
    expect(
      tester.widget<Opacity>(opacity.first).opacity,
      1,
      reason:
          'Already-completed push must reveal the back-stack list before pop',
    );
    await tester.pumpWidget(const SizedBox());
    debugDefaultTargetPlatformOverride = null;
    tester.platformDispatcher.clearAccessibilityFeaturesTestValue();
  });
  for (final platform in [TargetPlatform.iOS, TargetPlatform.android]) {
    testWidgets(
      '$platform notification reuses the existing inbox beneath a detail',
      (tester) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final app = sampleState();
        final observer = _Observer();
        final navigator = GlobalKey<NavigatorState>();
        await tester.pumpWidget(_host(app, observer, navigator));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        final items = app.messages.view.value.items;
        // A cold route keeps the pending request until its inbox is mounted.
        app.showPage(WindowPage.messages, openingMessage: true);
        app.messages.requestOpen((messageId: null, push: items.first));
        await tester.pump();
        // The first painted frame must retain the previous screen, with the
        // back-stack inbox hidden and no inbox entry animation.
        final inboxOpacity = find.ancestor(
          of: find.byType(PhoneMessagesPage),
          matching: find.byType(Opacity),
        );
        expect(
          inboxOpacity,
          findsOneWidget,
          reason: 'The initial inbox must be a hidden back-stack layer',
        );
        expect(tester.widget<Opacity>(inboxOpacity.first).opacity, 0);
        final inboxRoute = observer.pushes[1] as PageRoute;
        expect(inboxRoute.animation!.status, AnimationStatus.completed);
        expect(inboxRoute.opaque, false);
        expect((observer.pushes.first as PageRoute).offstage, false);
        await tester.pump();
        final detailRoute = observer.pushes[2] as PageRoute;
        var previousX = double.infinity;
        for (var frame = 0; frame < 24; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
          final x = tester.getTopLeft(find.byType(SelectableText)).dx;
          expect(x, lessThanOrEqualTo(previousX + 0.01));
          previousX = x;
          if (detailRoute.animation!.status != AnimationStatus.completed) {
            expect(tester.widget<Opacity>(inboxOpacity.first).opacity, 0);
          }
        }
        await tester.pump(const Duration(seconds: 1));
        expect(
          find.byType(PhoneMessagesPage, skipOffstage: false),
          findsOneWidget,
        );
        final firstInbox = observer.pushes[1];
        expect(observer.pushes.length, 3);
        app.showPage(WindowPage.messages, openingMessage: true);
        app.messages.requestOpen((messageId: null, push: items.last));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        // Updating an existing detail never visits the inbox or changes route.
        expect(observer.pushes.length, 3);
        expect(find.text(items.last.content), findsOneWidget);
        expect(observer.pops.length, 0);
        expect(observer.pushes.last, same(detailRoute));
        final beforeRepeat = observer.pushes.length;
        app.showPage(WindowPage.messages, openingMessage: true);
        app.messages.requestOpen((messageId: null, push: items.last));
        for (var frame = 0; frame < 24; frame++) {
          await tester.pump(const Duration(milliseconds: 16));
          expect(observer.pushes.length, beforeRepeat);
          expect(observer.pops, isEmpty);
          expect(find.text(items.last.content), findsOneWidget);
          expect(detailRoute.animation!.status, AnimationStatus.completed);
        }
        expect(observer.pops, isNot(contains(firstInbox)));
        expect(
          find.byType(PhoneMessagesPage, skipOffstage: false),
          findsOneWidget,
        );
        if (platform == TargetPlatform.iOS) {
          // Cancel an edge swipe, then commit it. Both retain the native route.
          final width =
              tester.view.physicalSize.width / tester.view.devicePixelRatio;
          final swipe = await tester.startGesture(const Offset(1, 300));
          await swipe.moveBy(Offset(width * .35, 0));
          await tester.pump(const Duration(milliseconds: 16));
          expect(navigator.currentState!.userGestureInProgress, true);
          await swipe.moveBy(Offset(-width * .34, 0));
          await swipe.up();
          await tester.pump();
          await tester.pump(const Duration(seconds: 1));
          expect(detailRoute.isCurrent, true);
          await tester.dragFrom(const Offset(1, 300), Offset(width * .85, 0));
        } else {
          navigator.currentState!.pop();
        }
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
        final beforeUserNavigation = observer.pushes.length;
        app.messages.requestOpen((messageId: 1, push: items.first));
        final cover = PageRouteBuilder<void>(
          pageBuilder: (context, _, _) => const Text('newer user navigation'),
        );
        navigator.currentState!.push<void>(cover);
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(cover.isCurrent, true);
        expect(observer.pushes.length, beforeUserNavigation + 1);
        navigator.currentState!.pop();
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        final beforeOwnerChange = observer.pushes.length;
        app.messages.requestOpen((messageId: 1, push: items.first));
        app.messages.followOwner(accountId: 'different', teamId: 'different');
        await tester.pump();
        expect(observer.pushes.length, beforeOwnerChange);
        // The notification-created inbox also keeps its native back gesture.
        if (platform == TargetPlatform.iOS) {
          final width =
              tester.view.physicalSize.width / tester.view.devicePixelRatio;
          await tester.dragFrom(const Offset(1, 300), Offset(width * .85, 0));
        } else {
          navigator.currentState!.pop();
        }
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        app.showPage(WindowPage.messages, openingMessage: true);
        app.messages.followOwner(
          accountId: 'another-owner',
          teamId: 'another-team',
        );
        await tester.pump();
        expect(observer.pushes.first.isCurrent, true);
        app.showPage(WindowPage.messages);
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        expect(observer.pushes.length, beforeOwnerChange + 2);
        expect(observer.pushes.last, isNot(same(firstInbox)));
        expect(find.byType(PhoneMessagesPage), findsOneWidget);
        await tester.pumpWidget(const SizedBox());
        debugDefaultTargetPlatformOverride = null;
      },
    );
    testWidgets(
      '$platform current detail is not popped for notification taps',
      (tester) async {
        debugDefaultTargetPlatformOverride = platform;
        addTearDown(() => debugDefaultTargetPlatformOverride = null);
        final app = sampleState();
        final observer = _Observer();
        final navigator = GlobalKey<NavigatorState>();
        await tester.pumpWidget(_host(app, observer, navigator));
        await tester.pump(const Duration(seconds: 1));
        final items = app.messages.view.value.items;
        app.showPage(WindowPage.messages);
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        app.messages.requestOpen((messageId: null, push: items.first));
        await tester.pump();
        await tester.pump(const Duration(seconds: 1));
        final route = observer.pushes.last as PageRoute;
        final before = observer.pushes.length;
        for (final item in [items.first, items.last]) {
          app.showPage(WindowPage.messages, openingMessage: true);
          app.messages.requestOpen((messageId: null, push: item));
          for (var frame = 0; frame < 24; frame++) {
            await tester.pump(const Duration(milliseconds: 16));
            expect(observer.pushes.length, before);
            expect(observer.pops, isEmpty);
            expect(route.isCurrent, true);
            expect(route.animation!.status, AnimationStatus.completed);
            expect(find.text(item.content), findsOneWidget);
          }
        }
        await tester.pumpWidget(const SizedBox());
        debugDefaultTargetPlatformOverride = null;
      },
    );
  }
}
