// The UI's view of the client. The state lives in Rust; this only holds the
// latest copies it pushed (ValueNotifier per kind) and forwards actions.

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'src/rust/api.dart' as rust;

/// A speed-test result as the nodes page shows it.
sealed class Latency {
  const Latency();
}

class LatencyTesting extends Latency {
  const LatencyTesting();
}

class LatencyMs extends Latency {
  const LatencyMs(this.ms);
  final int ms;
}

class LatencyTimeout extends Latency {
  const LatencyTimeout();
}

class LatencyFailed extends Latency {
  const LatencyFailed();
}

class AppState {
  AppState(rust.UiSnapshot initial) : snapshot = ValueNotifier(initial) {
    _refreshNodes(initial);
    unawaited(_pump());
  }

  /// Fixed state with no client behind it (sample.dart, for CI
  /// screenshots): nothing is read from Rust, actions fail quietly.
  AppState.fixed(rust.UiSnapshot initial) : snapshot = ValueNotifier(initial) {
    _stopped = true;
  }

  final ValueNotifier<rust.UiSnapshot> snapshot;
  final ValueNotifier<List<rust.UiNode>> nodes = ValueNotifier(const []);
  final ValueNotifier<rust.UiTraffic> traffic = ValueNotifier(const rust.UiTraffic(up: 0, down: 0));

  /// The last [historyLength] samples, oldest first (the sparkline).
  final ValueNotifier<List<rust.UiTraffic>> history = ValueNotifier(const []);
  static const historyLength = 30;

  /// When the connection last came on; null while it is not on. The
  /// connection card's 「本次连接」 timer counts from it.
  DateTime? connectedSince;
  final ValueNotifier<Map<String, Latency>> latency = ValueNotifier(const {});
  final ValueNotifier<Map<String, rust.UiLocalProxy>> proxies = ValueNotifier(const {});

  /// The latest failed action, for the one-time error dialog.
  final ValueNotifier<String?> actionError = ValueNotifier(null);

  /// Set once [stop] runs: the event loop ends.
  bool _stopped = false;
  String? _revision;
  bool _standardReady = false;

  Future<void> _pump() async {
    while (!_stopped) {
      final rust.UiEvents events;
      try {
        events = await rust.nextEvents();
      } catch (error) {
        debugPrint('next_events: $error');
        await Future<void>.delayed(const Duration(seconds: 1));
        continue;
      }
      final next = events.snapshot;
      if (next != null) {
        if (next.phase == rust.UiPhase.connected) {
          connectedSince ??= DateTime.now();
        } else if (next.phase != rust.UiPhase.reconnecting) {
          connectedSince = null;
        }
        snapshot.value = next;
        _refreshNodes(next);
      }
      final sample = events.traffic;
      if (sample != null) {
        traffic.value = sample;
        final list = [...history.value, sample];
        history.value = list.length > historyLength ? list.sublist(list.length - historyLength) : list;
      }
      if (events.probes.isNotEmpty) {
        final map = Map.of(latency.value);
        for (final probe in events.probes) {
          final ms = probe.latencyMs;
          map[probe.nodeId] = probe.success && ms != null
              ? LatencyMs(ms)
              : probe.timedOut
              ? const LatencyTimeout()
              : const LatencyFailed();
        }
        latency.value = map;
      }
    }
  }

  void _refreshNodes(rust.UiSnapshot next) {
    if (next.profileRevision != _revision) {
      _revision = next.profileRevision;
      try {
        nodes.value = rust.nodes();
      } catch (error) {
        debugPrint('nodes: $error');
      }
    }
    if (next.standardReady && !_standardReady) unawaited(_loadProxies());
    _standardReady = next.standardReady;
  }

  Future<void> _loadProxies() async {
    try {
      final list = await rust.localProxies();
      proxies.value = {for (final proxy in list) proxy.nodeId: proxy};
    } catch (error) {
      debugPrint('local_proxies: $error');
    }
  }

  rust.UiNode? nodeById(String? id) {
    for (final node in nodes.value) {
      if (node.id == id) return node;
    }
    return null;
  }

  Future<void> _run(Future<void> Function() action) async {
    try {
      await action();
    } catch (error) {
      actionError.value = '$error';
    }
  }

  Future<void> signIn() => _run(rust.signIn);
  void cancelSignIn() => rust.cancelSignIn();
  Future<void> signOut() => _run(rust.signOut);
  Future<void> connect() => _run(rust.connect);
  Future<void> disconnect() => _run(rust.disconnect);
  Future<void> retry() => _run(rust.retry);
  Future<void> refreshNodes() => _run(rust.refreshNodes);
  Future<void> selectNode(String id) => _run(() => rust.selectNode(nodeId: id));

  Future<void> testAll(rust.UiProbeMethod method) async {
    latency.value = {for (final node in nodes.value) node.id: const LatencyTesting()};
    await _run(() => rust.probeAll(method: method));
  }

  /// Puts the system proxy back and stops the core; before the app quits.
  void stop() {
    if (_stopped) return;
    _stopped = true;
    rust.shutdown();
  }
}
