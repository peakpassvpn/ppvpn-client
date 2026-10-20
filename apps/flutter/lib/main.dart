// The Flutter desktop prototype (#387): one window with the overview and
// the nodes pages, plus the menu bar / tray icon, on the PPVPN client
// (crates/desktop) through flutter_rust_bridge. Closing the window hides
// it; the app stays in the tray until "Quit".

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:forui/forui.dart';
import 'package:window_manager/window_manager.dart';

import 'app_state.dart';
import 'pages/nodes.dart';
import 'pages/overview.dart';
import 'pages/sign_in.dart';
import 'src/rust/api.dart' as rust;
import 'src/rust/frb_generated.dart';
import 'strings.dart';
import 'theme.dart';
import 'tray.dart';

/// The bridge library ci.yml puts next to the app: beside the executable
/// (Windows), in the bundle's lib/ (Linux), in Contents/Frameworks (macOS).
String _libraryPath() {
  final dir = File(Platform.resolvedExecutable).parent.path;
  if (Platform.isWindows) return '$dir\\ppvpn_flutter.dll';
  if (Platform.isMacOS) return '$dir/../Frameworks/libppvpn_flutter.dylib';
  return '$dir/lib/libppvpn_flutter.so';
}

Future<void> main() async {
  WidgetsFlutterBinding.ensureInitialized();
  await RustLib.init(externalLibrary: ExternalLibrary.open(_libraryPath()));
  final app = AppState(rust.start());

  await windowManager.ensureInitialized();
  await windowManager.waitUntilReadyToShow(
    const WindowOptions(size: Tokens.windowSize, minimumSize: Tokens.windowMinSize, center: true, title: 'PPVPN'),
    () async {
      await windowManager.show();
      await windowManager.focus();
    },
  );
  await windowManager.setPreventClose(true);

  Future<void> quit() async {
    app.stop();
    await windowManager.setPreventClose(false);
    await windowManager.destroy();
  }

  Future<void> open() async {
    await windowManager.show();
    await windowManager.focus();
  }

  final tray = Tray(app: app, onOpen: () => unawaited(open()), onQuit: quit);
  windowManager.addListener(_HideOnClose());
  runApp(PrototypeApp(app: app, tray: tray));
}

class _HideOnClose with WindowListener {
  @override
  void onWindowClose() => unawaited(windowManager.hide());
}

class PrototypeApp extends StatelessWidget {
  const PrototypeApp({super.key, required this.app, required this.tray});

  final AppState app;

  /// Held for the app's life.
  final Tray tray;

  @override
  Widget build(BuildContext context) => WidgetsApp(
    title: 'PPVPN',
    color: Tokens.light.accent,
    debugShowCheckedModeBanner: false,
    localizationsDelegates: FLocalizations.localizationsDelegates,
    supportedLocales: FLocalizations.supportedLocales,
    pageRouteBuilder: <T>(RouteSettings settings, WidgetBuilder builder) =>
        PageRouteBuilder<T>(settings: settings, pageBuilder: (context, _, _) => builder(context)),
    home: Builder(
      builder: (context) {
        final t = Tokens.of(context);
        return FTheme(
          data: t.forui,
          child: DefaultTextStyle(
            style: t.body,
            child: ColoredBox(color: t.window, child: _Shell(app: app)),
          ),
        );
      },
    ),
  );
}

enum _Page { overview, nodes }

/// Title bar (status line, page switch, account) over the current page;
/// signed out, only the sign-in page.
class _Shell extends StatefulWidget {
  const _Shell({required this.app});

  final AppState app;

  @override
  State<_Shell> createState() => _ShellState();
}

class _ShellState extends State<_Shell> {
  _Page _page = _Page.overview;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: widget.app.snapshot,
    builder: (context, snapshot, _) {
      final signedIn = snapshot.auth == rust.UiAuth.signedIn;
      return Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          _TitleBar(
            app: widget.app,
            snapshot: snapshot,
            page: signedIn ? _page : null,
            onPage: (page) => setState(() => _page = page),
          ),
          Expanded(
            child: !signedIn
                ? SignInPage(app: widget.app)
                : switch (_page) {
                    _Page.overview => OverviewPage(app: widget.app),
                    _Page.nodes => NodesPage(app: widget.app),
                  },
          ),
        ],
      );
    },
  );
}

class _TitleBar extends StatelessWidget {
  const _TitleBar({required this.app, required this.snapshot, required this.page, required this.onPage});

  final AppState app;
  final rust.UiSnapshot snapshot;

  /// `null` while signed out: no page switch, no account.
  final _Page? page;
  final ValueChanged<_Page> onPage;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final node = app.nodeById(snapshot.selectedNodeId);
    final status = page == null
        ? s('notSignedIn')
        : node == null
        ? statusTitle(snapshot.phase)
        : '${statusTitle(snapshot.phase)} · ${node.name}';
    Widget tab(_Page value, String label) => SizedBox(
      height: Tokens.controlHeight,
      child: FButton(
        variant: value == page ? FButtonVariant.secondary : FButtonVariant.ghost,
        size: FButtonSizeVariant.xs,
        mainAxisSize: MainAxisSize.min,
        selected: value == page,
        onPress: () => onPage(value),
        child: Text(label),
      ),
    );
    return Container(
      height: Tokens.titleBarHeight,
      padding: const EdgeInsets.symmetric(horizontal: 16),
      decoration: BoxDecoration(border: Border(bottom: BorderSide(color: t.border, width: 0.5))),
      child: Row(
        children: [
          Expanded(
            child: Row(
              children: [
                Container(
                  width: 8,
                  height: 8,
                  decoration: BoxDecoration(
                    color: page == null ? t.idle : statusColor(t, snapshot.phase),
                    shape: BoxShape.circle,
                  ),
                ),
                const SizedBox(width: 6),
                Flexible(
                  child: Semantics(
                    liveRegion: true,
                    child: Text(status, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.secondary),
                  ),
                ),
              ],
            ),
          ),
          if (page != null)
            Semantics(
              container: true,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [tab(_Page.overview, s('overview')), const SizedBox(width: 2), tab(_Page.nodes, s('nodes'))],
              ),
            ),
          Expanded(
            child: Align(
              alignment: Alignment.centerRight,
              child: page == null ? const SizedBox() : _AccountButton(app: app, snapshot: snapshot),
            ),
          ),
        ],
      ),
    );
  }
}

class _AccountButton extends StatelessWidget {
  const _AccountButton({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final name = snapshot.accountName ?? '';
    final initial = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    return FPopoverMenu(
      menuAnchor: Alignment.topRight,
      childAnchor: Alignment.bottomRight,
      menu: [
        FItemGroup(
          children: [
            FItem(title: Text(name), subtitle: Text(snapshot.teamName ?? ''), enabled: false),
            FItem(title: Text(S.current('signOut')), onPress: app.signOut),
          ],
        ),
      ],
      builder: (context, controller, _) => SizedBox(
        height: Tokens.controlHeight + 4,
        child: FButton(
          variant: FButtonVariant.ghost,
          size: FButtonSizeVariant.sm,
          mainAxisSize: MainAxisSize.min,
          onPress: controller.toggle,
          semanticsLabel: S.current('account'),
          prefix: Container(
            width: 18,
            height: 18,
            alignment: Alignment.center,
            decoration: BoxDecoration(color: t.accent, shape: BoxShape.circle),
            child: Text(initial, style: TextStyle(fontSize: 10, fontWeight: FontWeight.w700, color: t.onAccent)),
          ),
          suffix: const Icon(FLucideIcons.chevronDown, size: 12),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 140),
            child: Text(snapshot.teamName ?? name, maxLines: 1, overflow: TextOverflow.ellipsis),
          ),
        ),
      ),
    );
  }
}
