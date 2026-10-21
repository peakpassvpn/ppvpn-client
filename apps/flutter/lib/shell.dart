// The window's frame (design Main / DesktopNodes, navigation as decided on
// #387): the 200-wide sidebar with the traffic lights' row, the brand, the
// nav items (34 high) for overview, nodes and logs, and the account card
// with the settings gear at the bottom. Content: a 52-high header with the
// page title, a status pill on the nodes page, the bell with the unread
// badge, and on Windows and Linux the caption buttons. The window has no
// system title bar. The bell opens the message center and the gear the
// settings, both as pages (#401 step 3). Signed out, the sign-in brand
// panel takes the sidebar's place.

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';
import 'package:window_manager/window_manager.dart';

import 'app_state.dart';
import 'pages/logs.dart';
import 'pages/messages.dart';
import 'pages/nodes.dart';
import 'pages/overview.dart';
import 'pages/settings.dart';
import 'pages/sign_in.dart';
import 'sample.dart';
import 'src/rust/api.dart' as rust;
import 'strings.dart';
import 'theme.dart';
import 'widgets.dart';

enum _Page { overview, nodes, logs, messages, settings }

class Shell extends StatefulWidget {
  const Shell({super.key, required this.app});

  final AppState app;

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  _Page _page = switch (samplePage) {
    'nodes' => _Page.nodes,
    'logs' => _Page.logs,
    'messages' => _Page.messages,
    'settings' => _Page.settings,
    _ => _Page.overview,
  };

  @override
  void initState() {
    super.initState();
    widget.app.enhancedRequests.addListener(_turnOnEnhanced);
  }

  @override
  void dispose() {
    widget.app.enhancedRequests.removeListener(_turnOnEnhanced);
    super.dispose();
  }

  /// The tray asked for enhanced mode before the service is installed: the
  /// install explanation needs the window.
  void _turnOnEnhanced() => unawaited(turnOnEnhanced(context, widget.app));

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.app.snapshot,
    builder: (context, snapshot, _) {
      final t = Tokens.of(context);
      if (snapshot.auth != rust.UiAuth.signedIn) {
        return Row(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const Stack(
              children: [
                SignInBrand(),
                Positioned(left: 0, right: 0, top: 0, height: Tokens.topBarHeight, child: _Drag()),
              ],
            ),
            Expanded(
              child: ColoredBox(
                color: t.card,
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [
                    const _Header(),
                    Expanded(child: SignInPage(app: widget.app)),
                  ],
                ),
              ),
            ),
          ],
        );
      }
      final s = S.current;
      final title = switch (_page) {
        _Page.overview => s('overview'),
        _Page.nodes => s('nodes'),
        _Page.logs => s('logs'),
        _Page.messages => s('msgCenter'),
        _Page.settings => s('settings'),
      };
      return Row(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _Sidebar(
            app: widget.app,
            snapshot: snapshot,
            page: _page,
            onPage: (page) => setState(() => _page = page),
          ),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                _Header(
                  title: title,
                  status: _page == _Page.nodes ? _StatusPill(app: widget.app, snapshot: snapshot) : null,
                  bell: _Bell(
                    unread: snapshot.unread,
                    selected: _page == _Page.messages,
                    onPress: () => setState(() => _page = _Page.messages),
                  ),
                ),
                Expanded(
                  child: switch (_page) {
                    _Page.overview => OverviewPage(app: widget.app),
                    _Page.nodes => NodesPage(app: widget.app),
                    _Page.logs => const LogsPage(),
                    _Page.messages => MessagesPage(app: widget.app),
                    _Page.settings => SettingsPage(app: widget.app),
                  },
                ),
              ],
            ),
          ),
        ],
      );
    },
  );
}

class _Drag extends StatelessWidget {
  const _Drag();

  @override
  Widget build(BuildContext context) => const DragToMoveArea(child: SizedBox.expand());
}

/// The content header (52): title, status pill, bell; the caption buttons
/// at its end on Windows and Linux. Empty space drags the window.
class _Header extends StatelessWidget {
  const _Header({this.title, this.status, this.bell});

  final String? title;
  final Widget? status;
  final Widget? bell;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final caps = !Platform.isMacOS;
    return SizedBox(
      height: Tokens.topBarHeight,
      child: Stack(
        children: [
          const Positioned.fill(child: _Drag()),
          Padding(
            padding: EdgeInsets.only(left: Tokens.pageInset, right: caps ? 0 : Tokens.pageInset),
            child: Row(
              children: [
                Expanded(
                  child: title == null
                      ? const SizedBox()
                      : Semantics(header: true, child: Text(title!, style: t.pageTitle)),
                ),
                if (status != null) ...[status!, const SizedBox(width: 12)],
                ?bell,
                if (caps) ...[const SizedBox(width: 8), const _CaptionButtons()],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// 「已连接 · 节点」 on the nodes page's header (24 high, ok on ok-soft).
class _StatusPill extends StatelessWidget {
  const _StatusPill({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final tone = statusColor(t, snapshot);
    final node = app.nodeById(snapshot.selectedNodeId);
    final text = node == null ? statusTitle(snapshot) : '${statusTitle(snapshot)} · ${nodeTitle(node)}';
    return Container(
      height: 24,
      constraints: const BoxConstraints(maxWidth: 260),
      padding: const EdgeInsets.fromLTRB(8, 0, 10, 0),
      decoration: BoxDecoration(color: tone.withValues(alpha: 0.10), borderRadius: BorderRadius.circular(12)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          Container(
            width: 7,
            height: 7,
            decoration: BoxDecoration(
              color: tone,
              shape: BoxShape.circle,
              boxShadow: [BoxShadow(color: tone.withValues(alpha: 0.18), spreadRadius: 3)],
            ),
          ),
          const SizedBox(width: 7),
          Flexible(
            child: Text(
              text,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: t.note.copyWith(fontWeight: FontWeight.w500, color: tone),
            ),
          ),
        ],
      ),
    );
  }
}

/// Messages: a 32 icon button with the unread badge on its top right (16
/// high, ringed in the background colour; none at 0, 「99+」 beyond 99).
class _Bell extends StatelessWidget {
  const _Bell({required this.unread, required this.selected, required this.onPress});

  final int unread;
  final bool selected;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return Pressable(
      onPress: onPress,
      selected: selected,
      label: unread > 0 ? '${s('messages')}, ${s('unreadN', {'n': '$unread'})}' : s('messages'),
      builder: (context, hovered) => SizedBox(
        width: 32,
        height: 32,
        child: Stack(
          clipBehavior: Clip.none,
          children: [
            Positioned.fill(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  color: selected || hovered ? t.accentSoft : null,
                  borderRadius: BorderRadius.circular(Tokens.controlRadius),
                ),
                child: Icon(FLucideIcons.bell, size: 18, color: t.t2),
              ),
            ),
            if (unread > 0)
              Positioned(
                top: 3,
                right: 2,
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

class _CaptionButtons extends StatefulWidget {
  const _CaptionButtons();

  @override
  State<_CaptionButtons> createState() => _CaptionButtonsState();
}

class _CaptionButtonsState extends State<_CaptionButtons> with WindowListener {
  bool _maximized = false;

  @override
  void initState() {
    super.initState();
    windowManager.addListener(this);
    unawaited(windowManager.isMaximized().then((value) => mounted ? setState(() => _maximized = value) : null));
  }

  @override
  void dispose() {
    windowManager.removeListener(this);
    super.dispose();
  }

  @override
  void onWindowMaximize() => setState(() => _maximized = true);

  @override
  void onWindowUnmaximize() => setState(() => _maximized = false);

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    Widget button(IconData icon, String label, VoidCallback onPress, {bool close = false}) => Pressable(
      onPress: onPress,
      label: label,
      radius: 0,
      builder: (context, hovered) => Container(
        width: Tokens.captionButtonWidth,
        height: Tokens.topBarHeight,
        color: hovered ? (close ? const Color(0xFFC42B1C) : t.hair) : null,
        child: Icon(icon, size: 14, color: hovered && close ? const Color(0xFFFFFFFF) : t.t2),
      ),
    );
    return Row(
      mainAxisSize: MainAxisSize.min,
      children: [
        button(FLucideIcons.minus, s('minimize'), () => unawaited(windowManager.minimize())),
        button(
          _maximized ? FLucideIcons.copy : FLucideIcons.square,
          s('maximize'),
          () => unawaited(_maximized ? windowManager.unmaximize() : windowManager.maximize()),
        ),
        // Close hides the window, as the system button did (setPreventClose).
        button(FLucideIcons.x, s('close'), () => unawaited(windowManager.close()), close: true),
      ],
    );
  }
}

class _Sidebar extends StatelessWidget {
  const _Sidebar({required this.app, required this.snapshot, required this.page, required this.onPage});

  final AppState app;
  final rust.UiSnapshot snapshot;
  final _Page page;
  final ValueChanged<_Page> onPage;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return Container(
      width: Tokens.sidebarWidth,
      padding: Tokens.sidebarPadding,
      decoration: BoxDecoration(
        color: t.sidebar,
        border: Border(right: BorderSide(color: t.hair)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // The traffic lights' row (macOS draws them); drags the window.
          const SizedBox(height: Tokens.sidebarLightsHeight, child: _Drag()),
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 6, 8, 16),
            child: Row(
              children: [
                const BrandMark(size: 24),
                const SizedBox(width: 9),
                Text(s('app'), style: t.bodyStrong.copyWith(fontSize: 15, fontWeight: FontWeight.w700)),
              ],
            ),
          ),
          _NavItem(
            key: const ValueKey('nav-overview'),
            icon: (color, selected) => ShieldCheckIcon(size: 17, color: color, filled: selected),
            label: s('overview'),
            selected: page == _Page.overview,
            onPress: () => onPage(_Page.overview),
          ),
          const SizedBox(height: Tokens.navGap),
          _NavItem(
            key: const ValueKey('nav-nodes'),
            icon: (color, _) => Icon(FLucideIcons.globe, size: 17, color: color),
            label: s('nodes'),
            selected: page == _Page.nodes,
            onPress: () => onPage(_Page.nodes),
          ),
          const SizedBox(height: Tokens.navGap),
          _NavItem(
            key: const ValueKey('nav-logs'),
            icon: (color, _) => Icon(FLucideIcons.list, size: 17, color: color),
            label: s('logs'),
            selected: page == _Page.logs,
            onPress: () => onPage(_Page.logs),
          ),
          const Spacer(),
          _AccountCard(app: app, snapshot: snapshot, onSettings: () => onPage(_Page.settings)),
        ],
      ),
    );
  }
}

class _NavItem extends StatelessWidget {
  const _NavItem({super.key, required this.icon, required this.label, required this.selected, required this.onPress});

  final Widget Function(Color color, bool selected) icon;
  final String label;
  final bool selected;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final color = selected ? t.accent : t.t2;
    return Pressable(
      onPress: onPress,
      selected: selected,
      label: label,
      builder: (context, hovered) => Container(
        height: Tokens.navItemHeight,
        padding: const EdgeInsets.symmetric(horizontal: 10),
        decoration: BoxDecoration(
          color: selected ? t.navSelected : (hovered ? t.hair : null),
          borderRadius: BorderRadius.circular(Tokens.controlRadius),
          border: selected ? Border.all(color: t.hair) : null,
          boxShadow: selected ? t.navSelectedShadow : null,
        ),
        child: Row(
          children: [
            icon(color, selected),
            const SizedBox(width: 10),
            Text(label, style: t.control.copyWith(color: color, fontWeight: selected ? FontWeight.w600 : FontWeight.w500)),
          ],
        ),
      ),
    );
  }
}

/// Avatar (30), team name and plan, the settings gear; the card opens the
/// account menu (sign out).
class _AccountCard extends StatelessWidget {
  const _AccountCard({required this.app, required this.snapshot, required this.onSettings});

  final AppState app;
  final rust.UiSnapshot snapshot;
  final VoidCallback onSettings;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final name = snapshot.accountName ?? '';
    final initial = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    return Container(
      padding: const EdgeInsets.all(8),
      decoration: BoxDecoration(
        color: t.accountCard,
        borderRadius: BorderRadius.circular(10),
        border: Border.all(color: t.hair),
      ),
      child: Row(
        children: [
          Expanded(
            child: FPopoverMenu(
              menuAnchor: Alignment.bottomLeft,
              childAnchor: Alignment.topLeft,
              menu: [
                FItemGroup(
                  children: [FItem(title: Text(S.current('signOut')), onPress: app.signOut)],
                ),
              ],
              builder: (context, controller, _) => Pressable(
                key: const ValueKey('account'),
                onPress: controller.toggle,
                label: S.current('account'),
                builder: (context, hovered) => Row(
                  children: [
                    Container(
                      width: 30,
                      height: 30,
                      alignment: Alignment.center,
                      decoration: BoxDecoration(
                        shape: BoxShape.circle,
                        gradient: LinearGradient(
                          begin: Alignment.topLeft,
                          end: Alignment.bottomRight,
                          colors: t.avatar,
                        ),
                      ),
                      child: Text(initial, style: t.label.copyWith(fontSize: 13, fontWeight: FontWeight.w700, color: t.onAvatar)),
                    ),
                    const SizedBox(width: 10),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Text(
                            snapshot.teamName ?? name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: t.sectionTitle.copyWith(height: 17 / 13),
                          ),
                          Text(
                            name,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: t.note.copyWith(fontSize: 11, height: 14 / 11),
                          ),
                        ],
                      ),
                    ),
                  ],
                ),
              ),
            ),
          ),
          Button(kind: ButtonKind.icon, icon: FLucideIcons.settings, height: 28, semanticsLabel: S.current('settings'), onPress: onSettings),
        ],
      ),
    );
  }
}
