// Screenshot mode for CI (#387): PPVPN_FLUTTER_SAMPLE=overview|nodes|logs
// shows the signed-in pages on fixed data, the design handoff's sample nodes
// (sample-nodes.json) and a few log lines, with no client behind them: no
// backend, no core, no sign-in, no log file read. PPVPN_FLUTTER_THEME=dark|light forces the theme. Only
// apps/flutter/scripts/smoke.sh sets these.

import 'dart:io';
import 'dart:math' as math;

import 'package:flutter/widgets.dart';

import 'app_state.dart';
import 'src/rust/api.dart' as rust;

/// The page the screenshot mode opens on, or null when it is off.
final String? samplePage = switch (Platform.environment['PPVPN_FLUTTER_SAMPLE']) {
  final page? when page == 'overview' || page == 'nodes' || page == 'logs' => page,
  _ => null,
};

/// The forced theme brightness, or null to follow the system.
final Brightness? forcedBrightness = switch (Platform.environment['PPVPN_FLUTTER_THEME']) {
  'dark' => Brightness.dark,
  'light' => Brightness.light,
  _ => null,
};

/// The logs page in the screenshot mode: its directory (as the footer shows
/// it), its "today" and its lines.
final String sampleLogDir = Platform.isMacOS
    ? '~/Library/Logs/PPVPN Flutter'
    : Platform.isWindows
    ? r'%LOCALAPPDATA%\PPVPN Flutter\logs'
    : '~/.local/state/ppvpn-flutter/logs';
final DateTime sampleLogDate = DateTime.utc(2026, 10, 20);
const sampleLogLines = [
  '2026-10-20T14:30:02.418Z  INFO ppvpn_desktop: client created app_version=0.7.0',
  '2026-10-20T14:30:02.733Z  INFO ppvpn_desktop::session: session restored',
  '2026-10-20T14:30:03.120Z  INFO ppvpn_desktop::session: profile ready nodes=11',
  '2026-10-20T14:30:03.402Z  INFO ppvpn_desktop::standard: core started',
  '2026-10-20T14:30:03.958Z  INFO ppvpn_desktop::standard: local proxies listening 127.0.0.1:7890',
  '2026-10-20T14:30:04.211Z  INFO ppvpn_desktop::connection: connecting node=hk1 line=HKG-A',
  '2026-10-20T14:30:04.597Z  INFO ppvpn_desktop::sysproxy: system proxy applied',
  '2026-10-20T14:30:04.603Z  INFO ppvpn_desktop::connection: connected node=hk1 latency_ms=38',
  '2026-10-20T14:31:12.044Z  INFO ppvpn_desktop::standard: probe finished method=tcp nodes=11',
  '2026-10-20T14:31:12.045Z  WARN ppvpn_desktop::standard: probe timed out node=de1',
  '2026-10-20T14:36:40.881Z  INFO ppvpn_desktop::notifications: unread=3',
  '2026-10-20T14:42:18.306Z  WARN ppvpn_desktop::connection: line failed, failing over from=HKG-A to=HKG-B',
  '2026-10-20T14:42:18.912Z  INFO ppvpn_desktop::connection: connected node=hk1 line=HKG-B latency_ms=46',
  '2026-10-20T14:55:00.007Z  INFO ppvpn_desktop::session: profile unchanged',
];

AppState sampleState() {
  rust.UiNode node(
    String id,
    String country,
    String name,
    String region,
    List<String> lines, {
    String? tier,
    String key = 'standard',
  }) => rust.UiNode(
    id: id,
    name: name,
    exitRegion: region,
    exitCountryCode: country,
    entryLabel: tier,
    entryKey: key,
    udp: true,
    lines: lines,
  );
  final nodes = [
    node('hk1', 'HK', 'Hong Kong 01', 'Hong Kong', ['HKG-A', 'HKG-B', 'SZX-R'], tier: 'Premium', key: 'premium'),
    node('hk2', 'HK', 'Hong Kong 02', 'Hong Kong', ['HKG-C', 'HKG-D']),
    node('tw1', 'TW', 'Taipei 01', 'Taiwan', ['TPE-A', 'TPE-B'], tier: 'Optimized', key: 'cn-optimized'),
    node('jp1', 'JP', 'Tokyo 01', 'Japan', ['NRT-A', 'NRT-B', 'HND-A'], tier: 'Premium', key: 'premium'),
    node('sg1', 'SG', 'Singapore 01', 'Singapore', ['SIN-A', 'SIN-B'], tier: 'Optimized', key: 'cn-optimized'),
    node('kr1', 'KR', 'Seoul 01', 'South Korea', ['ICN-A', 'ICN-B']),
    node('jp2', 'JP', 'Osaka 01', 'Japan', ['KIX-A']),
    node('us1', 'US', 'Los Angeles 01', 'United States', ['LAX-A', 'LAX-B', 'SJC-A'], tier: 'Optimized', key: 'cn-optimized'),
    node('us2', 'US', 'New York 01', 'United States', ['JFK-A']),
    node('de1', 'DE', 'Frankfurt 01', 'Germany', ['FRA-A', 'FRA-B']),
    node('gb1', 'GB', 'London 01', 'United Kingdom', ['LHR-A']),
  ];
  const latencies = {'hk1': 38, 'hk2': 46, 'tw1': 55, 'jp1': 62, 'sg1': 74, 'kr1': 83, 'jp2': 91, 'us1': 168, 'us2': 231};
  final app = AppState.fixed(
    rust.UiSnapshot(
      auth: rust.UiAuth.signedIn,
      deviceCode: null,
      accountName: 'Jordan',
      teamName: 'Acme Studio',
      profile: rust.UiProfile.ready,
      profileRevision: 'sample',
      expiresAt: null,
      phase: rust.UiPhase.connected,
      line: 'HKG-A',
      latencyMs: 38,
      standardReady: true,
      selectedNodeId: 'hk1',
      error: null,
      unread: 3,
    ),
  );
  app.nodes.value = nodes;
  app.latency.value = {
    for (final n in nodes)
      n.id: switch (latencies[n.id]) {
        final ms? => LatencyMs(ms),
        null => n.id == 'de1' ? const LatencyTimeout() : const LatencyFailed(),
      },
  };
  app.proxies.value = {
    for (final n in nodes)
      n.id: rust.UiLocalProxy(nodeId: n.id, host: '127.0.0.1', port: 7890, username: 'u8f2k-${n.id}', password: 'sample'),
  };
  app.history.value = [
    for (var i = 0; i < AppState.historyLength; i++)
      rust.UiTraffic(
        up: (1.2e6 * (0.7 + 0.3 * math.sin(i / 3))).round(),
        down: (8.4e6 * (0.4 + 0.6 * i / AppState.historyLength) * (0.85 + 0.15 * math.cos(i / 2))).round(),
      ),
  ];
  app.traffic.value = app.history.value.last;
  app.connectedSince = DateTime.now().subtract(const Duration(minutes: 42, seconds: 17));
  return app;
}
