// The menu bar / tray icon and its menu (design "桌面 · 托盘 / 菜单栏",
// #401 step 3): a native menu on all three platforms, on tray_manager's
// engine (nativeapi); Flutter only gives it the data. The menu is rebuilt
// only when what it shows changes (the state, the node list and its speed
// tests, the unread count), never with traffic: frequent tray updates are
// what made other Flutter clients burn CPU (#387).
//
// - Restoring the sign-in: the app items only.
// - Signed out: 「未登录」, then the app items.
// - Access limited: the status dot and the limited title (as on the
//   overview), 「打开 PPVPN 查看详情」, the unread row, the app items; no
//   switches, no nodes.
// - Signed in: the status dot and title, then 「mode · node · latency」;
//   system proxy and enhanced mode as check items with the overview's
//   switch state (in progress: both wait, the busy one stays checked;
//   failed: unchecked, a click retries); 「当前节点：X」 with the nodes in the
//   nodes page's order, latency on the right, the current one checked; the
//   unread row, which opens the messages page.
// - App items: open PPVPN; settings (macOS 「设置…」 ⌘,, Windows 「设置」,
//   Linux 「首选项」); check for updates only where the app updates itself
//   (`onCheckForUpdates`); quit (⌘Q on macOS).
//
// macOS: a click opens the menu; a template icon and no title; in progress
// it alternates with the off icon every 0.5 s, and stays solid with reduced
// motion. Windows and Linux: a click opens the window, a right click the
// menu; colour icons, no animation, the tooltip 「PPVPN · status · node」.
// Native menus take no flags and no colours: a node is its name, the dots
// are coloured emoji circles.

import 'dart:async';
import 'dart:io';
import 'dart:ui' show PlatformDispatcher;

import 'package:nativeapi/nativeapi.dart' show ModifierKey, Size;
import 'package:tray_manager/tray_manager.dart';

import 'app_state.dart';
import 'bench.dart';
import 'pages/overview.dart';
import 'settings.dart';
import 'src/rust/api.dart' as rust;
import 'strings.dart';
import 'widgets.dart' show SwitchState, nodeTitle;

/// The icon's four states: not connected, in progress, connected, error.
enum _Icon { off, busy, on, error }

class Tray {
  Tray({required this.app, required this.onOpen, required this.onQuit, this.onCheckForUpdates}) {
    _icon = TrayIcon.create();
    final icon = _icon;
    if (icon == null) return;
    icon.setContextMenuTrigger(Platform.isMacOS ? ContextMenuTrigger.clicked : ContextMenuTrigger.rightClicked);
    // The first listener also starts the icon's click handling, which the
    // trigger needs on macOS.
    icon.addListener((event) {
      if (Platform.isMacOS) {
        if (event is TrayIconRightClickedEvent) icon.openContextMenu();
      } else if (event is TrayIconClickedEvent || event is TrayIconDoubleClickedEvent) {
        // GNOME's AppIndicator opens the menu on a click itself and reports
        // a double click only.
        onOpen();
      }
    });
    app.snapshot.addListener(_update);
    app.nodes.addListener(_update);
    // A new language: rebuild the menu whatever else changed.
    Settings.language.addListener(() {
      _shown = null;
      _update();
    });
    app.latency.addListener(_update);
    _update();
    icon.setVisible(true);
    if (trayBench) {
      // compare.py: a rebuild every second, whatever changed.
      Timer.periodic(const Duration(seconds: 1), (_) {
        _shown = null;
        _iconShown = null;
        _benchTick++;
        _update();
      });
    }
  }

  int _benchTick = 0;

  final AppState app;
  final void Function() onOpen;
  final Future<void> Function() onQuit;

  /// Shows 「检查更新…」; null where the app does not update itself (in-app
  /// update is #401 step 5).
  final void Function()? onCheckForUpdates;

  TrayIcon? _icon;
  String? _shown;
  _Icon? _iconShown;
  Timer? _breath;
  bool _dim = false;
  final Map<String, Image?> _images = {};

  void _update() {
    final icon = _icon;
    if (icon == null) return;
    final s = S.current;
    final snapshot = app.snapshot.value;
    final signedIn = snapshot.auth == rust.UiAuth.signedIn;
    final limited = signedIn ? LimitedAccess.of(snapshot)?.title : null;
    final node = app.nodeById(snapshot.selectedNodeId);
    _showIcon(
      !signedIn || limited != null
          ? _Icon.off
          : switch (snapshot.phase) {
              rust.UiPhase.connected => _Icon.on,
              rust.UiPhase.error || rust.UiPhase.contended => _Icon.error,
              final phase when isBusy(phase) => _Icon.busy,
              _ => _Icon.off,
            },
    );
    final status = switch (snapshot.auth) {
      rust.UiAuth.restoring => null,
      rust.UiAuth.signedIn => limited ?? statusTitle(snapshot),
      _ => s('notSignedIn'),
    };
    final ready = signedIn && limited == null;
    final detail = [
      if (snapshot.phase != rust.UiPhase.off)
        s(snapshot.connectionMode == rust.UiConnectionMode.enhanced ? 'tunMode' : 'stdMode'),
      node == null ? '—' : nodeTitle(node),
      if (snapshot.latencyMs case final ms?) '$ms ms' else '—',
    ].join(' · ');
    final latency = app.latency.value;
    final nodes = [for (final each in app.nodes.value) (node: each, latency: latency[each.id])];
    // Rebuild only when something shown changes.
    final key = [
      status,
      if (ready) ...[
        detail,
        snapshot.phase,
        snapshot.connectionMode,
        snapshot.selectedNodeId,
        for (final row in nodes) '${row.node.id}:${row.node.name}:${_latencyText(row.latency)}',
      ],
      if (signedIn) snapshot.unread,
    ].join('|');
    if (key == _shown) return;
    _shown = key;

    if (!Platform.isMacOS || trayBench) {
      icon.setTooltip(
        [
          'PPVPN',
          ?status,
          if (ready && node != null) nodeTitle(node),
          if (trayBench) '$_benchTick',
        ].join(' · '),
      );
    }

    final menu = Menu.create();
    if (menu == null) return;
    if (!signedIn && status != null) {
      menu.addItem(_label(status));
      menu.addSeparator();
    } else if (limited != null) {
      menu.addItem(_label('${_dot(null)} $limited'));
      menu.addItem(_item(s('openForDetails'), onOpen));
      menu.addSeparator();
      menu.addItem(_unread(snapshot.unread));
      menu.addSeparator();
    } else if (ready) {
      menu.addItem(_label('${_dot(snapshot.phase)} $status'));
      menu.addItem(_label(detail));
      menu.addSeparator();
      menu.addItem(_mode(snapshot, s('stdMode'), rust.UiConnectionMode.compatible));
      menu.addItem(_mode(snapshot, s('tunMode'), rust.UiConnectionMode.enhanced));
      final parent = MenuItem.createWithLabelAndType(
        '${s('currentNode')}: ${node == null ? '—' : nodeTitle(node)}',
        MenuItemType.submenu,
      );
      final submenu = Menu.create();
      if (parent != null && submenu != null) {
        for (final row in nodes) {
          final item = MenuItem.createWithLabelAndType(
            _withLatency(nodeTitle(row.node), _latencyText(row.latency)),
            MenuItemType.radio,
          );
          if (item == null) continue;
          final current = row.node.id == snapshot.selectedNodeId;
          item.state = current ? MenuItemState.checked : MenuItemState.unchecked;
          // Failed its latest speed test: greyed, unless it is the current one.
          item.isEnabled = current || !(row.latency is LatencyTimeout || row.latency is LatencyFailed);
          item.addListener((event) {
            if (event is MenuItemClickedEvent) unawaited(app.selectNode(row.node.id));
          });
          submenu.addItem(item);
        }
        parent.submenu = submenu;
        parent.isEnabled = nodes.isNotEmpty;
        menu.addItem(parent);
      }
      menu.addSeparator();
      menu.addItem(_unread(snapshot.unread));
      menu.addSeparator();
    }

    menu.addItem(_item(s('openMain'), onOpen));
    final settings = _item(
      Platform.isMacOS
          ? s('settingsMenuMac')
          : Platform.isWindows
          ? s('settingsMenuWin')
          : s('preferences'),
      () => _openPage(WindowPage.settings),
    );
    if (Platform.isMacOS) settings?.accelerator = const KeyboardAccelerator(modifiers: ModifierKey.meta, key: ',');
    menu.addItem(settings);
    if (onCheckForUpdates case final check?) menu.addItem(_item(s('checkUpdates'), check));
    menu.addSeparator();
    final quit = _item(s('quit'), onQuit);
    if (Platform.isMacOS) quit?.accelerator = const KeyboardAccelerator(modifiers: ModifierKey.meta, key: 'q');
    menu.addItem(quit);
    icon.setContextMenu(menu);
  }

  /// System proxy or enhanced mode: checked while the overview's switch is
  /// on or in between; neither can be clicked while one is on its way.
  MenuItem? _mode(rust.UiSnapshot snapshot, String label, rust.UiConnectionMode mode) {
    final item = MenuItem.createWithLabelAndType(label, MenuItemType.checkbox);
    if (item == null) return null;
    final checked = switchState(snapshot, mode) != SwitchState.off;
    item.state = checked ? MenuItemState.checked : MenuItemState.unchecked;
    item.isEnabled = !isBusy(snapshot.phase);
    item.addListener((event) {
      if (event is MenuItemClickedEvent) _toggle(mode, checked);
    });
    return item;
  }

  /// A check item's click, as the overview's switch and problem card do it.
  void _toggle(rust.UiConnectionMode mode, bool checked) {
    final snapshot = app.snapshot.value;
    if (checked) {
      unawaited(app.disconnect());
    } else if (mode == rust.UiConnectionMode.enhanced && !snapshot.serviceInstalled) {
      // The install explanation comes first, in the window's overview.
      onOpen();
      app.enhancedRequests.value++;
    } else if (snapshot.connectionMode == mode && snapshot.phase == rust.UiPhase.contended && snapshot.canTakeOver) {
      unawaited(app.takeOver());
    } else if (snapshot.connectionMode == mode &&
        (snapshot.phase == rust.UiPhase.error || snapshot.phase == rust.UiPhase.contended)) {
      unawaited(app.retry());
    } else {
      unawaited(app.connectIn(mode));
    }
  }

  /// 「N 条未读消息」 with the red dot, or 「没有未读消息」; opens the messages
  /// page.
  MenuItem? _unread(int unread) {
    final s = S.current;
    return _item(
      unread > 0 ? '🔴 ${s('unreadN', {'n': unread > 99 ? '99+' : '$unread'})}' : s('noUnread'),
      () => _openPage(WindowPage.messages),
    );
  }

  void _openPage(WindowPage page) {
    onOpen();
    app.showPage(page);
  }

  /// The icon for [state]; on macOS the in-progress one breathes.
  void _showIcon(_Icon state) {
    if (state == _iconShown) return;
    _iconShown = state;
    _breath?.cancel();
    _breath = null;
    _dim = false;
    _setImage(state);
    if (state != _Icon.busy || !Platform.isMacOS) return;
    _breath = Timer.periodic(const Duration(milliseconds: 500), (_) {
      // Reduced motion: the solid frame only.
      final dim = !_dim && !PlatformDispatcher.instance.accessibilityFeatures.disableAnimations;
      if (dim == _dim) return;
      _dim = dim;
      _setImage(dim ? _Icon.off : _Icon.busy);
    });
  }

  void _setImage(_Icon state) {
    final icon = _icon;
    if (icon == null) return;
    final ratio = PlatformDispatcher.instance.implicitView?.devicePixelRatio ?? 1;
    // The 1x files are 18 px on macOS, 32 px elsewhere (a 16 px tray icon
    // up to 200 %).
    final colour = switch (state) {
      _Icon.off => 'grey',
      _Icon.busy => 'blue',
      _Icon.on => 'green',
      _Icon.error => 'red',
    };
    final name = Platform.isMacOS
        ? 'menubar-${state.name}${ratio > 1 ? '@2x' : ''}'
        : 'tray-$colour${ratio > 2 ? '@2x' : ''}';
    final image = _images.putIfAbsent(name, () => ImageAsset.fromAsset('assets/tray/$name.png'));
    if (image == null) return;
    icon.icon = image;
    if (Platform.isMacOS) {
      // 18 pt whichever file it is: the @2x one stays sharp on Retina.
      icon.iconSize = const Size(width: 18, height: 18);
      icon.isIconTemplate = true;
    }
  }

  /// A line that cannot be clicked (status, the second header line).
  MenuItem? _label(String label) {
    final item = _item(label);
    item?.isEnabled = false;
    return item;
  }

  MenuItem? _item(String label, [void Function()? onClick]) {
    final item = MenuItem.createWithLabelAndType(label, MenuItemType.normal);
    if (item != null && onClick != null) {
      item.addListener((event) {
        if (event is MenuItemClickedEvent) onClick();
      });
    }
    return item;
  }

  void dispose() {
    app.snapshot.removeListener(_update);
    app.nodes.removeListener(_update);
    app.latency.removeListener(_update);
    _breath?.cancel();
    _icon?.setVisible(false);
    _icon?.dispose();
    _icon = null;
  }
}

/// The status dot in the phase's tone (overview `statusColor`); grey with
/// no phase (access limited).
String _dot(rust.UiPhase? phase) => switch (phase) {
  rust.UiPhase.connected => '🟢',
  rust.UiPhase.error => '🔴',
  rust.UiPhase.contended => '🟠',
  final phase? when isBusy(phase) => '🔵',
  _ => '⚪',
};

/// A speed-test result as text; 「—」 with none yet.
String _latencyText(Latency? latency) => switch (latency) {
  LatencyMs(:final ms) => '$ms ms',
  LatencyTimeout() => S.current('timeout'),
  LatencyFailed() => S.current('failed'),
  LatencyTesting() || null => '—',
};

/// The latency on the right: Windows menus right-align what follows a tab;
/// macOS and GTK have no such column, so it trails the name.
String _withLatency(String name, String latency) => Platform.isWindows ? '$name\t$latency' : '$name   $latency';
