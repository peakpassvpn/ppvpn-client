// iOS 26 system navigation controls. Unsupported hosts keep the Flutter bars.
import 'dart:async';
import 'dart:io';

import 'package:flutter/foundation.dart';
import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

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
  });
  final String kind;
  final Map<String, Object?> configuration;
  final ValueChanged<String> onAction;

  @override
  State<NativeChromeView> createState() => _NativeChromeViewState();
}

class _NativeChromeViewState extends State<NativeChromeView> {
  MethodChannel? _channel;

  void _created(int id) {
    final channel = MethodChannel('com.peakpassvpn.ppvpn/native-chrome/$id');
    _channel = channel;
    channel.setMethodCallHandler((call) async {
      if (mounted && call.method == 'action' && call.arguments is String) {
        widget.onAction(call.arguments as String);
      }
    });
    unawaited(_update());
  }

  Future<void> _update() async {
    try {
      await _channel?.invokeMethod<void>('update', widget.configuration);
    } on PlatformException catch (error) {
      debugPrint('native chrome update: ${error.code}');
    } on MissingPluginException {
      // The native view may already have been removed during navigation.
    }
  }

  @override
  void didUpdateWidget(covariant NativeChromeView oldWidget) {
    super.didUpdateWidget(oldWidget);
    unawaited(_update());
  }

  @override
  void dispose() {
    _channel?.setMethodCallHandler(null);
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => UiKitView(
    viewType: 'com.peakpassvpn.ppvpn/native-${widget.kind}',
    creationParams: widget.configuration,
    creationParamsCodec: const StandardMessageCodec(),
    layoutDirection: Directionality.of(context),
    gestureRecognizers: {
      Factory<OneSequenceGestureRecognizer>(EagerGestureRecognizer.new),
    },
    onPlatformViewCreated: _created,
  );
}
