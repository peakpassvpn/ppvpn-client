// The Flutter desktop prototype (#387): one window (see shell.dart) with the
// overview and the nodes pages, plus the menu bar / tray icon, on the client
// (crates/desktop) through flutter_rust_bridge. Closing the window hides
// it; the app stays in the tray until "Quit".

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:flutter_rust_bridge/flutter_rust_bridge_for_generated.dart';
import 'package:forui/forui.dart';
import 'package:window_manager/window_manager.dart';

import 'app_state.dart';
import 'bench.dart';
import 'src/rust/api.dart' as rust;
import 'sample.dart';
import 'shell.dart';
import 'src/rust/frb_generated.dart';
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
  mark('main');
  WidgetsFlutterBinding.ensureInitialized();
  final AppState app;
  if (samplePage != null) {
    app = sampleState();
  } else {
    await RustLib.init(externalLibrary: ExternalLibrary.open(_libraryPath()));
    mark('bridge_loaded');
    app = AppState(rust.start());
    mark('client_created');
  }

  await windowManager.ensureInitialized();
  mark('window_manager_ready');
  await windowManager.waitUntilReadyToShow(
    const WindowOptions(
      size: Tokens.windowSize,
      minimumSize: Tokens.windowMinSize,
      center: true,
      title: 'PPVPN',
      // The sidebar runs to the top; macOS keeps its traffic lights over it.
      titleBarStyle: TitleBarStyle.hidden,
    ),
    () async {
      mark('ready_to_show');
      await windowManager.show();
      await windowManager.focus();
      mark('window_shown');
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
  mark('tray_created');
  runApp(PrototypeApp(app: app, tray: tray));
  unawaited(WidgetsBinding.instance.waitUntilFirstFrameRasterized.then((_) => mark('first_frame')));
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
            child: ColoredBox(color: t.bg, child: Shell(app: app)),
          ),
        );
      },
    ),
  );
}
