// The menu bar / tray icon and its menu (handoff "5. 菜单栏 / 托盘菜单"),
// on tray_manager's native API. It changes only when the snapshot or the
// node list does, never with traffic: frequent tray updates are what made
// other Flutter clients burn CPU (#387).
//
// Signed in: status header, system proxy (checked while on), current node
// submenu, open, quit. Signed out: "not signed in", open, quit. The icon's
// four states differ by shape only (placeholders for the brand mark).

import 'dart:async';
import 'dart:io';

import 'package:tray_manager/tray_manager.dart';

import 'app_state.dart';
import 'bench.dart';
import 'pages/overview.dart';
import 'settings.dart';
import 'src/rust/api.dart' as rust;
import 'strings.dart';

class Tray {
  Tray({required this.app, required this.onOpen, required this.onQuit}) {
    _icon = TrayIcon.create();
    final icon = _icon;
    if (icon == null) return;
    // macOS opens the menu on a click of the icon; elsewhere the menu comes
    // on a right click and a click opens the window.
    icon.addListener((event) {
      if (event is TrayIconClickedEvent && !Platform.isMacOS) onOpen();
    });
    app.snapshot.addListener(_update);
    app.nodes.addListener(_update);
    // A new language: rebuild the menu whatever else changed.
    Settings.language.addListener(() {
      _shown = null;
      _update();
    });
    _update();
    icon.setVisible(true);
    if (trayBench) {
      // compare.py: a rebuild every second, whatever changed.
      Timer.periodic(const Duration(seconds: 1), (_) {
        _shown = null;
        _benchTick++;
        _update();
      });
    }
  }

  int _benchTick = 0;

  final AppState app;
  final void Function() onOpen;
  final Future<void> Function() onQuit;

  TrayIcon? _icon;
  String? _shown;

  void _update() {
    final icon = _icon;
    if (icon == null) return;
    final snapshot = app.snapshot.value;
    final node = app.nodeById(snapshot.selectedNodeId);
    final signedIn = snapshot.auth == rust.UiAuth.signedIn;
    final state = !signedIn
        ? 'off'
        : switch (snapshot.phase) {
            rust.UiPhase.connected => 'on',
            rust.UiPhase.error || rust.UiPhase.contended => 'error',
            final phase when isBusy(phase) => 'busy',
            _ => 'off',
          };
    final title = signedIn ? statusTitle(snapshot.phase) : S.current('notSignedIn');
    // Rebuild only when something shown changes.
    final key = [state, title, snapshot.selectedNodeId, app.nodes.value.length].join('|');
    if (key == _shown) return;
    _shown = key;

    final template = Platform.isMacOS;
    final image = ImageAsset.fromAsset('assets/tray/${state}_${template ? 'template' : 'mono'}.png');
    if (image != null) icon.icon = image;
    if (template) icon.isIconTemplate = true;
    icon.setTooltip(
      [
        'PPVPN · $title',
        if (node != null) node.name,
        if (trayBench) '$_benchTick',
      ].join(' · '),
    );

    final menu = Menu.create();
    if (menu == null) return;
    final header = _item(node == null || !signedIn ? title : '$title · ${node.name}');
    header?.isEnabled = false;
    menu.addItem(header);
    menu.addSeparator();
    if (signedIn) {
      final proxy = MenuItem.createWithLabelAndType(S.current('stdMode'), MenuItemType.checkbox);
      if (proxy != null) {
        proxy.state = snapshot.phase == rust.UiPhase.connected ? MenuItemState.checked : MenuItemState.unchecked;
        proxy.isEnabled = !isBusy(snapshot.phase);
        proxy.addListener((event) {
          if (event is MenuItemClickedEvent) {
            snapshot.phase == rust.UiPhase.connected ? app.disconnect() : app.connect();
          }
        });
        menu.addItem(proxy);
      }
      final nodes = Menu.create();
      final parent = MenuItem.createWithLabelAndType(
        '${S.current('currentNode')}: ${node?.name ?? '—'}',
        MenuItemType.submenu,
      );
      if (nodes != null && parent != null) {
        for (final each in app.nodes.value) {
          final item = MenuItem.createWithLabelAndType(each.name, MenuItemType.radio);
          if (item == null) continue;
          item.state = each.id == snapshot.selectedNodeId ? MenuItemState.checked : MenuItemState.unchecked;
          item.addListener((event) {
            if (event is MenuItemClickedEvent) app.selectNode(each.id);
          });
          nodes.addItem(item);
        }
        parent.submenu = nodes;
        menu.addItem(parent);
      }
      menu.addSeparator();
    }
    menu.addItem(_item(S.current('openMain'), onOpen));
    menu.addSeparator();
    menu.addItem(_item(S.current('quit'), onQuit));
    icon.setContextMenu(menu);
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
    _icon?.setVisible(false);
    _icon?.dispose();
    _icon = null;
  }
}
