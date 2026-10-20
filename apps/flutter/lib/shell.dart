// The window's frame (#387, navigation as decided on the issue): a left
// sidebar, 200 wide, with 34-high items for overview, nodes and logs, and
// the account card plus settings at the bottom. Messages are not in the
// sidebar: a bell with the unread badge at the top right of the content
// area opens them. The window has no system title bar: on macOS the traffic
// lights sit at the top of the sidebar; on Windows and Linux the caption
// buttons are drawn at the top right of the content area, right of the
// bell. Logs, messages and settings are not in the prototype: they lead to
// a placeholder.
// Signed out, the sign-in page fills the window.

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';
import 'package:window_manager/window_manager.dart';

import 'app_state.dart';
import 'pages/nodes.dart';
import 'pages/overview.dart';
import 'pages/sign_in.dart';
import 'src/rust/api.dart' as rust;
import 'strings.dart';
import 'theme.dart';

enum _Page { overview, nodes, logs, messages, settings }

class Shell extends StatefulWidget {
  const Shell({super.key, required this.app});

  final AppState app;

  @override
  State<Shell> createState() => _ShellState();
}

class _ShellState extends State<Shell> {
  _Page _page = _Page.overview;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.app.snapshot,
    builder: (context, snapshot, _) {
      if (snapshot.auth != rust.UiAuth.signedIn) {
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            const _TopBar(),
            Expanded(child: SignInPage(app: widget.app)),
          ],
        );
      }
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
                _TopBar(
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
                    _ => const _NotInPrototype(),
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

/// The draggable strip above the content; at its end the bell (signed in)
/// and, on Windows and Linux, the caption buttons.
class _TopBar extends StatelessWidget {
  const _TopBar({this.bell});

  final Widget? bell;

  @override
  Widget build(BuildContext context) => SizedBox(
    height: Tokens.topBarHeight,
    child: Row(
      children: [
        const Expanded(child: DragToMoveArea(child: SizedBox.expand())),
        if (bell case final bell?) ...[bell, SizedBox(width: Platform.isMacOS ? 16 : 8)],
        if (!Platform.isMacOS) const _CaptionButtons(),
      ],
    ),
  );
}

/// Messages: the bell, the unread badge on its top right corner (none at 0).
class _Bell extends StatelessWidget {
  const _Bell({required this.unread, required this.selected, required this.onPress});

  final int unread;
  final bool selected;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return SizedBox(
      width: 34,
      height: 30,
      child: Stack(
        clipBehavior: Clip.none,
        children: [
          Positioned.fill(
            child: FButton.icon(
              variant: selected ? FButtonVariant.secondary : FButtonVariant.ghost,
              semanticsLabel: unread > 0 ? '${s('messages')}, ${s('unreadN', {'n': '$unread'})}' : s('messages'),
              onPress: onPress,
              child: Icon(FLucideIcons.bell, size: 16, color: t.textSecondary),
            ),
          ),
          if (unread > 0)
            Positioned(
              top: -3,
              right: -6,
              child: ExcludeSemantics(
                child: DecoratedBox(
                  // The background-coloured ring around the badge.
                  decoration: BoxDecoration(
                    color: t.window,
                    borderRadius: BorderRadius.circular(11),
                  ),
                  child: Padding(padding: const EdgeInsets.all(1.5), child: Badge(count: unread)),
                ),
              ),
            ),
        ],
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
    final s = S.current;
    Widget button(IconData icon, String label, VoidCallback onPress) => SizedBox(
      width: Tokens.captionButtonWidth,
      height: Tokens.topBarHeight,
      child: FButton.icon(
        variant: FButtonVariant.ghost,
        semanticsLabel: label,
        onPress: onPress,
        child: Icon(icon, size: 14),
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
        button(FLucideIcons.x, s('close'), () => unawaited(windowManager.close())),
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
      decoration: BoxDecoration(
        color: t.sidebar,
        border: Border(right: BorderSide(color: t.border, width: 0.5)),
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          // Room for the macOS traffic lights; a drag area everywhere.
          const SizedBox(height: Tokens.topBarHeight, child: DragToMoveArea(child: SizedBox.expand())),
          _Item(
            icon: FLucideIcons.house,
            label: s('overview'),
            selected: page == _Page.overview,
            onPress: () => onPage(_Page.overview),
          ),
          _Item(
            icon: FLucideIcons.globe,
            label: s('nodes'),
            selected: page == _Page.nodes,
            onPress: () => onPage(_Page.nodes),
          ),
          _Item(
            icon: FLucideIcons.fileText,
            label: s('logs'),
            selected: page == _Page.logs,
            onPress: () => onPage(_Page.logs),
          ),
          const Spacer(),
          Padding(
            padding: const EdgeInsets.fromLTRB(8, 0, 8, 10),
            child: Row(
              children: [
                Expanded(child: _AccountCard(app: app, snapshot: snapshot)),
                const SizedBox(width: 2),
                SizedBox(
                  width: Tokens.sidebarItemHeight,
                  height: Tokens.sidebarItemHeight,
                  child: FButton.icon(
                    variant: page == _Page.settings ? FButtonVariant.secondary : FButtonVariant.ghost,
                    semanticsLabel: s('settings'),
                    onPress: () => onPage(_Page.settings),
                    child: const Icon(FLucideIcons.settings, size: 15),
                  ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Item extends StatelessWidget {
  const _Item({required this.icon, required this.label, required this.selected, required this.onPress});

  final IconData icon;
  final String label;
  final bool selected;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Padding(
      padding: const EdgeInsets.symmetric(horizontal: 8, vertical: 1),
      child: SizedBox(
        height: Tokens.sidebarItemHeight,
        child: FButton(
          variant: selected ? FButtonVariant.secondary : FButtonVariant.ghost,
          selected: selected,
          mainAxisAlignment: MainAxisAlignment.start,
          onPress: onPress,
          semanticsLabel: label,
          prefix: Icon(icon, size: 15, color: selected ? t.accent : t.textSecondary),
          child: Align(
            alignment: Alignment.centerLeft,
            child: Text(label, style: t.rowTitle.copyWith(fontWeight: selected ? FontWeight.w600 : null)),
          ),
        ),
      ),
    );
  }
}

/// Unread count (handoff "角标"): none at 0, the number up to 99, "99+"
/// beyond; danger colour, bold tabular digits.
class Badge extends StatelessWidget {
  const Badge({super.key, required this.count});

  final int count;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      constraints: const BoxConstraints(minWidth: 18),
      height: 18,
      padding: const EdgeInsets.symmetric(horizontal: 5),
      alignment: Alignment.center,
      decoration: BoxDecoration(color: t.badge, borderRadius: BorderRadius.circular(9)),
      child: Text(
        count > 99 ? '99+' : '$count',
        style: TextStyle(
          fontSize: 11,
          fontWeight: FontWeight.w700,
          fontFeatures: const [FontFeature.tabularFigures()],
          color: t.onBadge,
        ),
      ),
    );
  }
}

/// Avatar initial, account name and team; the menu holds sign-out.
class _AccountCard extends StatelessWidget {
  const _AccountCard({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final name = snapshot.accountName ?? '';
    final initial = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    return FPopoverMenu(
      menuAnchor: Alignment.bottomLeft,
      childAnchor: Alignment.topLeft,
      menu: [
        FItemGroup(
          children: [
            FItem(title: Text(S.current('signOut')), onPress: app.signOut),
          ],
        ),
      ],
      builder: (context, controller, _) => SizedBox(
        height: 40,
        child: FButton(
          variant: FButtonVariant.ghost,
          mainAxisAlignment: MainAxisAlignment.start,
          onPress: controller.toggle,
          semanticsLabel: S.current('account'),
          prefix: Container(
            width: 24,
            height: 24,
            alignment: Alignment.center,
            decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle),
            child: Text(initial, style: TextStyle(fontSize: 11, fontWeight: FontWeight.w700, color: t.onAccent)),
          ),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(name, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.rowTitle),
              if (snapshot.teamName case final team?)
                Text(team, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.note),
            ],
          ),
        ),
      ),
    );
  }
}

class _NotInPrototype extends StatelessWidget {
  const _NotInPrototype();

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(child: Text(S.current('protoPage'), style: t.secondary));
  }
}
