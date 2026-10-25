// The phone layout (#448 step 5), below Tokens.phoneBreakpoint wide: three
// tabs at the bottom (概览 · 节点 · 我的) on a translucent bar, each page
// under a top bar with its large title, the bell on 概览 and 我的. Messages,
// logs and choices are pushed full screen with a back control. Signed out:
// the phone sign-in.

import 'dart:async';
import 'dart:ui' show ImageFilter;

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../pages/messages.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'me.dart';
import 'nodes.dart';
import 'overview.dart';
import 'sign_in.dart';

enum _Tab { overview, nodes, me }

class PhoneShell extends StatefulWidget {
  const PhoneShell({super.key, required this.app});

  final AppState app;

  @override
  State<PhoneShell> createState() => _PhoneShellState();
}

class _PhoneShellState extends State<PhoneShell> {
  _Tab _tab = _Tab.overview;

  @override
  void initState() {
    super.initState();
    widget.app.pageRequests.addListener(_showRequested);
  }

  @override
  void dispose() {
    widget.app.pageRequests.removeListener(_showRequested);
    super.dispose();
  }

  /// A notification click (the messages) or a settings request (我的).
  /// Signed out the sign-in stays.
  void _showRequested() {
    final request = widget.app.pageRequests.value;
    if (request == null || widget.app.snapshot.value.auth != rust.UiAuth.signedIn) return;
    switch (request.page) {
      case WindowPage.messages:
        openMessages(context, widget.app);
      case WindowPage.settings:
        setState(() => _tab = _Tab.me);
    }
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.app.snapshot,
    builder: (context, snapshot, _) {
      final app = widget.app;
      if (snapshot.auth != rust.UiAuth.signedIn) return PhoneSignIn(app: app);
      final s = S.current;
      final bell = PhoneBell(unread: snapshot.unread, onPress: () => openMessages(context, app));
      final page = switch (_tab) {
        _Tab.overview => PhoneScaffold(
          title: s('overview'),
          trailing: bell,
          child: PhoneOverview(app: app, onNodes: () => setState(() => _tab = _Tab.nodes)),
        ),
        _Tab.nodes => PhoneNodes(app: app),
        _Tab.me => PhoneScaffold(title: s('me'), trailing: bell, child: PhoneMe(app: app)),
      };
      final media = MediaQuery.of(context);
      return Stack(
        children: [
          // The pages scroll under the translucent tab bar: their bottom
          // padding includes it.
          Positioned.fill(
            child: MediaQuery(
              data: media.copyWith(padding: media.padding.copyWith(bottom: media.padding.bottom + Tokens.phoneTabBar)),
              child: KeyedSubtree(key: ValueKey(_tab), child: page),
            ),
          ),
          Positioned(
            left: 0,
            right: 0,
            bottom: 0,
            child: _TabBar(tab: _tab, onTab: (tab) => setState(() => _tab = tab)),
          ),
        ],
      );
    },
  );
}

/// The tab bar: 50 plus the safe area, the card colour at 82 % over a blur,
/// a 0.5 px hair on top. The selected tab's icon sits on an accent-soft
/// pill in the accent colour (Lucide has no filled icons).
class _TabBar extends StatelessWidget {
  const _TabBar({required this.tab, required this.onTab});

  final _Tab tab;
  final ValueChanged<_Tab> onTab;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final bottom = MediaQuery.paddingOf(context).bottom;
    Widget item(_Tab value, IconData icon, String label) {
      final on = value == tab;
      final color = on ? t.accent : t.t3;
      return Expanded(
        child: Pressable(
          key: ValueKey('tab-${value.name}'),
          onPress: () => onTab(value),
          selected: on,
          label: label,
          radius: 0,
          builder: (context, _) => Column(
            mainAxisAlignment: MainAxisAlignment.center,
            children: [
              AnimatedContainer(
                duration: Tokens.motion(context, Tokens.motionFast),
                curve: Tokens.motionFastCurve,
                width: 48,
                height: 26,
                decoration: BoxDecoration(
                  color: on ? t.accentSoft : t.accentSoft.withValues(alpha: 0),
                  borderRadius: BorderRadius.circular(13),
                ),
                child: Icon(icon, size: 20, color: color),
              ),
              const SizedBox(height: 2),
              Text(
                label,
                style: t.label.copyWith(fontSize: 10, color: color, fontWeight: on ? FontWeight.w600 : FontWeight.w500),
              ),
            ],
          ),
        ),
      );
    }

    return ClipRect(
      child: BackdropFilter(
        filter: ImageFilter.blur(sigmaX: 20, sigmaY: 20),
        child: Container(
          height: Tokens.phoneTabBar + bottom,
          padding: EdgeInsets.only(bottom: bottom),
          decoration: BoxDecoration(
            color: t.card.withValues(alpha: 0.82),
            border: Border(top: BorderSide(color: t.hair, width: 0.5)),
          ),
          child: Row(
            children: [
              item(_Tab.overview, FLucideIcons.shield, s('overview')),
              item(_Tab.nodes, FLucideIcons.globe, s('nodes')),
              item(_Tab.me, FLucideIcons.user, s('me')),
            ],
          ),
        ),
      ),
    );
  }
}

/// A page under the top bar (56 plus the safe area): the large title on the
/// left, [trailing] on the right (the bell, the nodes page's tools); with
/// [back] a pushed page's back control and a smaller title instead.
class PhoneScaffold extends StatelessWidget {
  const PhoneScaffold({super.key, required this.title, required this.child, this.trailing, this.back = false});

  final String title;
  final Widget child;
  final Widget? trailing;
  final bool back;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final top = MediaQuery.paddingOf(context).top;
    return ColoredBox(
      color: t.bg,
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Container(
            height: Tokens.phoneTopBar + top,
            padding: EdgeInsets.fromLTRB(back ? 4 : Tokens.phonePadding, top, Tokens.phonePadding - 6, 0),
            child: Row(
              children: [
                if (back) const PhoneBack(),
                Expanded(
                  child: Semantics(
                    header: true,
                    child: Text(
                      title,
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: back ? t.phoneBodyStrong.copyWith(fontSize: 17) : t.phoneTitle,
                    ),
                  ),
                ),
                ?trailing,
              ],
            ),
          ),
          Expanded(child: MediaQuery.removePadding(context: context, removeTop: true, child: child)),
        ],
      ),
    );
  }
}

/// The scroll padding of a phone page: the page padding, and at the bottom
/// the safe area (with the tab bar, PhoneShell) as well.
EdgeInsets phoneScrollPadding(BuildContext context) => EdgeInsets.fromLTRB(
  Tokens.phonePadding,
  4,
  Tokens.phonePadding,
  Tokens.phonePadding + MediaQuery.paddingOf(context).bottom,
);

/// A pushed page's back control (44, the chevron in the accent colour).
class PhoneBack extends StatelessWidget {
  const PhoneBack({super.key});

  @override
  Widget build(BuildContext context) => Pressable(
    key: const ValueKey('back'),
    onPress: () => unawaited(Navigator.of(context).maybePop()),
    label: S.current('back'),
    builder: (context, _) => SizedBox(
      width: Tokens.phoneControl,
      height: Tokens.phoneControl,
      child: Icon(FLucideIcons.chevronLeft, size: 24, color: Tokens.of(context).accent),
    ),
  );
}

/// Messages: a 44 icon button with the unread badge on its top right (none
/// at 0, 「99+」 beyond 99), as the desktop's bell.
class PhoneBell extends StatelessWidget {
  const PhoneBell({super.key, required this.unread, required this.onPress});

  final int unread;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return Pressable(
      key: const ValueKey('bell'),
      onPress: onPress,
      label: unread > 0 ? '${s('messages')}, ${s('unreadN', {'n': '$unread'})}' : s('messages'),
      builder: (context, _) => SizedBox(
        width: Tokens.phoneControl,
        height: Tokens.phoneControl,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Center(child: Icon(FLucideIcons.bell, size: 22, color: t.t2)),
            if (unread > 0)
              Positioned(
                top: 6,
                right: 4,
                child: Container(
                  constraints: const BoxConstraints(minWidth: 16),
                  height: 16,
                  padding: const EdgeInsets.symmetric(horizontal: 4),
                  alignment: Alignment.center,
                  decoration: BoxDecoration(
                    color: t.bad,
                    borderRadius: BorderRadius.circular(8),
                    boxShadow: [BoxShadow(color: t.bg, spreadRadius: 2)],
                  ),
                  child: Text(
                    unread > 99 ? '99+' : '$unread',
                    style: t.label.copyWith(fontSize: 10, height: 1.6, fontWeight: FontWeight.w700, color: t.onBad),
                  ),
                ),
              ),
          ],
        ),
      ),
    );
  }
}

/// Pushes [page] full screen, sliding in from the end over
/// [Tokens.motionPage]. Routes sit beside the app's home, so the page gets
/// forui's theme and the body text style here.
Future<T?> pushPhonePage<T>(BuildContext context, Widget page) => Navigator.of(context).push<T>(
  PageRouteBuilder<T>(
    transitionDuration: Tokens.motion(context, Tokens.motionPage),
    reverseTransitionDuration: Tokens.motion(context, Tokens.motionFast),
    pageBuilder: (context, _, _) {
      final t = Tokens.of(context);
      return FTheme(
        data: t.forui,
        child: DefaultTextStyle(style: t.body, child: page),
      );
    },
    transitionsBuilder: (context, animation, _, child) => SlideTransition(
      position: animation.drive(
        Tween(begin: const Offset(1, 0), end: Offset.zero).chain(CurveTween(curve: Tokens.motionCurve)),
      ),
      child: child,
    ),
  ),
);

/// The message center, pushed.
void openMessages(BuildContext context, AppState app) => unawaited(
  pushPhonePage<void>(
    context,
    PhoneScaffold(title: S.current('msgCenter'), back: true, child: MessagesPage(app: app)),
  ),
);
