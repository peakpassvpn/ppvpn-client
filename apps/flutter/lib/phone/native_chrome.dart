// iOS 26 system navigation controls. Unsupported hosts keep the Flutter bars.
import 'dart:async';
import 'dart:io';
import 'dart:convert';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import '../haptics.dart';

class NativeChrome {
  static bool enabled = false;

  static Future<void> initialize() async {
    if (!Platform.isIOS) return;
    try {
      enabled =
          await const MethodChannel('com.peakpassvpn.ppvpn/app')
              .invokeMethod<bool>('nativeChromeAvailable') ??
          false;
    } on MissingPluginException {
      enabled = false;
    } on PlatformException {
      enabled = false;
    }
  }
}

class NativeChromeAction {
  const NativeChromeAction({
    required this.id,
    required this.label,
    required this.onPress,
    this.symbol,
    this.enabled = true,
    this.prominent = false,
    this.badge = 0,
  });
  final String id;
  final String label;
  final VoidCallback onPress;
  final String? symbol;
  final bool enabled;
  final bool prominent;
  final int badge;

  Map<String, Object?> get configuration => {
    'id': id,
    'label': label,
    'symbol': symbol,
    'enabled': enabled,
    'prominent': prominent,
    'badge': badge,
  };
}

/// A system control, not a Flutter blur or a rasterized snapshot.
class NativeChromeView extends StatefulWidget {
  const NativeChromeView({
    super.key,
    required this.kind,
    required this.configuration,
    required this.onAction,
    this.frozen = false,
    this.placeholder,
  });
  final bool frozen;
  final Widget? placeholder;
  final String kind;
  final Map<String, Object?> configuration;
  final ValueChanged<String> onAction;

  @override
  State<NativeChromeView> createState() => _NativeChromeViewState();
}

class _NativeChromeViewState extends State<NativeChromeView> {
  MethodChannel? _channel;
  String? _configuration;
  MemoryImage? _snapshot;
  String? _snapshotConfiguration;
  Future<void>? _captureWork;

  Future<void> _capture() {
    if (_captureWork case final work?) return work;
    if (_channel == null || !mounted || widget.frozen) return Future.value();
    final work = _captureImage();
    _captureWork = work;
    return work.whenComplete(() => _captureWork = null);
  }

  Future<void> _captureImage() async {
    try {
      // Selection/configuration may change while UIKit settles or PNG decodes.
      // Never publish an image requested for an older configuration.
      for (var attempt = 0; attempt < 3 && mounted; attempt++) {
        final configuration = _configuration;
        final bytes = await _channel!.invokeMethod<Uint8List>('snapshot');
        if (bytes == null || bytes.isEmpty || !mounted) return;
        final image = MemoryImage(bytes);
        await precacheImage(image, context);
        if (!mounted) return;
        if (configuration != _configuration ||
            configuration != jsonEncode(widget.configuration))
          continue;
        setState(() {
          _snapshot = image;
          _snapshotConfiguration = configuration;
        });
        return;
      }
    } on PlatformException catch (error) {
      debugPrint('native chrome snapshot: ${error.code}');
    } on MissingPluginException {
      // Older runners can still render their existing native view.
    }
  }

  void _created(int id) {
    final channel = MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id');
    _channel = channel;
    channel.setMethodCallHandler((call) async {
      if (mounted && call.method == 'action' && call.arguments is String) {
        unawaited(AppHaptics.selection());
        widget.onAction(call.arguments as String);
      }
    });
    unawaited(_setFrozen());
    unawaited(_update());
  }

  Future<void> _setFrozen() async {
    try {
      await _channel?.invokeMethod<void>('setFrozen', widget.frozen);
    } on PlatformException catch (error) {
      debugPrint('native chrome visibility: ${error.code}');
    } on MissingPluginException {
      // Older runners still receive Flutter's offstage visibility.
    }
  }

  Future<void> _update() async {
    try {
      final configuration = jsonEncode(widget.configuration);
      if (_configuration == configuration || _channel == null) return;
      await _channel!.invokeMethod<void>('update', widget.configuration);
      _configuration = configuration;
      // Cache the exact native appearance while idle; route motion uses this
      // Flutter image instead of swapping to a differently styled header.
      if (widget.kind == 'tabs') _warmSnapshot();
    } on PlatformException catch (error) {
      debugPrint('native chrome update: ${error.code}');
    } on MissingPluginException {
      // The native view may already have been removed during navigation.
    }
  }

  // Cache work is best effort and never gates Navigator.push. Request a frame
  // explicitly so an otherwise idle screen also prepares its native image.
  void _warmSnapshot() {
    WidgetsBinding.instance.addPostFrameCallback((_) => unawaited(_capture()));
    WidgetsBinding.instance.scheduleFrame();
  }

  @override
  void didUpdateWidget(covariant NativeChromeView oldWidget) {
    super.didUpdateWidget(oldWidget);
    if (oldWidget.frozen != widget.frozen) unawaited(_setFrozen());
    unawaited(_update());
    if (oldWidget.frozen && !widget.frozen) {
      if (widget.kind == 'tabs') _warmSnapshot();
    }
  }

  @override
  void dispose() {
    _channel?.setMethodCallHandler(null);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => Stack(
    fit: StackFit.expand,
    children: [
      Offstage(
        // Platform-view backing surfaces must never participate in a moving
        // route, even on a first visit or after theme/locale invalidates PNGs.
        offstage: widget.frozen,
        child: UiKitView(
          viewType: 'com.peakpassvpn.ppvpn/native-${widget.kind}',
          creationParams: widget.configuration,
          creationParamsCodec: const StandardMessageCodec(),
          layoutDirection: Directionality.of(context),
          gestureRecognizers: {
            Factory<OneSequenceGestureRecognizer>(EagerGestureRecognizer.new),
          },
          onPlatformViewCreated: _created,
        ),
      ),
      if (widget.frozen &&
          (_snapshot == null ||
              _snapshotConfiguration != jsonEncode(widget.configuration)) &&
          widget.placeholder != null)
        ExcludeSemantics(child: IgnorePointer(child: widget.placeholder!)),
      if (widget.frozen &&
          _snapshot != null &&
          _snapshotConfiguration == jsonEncode(widget.configuration))
        ExcludeSemantics(
          child: IgnorePointer(
            child: Image(
              image: _snapshot!,
              fit: BoxFit.fill,
              gaplessPlayback: true,
            ),
          ),
        ),
    ],
  );
}

/// Track route motion for the bottom tabs' cached transition image.
/// Navigation bars remain native throughout the route so their system fonts
/// and back controls never swap to a differently styled placeholder.
class RouteChrome extends StatefulWidget {
  const RouteChrome({super.key, required this.builder});
  final Widget Function(BuildContext context, bool settled) builder;
  @override
  State<RouteChrome> createState() => _RouteChromeState();
}

class _RouteChromeState extends State<RouteChrome> {
  Animation<double>? _primary;
  Animation<double>? _secondary;
  bool get _settled =>
      (_primary == null || _primary!.status == AnimationStatus.completed) &&
      (_secondary == null || _secondary!.status == AnimationStatus.dismissed);
  bool _last = true;
  void _changed(AnimationStatus _) {
    final next = _settled;
    if (next != _last) setState(() => _last = next);
  }

  @override
  void didChangeDependencies() {
    super.didChangeDependencies();
    _primary?.removeStatusListener(_changed);
    _secondary?.removeStatusListener(_changed);
    final route = ModalRoute.of(context);
    _primary = route?.animation;
    _secondary = route?.secondaryAnimation;
    _primary?.addStatusListener(_changed);
    _secondary?.addStatusListener(_changed);
    _last = _settled;
  }

  @override
  void dispose() {
    _primary?.removeStatusListener(_changed);
    _secondary?.removeStatusListener(_changed);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => widget.builder(context, _last);
}
