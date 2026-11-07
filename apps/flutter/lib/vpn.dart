// The system VPN on a phone (#448, the client's tunnel mode): the
// platform's plugin (Android: runner/android VpnPlugin and TunnelService)
// starts and stops it; the tunnel host inside serves tunnel.sock, which the
// client then connects to. AppState drives it: on connect the permission,
// then the VPN, then the client; on disconnect the client, then the VPN.

import 'dart:async';

import 'package:flutter/foundation.dart';
import 'package:flutter/services.dart';

const _methods = MethodChannel('com.peakpassvpn.ppvpn/vpn');
const _events = EventChannel('com.peakpassvpn.ppvpn/vpn-state');

/// The system VPN's state as the platform reports it.
enum VpnState { off, connecting, on, disconnecting }

class SystemVpn {
  SystemVpn._();

  /// As last reported; off until the platform says otherwise.
  static final ValueNotifier<VpnState> state = ValueNotifier(VpnState.off);

  static StreamSubscription<Object?>? _subscription;

  /// Follows the platform's state; once, at start.
  static void listen() {
    _subscription ??= _events.receiveBroadcastStream().listen((value) {
      for (final known in VpnState.values) {
        if (known.name == value) state.value = known;
      }
    }, onError: (Object error) => debugPrint('vpn-state: $error'));
  }

  /// True once the user has allowed the VPN (the system asks the first
  /// time); false when they declined.
  static Future<bool> prepare() async =>
      await _methods.invokeMethod<bool>('prepare') ?? false;

  /// Connecting from here on, before the platform first reports it: a
  /// [stop] right after still reaches the service.
  static Future<void> start() async {
    final previous = state.value;
    state.value = VpnState.connecting;
    try {
      await _methods.invokeMethod<void>('start');
    } catch (_) {
      if (state.value == VpnState.connecting) state.value = previous;
      rethrow;
    }
  }

  /// A failed core activation must not leave a freshly started system VPN up.
  /// A rejected update may retain an existing, verified serving connection.
  static Future<void> connect(
    Future<void> Function() activateCore, {
    required bool Function() keepServing,
  }) async {
    if (!await prepare()) return;
    final wasOn = state.value == VpnState.on;
    try {
      if (!wasOn) await start();
      await activateCore();
    } catch (_) {
      var retain = false;
      if (wasOn) {
        try {
          retain = keepServing();
        } catch (_) {
          // An unverifiable owner cannot justify retaining the provider.
        }
      }
      if (!retain) {
        try {
          await stop(force: true);
        } catch (error) {
          debugPrint('vpn stop after failed activation: $error');
        }
      }
      rethrow;
    }
  }

  /// Nothing to do while it is off.
  static Future<void> stop({bool force = false}) async {
    if (!force && state.value == VpnState.off) return;
    await _methods.invokeMethod<void>('stop');
  }
}
