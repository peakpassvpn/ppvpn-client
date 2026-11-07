import 'dart:async';

import 'package:flutter/services.dart';

/// The authenticated transport returns true only after the server commits the
/// exact owner/token registration. The backend contract is supplied separately;
/// this bridge never substitutes the legacy token-only API or stores a bearer.
typedef ApplePushEffect = Future<bool> Function(Map<String, Object?> request);
typedef ApplePushNotice = Future<bool> Function(
  String? accountId,
  int? messageId,
  bool opened,
);

class ApplePush {
  ApplePush({
    required this.notice,
    this.register,
    this.unregister,
    this.channel = const MethodChannel('com.peakpassvpn.ppvpn/apple-push'),
  });
  final MethodChannel channel;
  final ApplePushNotice notice;
  final ApplePushEffect? register;
  final ApplePushEffect? unregister;
  Future<void> _effects = Future.value();
  String? _account;
  String? _lastState;
  bool _disposed = false;

  void start() => channel.setMethodCallHandler(_handle);

  Future<void> followAccount({required String state, String? id}) async {
    if (_disposed || (state == _lastState && id == _account)) return;
    _lastState = state;
    _account = state == 'signedIn' ? id : null;
    try {
      await channel.invokeMethod<void>('account', {
        'state': state,
        if (id != null) 'id': id,
      });
    } on MissingPluginException {
      /* Native channel is absent in older/test runners. */
      _lastState = null;
    } on PlatformException {
      _lastState = null;
    }
  }

  /// Runs before Rust revokes auth. Native invalidation emits unregister first;
  /// wait for its serialized transport effect while the old bearer still works.
  Future<void> beforeSignOut() async {
    await followAccount(state: 'signedOut');
    try {
      // A native barrier includes messages queued before the account reply;
      // this does not depend on Flutter method-call/response delivery order.
      await channel
          .invokeMethod<void>('drain')
          .timeout(const Duration(seconds: 15));
      await _effects.timeout(const Duration(seconds: 15));
    } on MissingPluginException {
      await _effects.timeout(const Duration(seconds: 15));
    } on PlatformException {
      // Rust still revokes the session; backend tombstones fence late work.
    } on TimeoutException {
      // Timeout never acknowledges registration/unregistration as committed.
    }
  }

  Future<Object?> _handle(MethodCall call) async {
    if (_disposed || call.arguments is! Map) return false;
    final args = Map<String, Object?>.from(call.arguments as Map);
    if (call.method == 'notice') {
      final owner = args['accountID'];
      if (_account == null || (owner != null && owner != _account))
        return false;
      final raw = args['messageID'];
      final id = raw is String && RegExp(r'^[0-9]+$').hasMatch(raw)
          ? int.tryParse(raw)
          : null;
      return notice(
        owner as String?,
        id != null && id > 0 ? id : null,
        args['opened'] == true,
      );
    }
    if (call.method != 'register' && call.method != 'unregister') return false;
    final remove = call.method == 'unregister';
    final effect = remove ? unregister : register;
    // No backend contract means no registration acknowledgement. Tokens and
    // payloads are deliberately absent from diagnostic output and exceptions.
    if (effect == null) return false;
    final result = Completer<bool>();
    _effects = _effects.then((_) async {
      if (_disposed || (!remove && args['accountID'] != _account)) {
        result.complete(false);
        return;
      }
      try {
        result.complete(await effect(args));
      } catch (_) {
        result.complete(false);
      }
    });
    return result.future;
  }

  Future<void> retry() async {
    try {
      await channel.invokeMethod<void>('retry');
    } on MissingPluginException {
      /* Older runner. */
    }
  }

  /// Silent/offline hints are recovered on foregrounding only in an enabled
  /// signed package. Disabled builds do not add push-driven network work.
  Future<bool> resume() async {
    try {
      if (await channel.invokeMethod<bool>('configured') != true) return false;
      await retry();
      return true;
    } on MissingPluginException {
      return false;
    } on PlatformException {
      return false;
    }
  }

  void dispose() {
    _disposed = true;
    channel.setMethodCallHandler(null);
  }
}
