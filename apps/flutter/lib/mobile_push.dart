// APNs / FCM host callbacks (#539). The host keeps the latest token and
// notification tap until start: neither depends on Dart's startup timing.
import 'dart:async';

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

class MobilePush with WidgetsBindingObserver {
  MobilePush({required this.onToken, required this.onOpen, required this.onForeground});

  static const channel = MethodChannel('com.peakpassvpn.ppvpn/push');
  final void Function(String token, String? bundleId, {String? environment}) onToken;
  final Future<void> Function(int? messageId, {String? deliveryId}) onOpen;
  final void Function(bool foreground) onForeground;
  bool _started = false;
  bool _asked = false;

  Future<void> start() async {
    if (_started) return;
    _started = true;
    channel.setMethodCallHandler(_handle);
    WidgetsBinding.instance.addObserver(this);
    didChangeAppLifecycleState(WidgetsBinding.instance.lifecycleState ?? AppLifecycleState.resumed);
    await _invoke('start');
  }

  /// Called after sign-in. Declining does not cause a prompt every launch:
  /// each OS host checks its permission status before asking.
  Future<void> requestPermission() async {
    if (_asked) return;
    _asked = true;
    await _invoke('requestPermission');
  }

  Future<void> _invoke(String method) async {
    try {
      await channel.invokeMethod<void>(method);
    } on MissingPluginException {
      // A platform's native host may land separately from this shared bridge.
    } on PlatformException catch (error) {
      debugPrint('push $method: ${error.code}');
    }
  }

  Future<void> _handle(MethodCall call) async {
    final args = call.arguments;
    if (args is! Map) return;
    if (call.method == 'token') {
      final token = args['token'];
      final bundleId = args['bundle_id'];
      final environment = args['environment'];
      if (token is! String || token.trim().isEmpty) return;
      onToken(
        token,
        bundleId is String && bundleId.isNotEmpty ? bundleId : null,
        environment: environment is String && environment.isNotEmpty ? environment : null,
      );
    } else if (call.method == 'opened') {
      final rawDelivery = args['delivery_id'];
      final hasDelivery = args.containsKey('delivery_id') && rawDelivery != null;
      final deliveryId = rawDelivery is String && rawDelivery.trim().isNotEmpty
          ? rawDelivery.trim()
          : (hasDelivery ? 'invalid' : null);
      final value = args['message_id'];
      final id = value is int
          ? value
          : value is String
          ? int.tryParse(value)
          : null;
      // Payloads select a delivery or message, never supply its trusted body
      // or a URL to launch. The signed-in account's inbox resolves it.
      await onOpen(id != null && id > 0 ? id : null, deliveryId: deliveryId);
    }
  }

  @override
  void didChangeAppLifecycleState(AppLifecycleState state) {
    onForeground(state == AppLifecycleState.resumed);
  }

  void dispose() {
    if (!_started) return;
    WidgetsBinding.instance.removeObserver(this);
    channel.setMethodCallHandler(null);
    _started = false;
  }
}
