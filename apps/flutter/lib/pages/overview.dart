// Overview (design Main / DesktopDark): the connection card (status orb,
// title, node and latency, the session timer, the route), the modes and
// node card, then live traffic and the local proxy side by side. A full
// empty state instead while access is limited. The prototype has the
// system proxy (standard mode) only: the enhanced-mode row is shown,
// switched off and disabled.

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

/// Busy phases: the switch shows its in-between state.
bool isBusy(rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.preparing ||
  rust.UiPhase.waitingPermission ||
  rust.UiPhase.connecting ||
  rust.UiPhase.reconnecting ||
  rust.UiPhase.disconnecting => true,
  _ => false,
};

/// The status title for the system proxy (handoff "状态标题规则").
String statusTitle(rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.connected => S.current('h_on'),
  rust.UiPhase.disconnecting => S.current('h_stdStopping'),
  rust.UiPhase.error || rust.UiPhase.contended => S.current('h_stdFail'),
  _ when isBusy(phase) => S.current('h_stdStarting'),
  _ => S.current('h_idle'),
};

Color statusColor(Tokens t, rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.connected => t.ok,
  rust.UiPhase.error || rust.UiPhase.contended => t.bad,
  _ when isBusy(phase) => t.accent,
  _ => t.t3,
};

class OverviewPage extends StatelessWidget {
  const OverviewPage({super.key, required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final limited = LimitedAccess.of(snapshot);
      if (limited != null) return limited;
      if (snapshot.profile == rust.UiProfile.loading) return const LoadingNodes();
      return SingleChildScrollView(
        padding: Tokens.pagePadding,
        child: Center(
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: Tokens.pageMaxWidth),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                if (snapshot.profile == rust.UiProfile.invalid) ...[
                  const ConfigWarning(),
                  const SizedBox(height: Tokens.cardGap),
                ],
                _Hero(app: app, snapshot: snapshot),
                if (snapshot.phase == rust.UiPhase.error) ...[
                  const SizedBox(height: Tokens.cardGap),
                  _Problem(app: app, error: snapshot.error),
                ],
                const SizedBox(height: Tokens.cardGap),
                _Modes(app: app, snapshot: snapshot),
                const SizedBox(height: Tokens.cardGap),
                IntrinsicHeight(
                  child: Row(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(child: _Traffic(app: app)),
                      const SizedBox(width: Tokens.cardGap),
                      Expanded(child: _LocalProxy(app: app, nodeId: snapshot.selectedNodeId)),
                    ],
                  ),
                ),
              ],
            ),
          ),
        ),
      );
    },
  );
}

/// The connection card.
class _Hero extends StatelessWidget {
  const _Hero({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final phase = snapshot.phase;
    final connected = phase == rust.UiPhase.connected;
    final busy = isBusy(phase);
    final tone = statusColor(t, phase);
    final node = app.nodeById(snapshot.selectedNodeId);
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: t.cardDecoration(),
      child: Stack(
        children: [
          // The tint from the top left, in the state's colour.
          Positioned.fill(
            child: DecoratedBox(
              decoration: BoxDecoration(
                gradient: RadialGradient(
                  center: Alignment.topLeft,
                  radius: 1.3,
                  colors: [tone.withValues(alpha: 0.11), tone.withValues(alpha: 0)],
                  stops: const [0, 0.55],
                ),
              ),
            ),
          ),
          // The dot grid on the right half, fading in from the left.
          Positioned.fill(child: CustomPaint(painter: _Dots(tone))),
          Padding(
            padding: const EdgeInsets.fromLTRB(22, 20, 22, 18),
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    _Orb(phase: phase),
                    const SizedBox(width: 16),
                    Expanded(
                      child: Column(
                        crossAxisAlignment: CrossAxisAlignment.start,
                        children: [
                          Semantics(
                            header: true,
                            liveRegion: true,
                            child: Text(statusTitle(phase), style: t.heroTitle),
                          ),
                          const SizedBox(height: 2),
                          if (phase == rust.UiPhase.off)
                            Text(s('d_idle'), style: t.sub)
                          else if (node != null)
                            Row(
                              children: [
                                Flag(countryCode: node.exitCountryCode),
                                const SizedBox(width: 8),
                                Flexible(
                                  child: Text(
                                    nodeTitle(node),
                                    maxLines: 1,
                                    overflow: TextOverflow.ellipsis,
                                    style: t.sub.copyWith(color: t.t1, fontWeight: FontWeight.w600),
                                  ),
                                ),
                                const SizedBox(width: 8),
                                LatencyPill.ms(snapshot.latencyMs),
                              ],
                            ),
                        ],
                      ),
                    ),
                    if (connected)
                      _SessionTimer(since: app.connectedSince)
                    else if (busy)
                      Column(
                        crossAxisAlignment: CrossAxisAlignment.end,
                        children: [
                          Text(s('stdMode'), style: t.sectionTitle.copyWith(color: t.accent)),
                          Text(statusTitle(phase), style: t.note),
                        ],
                      ),
                  ],
                ),
                if (node != null && phase != rust.UiPhase.off) ...[
                  const SizedBox(height: 18),
                  Padding(
                    padding: const EdgeInsets.only(left: Tokens.orbSize + 16),
                    child: _Route(lines: node.lines, current: snapshot.line, connected: connected),
                  ),
                ],
              ],
            ),
          ),
        ],
      ),
    );
  }
}

class _Orb extends StatelessWidget {
  const _Orb({required this.phase});

  final rust.UiPhase phase;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final connected = phase == rust.UiPhase.connected;
    final busy = isBusy(phase);
    final tone = statusColor(t, phase);
    final light = t.brightness == Brightness.light;
    return SizedBox(
      width: Tokens.orbSize,
      height: Tokens.orbSize,
      child: Stack(
        alignment: Alignment.center,
        children: [
          Container(
            decoration: BoxDecoration(
              shape: BoxShape.circle,
              gradient: connected && light ? Tokens.connectedOrb : null,
              color: connected && light ? null : tone.withValues(alpha: 0.10),
              border: connected && light ? null : Border.all(color: tone.withValues(alpha: 0.25)),
              boxShadow: [
                BoxShadow(color: tone.withValues(alpha: 0.12), spreadRadius: 6),
                BoxShadow(color: tone.withValues(alpha: 0.06), spreadRadius: 13),
                if (connected && light) BoxShadow(color: tone.withValues(alpha: 0.30), offset: const Offset(0, 6), blurRadius: 16),
              ],
            ),
          ),
          if (busy) Spinner(color: tone, size: Tokens.orbSize),
          if (phase == rust.UiPhase.error || phase == rust.UiPhase.contended)
            Icon(FLucideIcons.circleAlert, size: 26, color: tone)
          else if (phase == rust.UiPhase.off)
            Icon(FLucideIcons.link2Off, size: 24, color: tone)
          else
            ShieldCheckIcon(size: 26, stroke: 2.2, color: connected && light ? const Color(0xFFFFFFFF) : tone),
        ],
      ),
    );
  }
}

class _Dots extends CustomPainter {
  _Dots(this.color);
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    // The right 48 %, fading in from its left edge over 60 % of it.
    final left = size.width * 0.52;
    final span = size.width - left;
    for (var x = left + 6; x < size.width; x += 12) {
      final fade = ((x - left) / (span * 0.6)).clamp(0.0, 1.0);
      final paint = Paint()..color = color.withValues(alpha: 0.2 * 0.7 * fade);
      for (var y = 6.0; y < size.height; y += 12) {
        canvas.drawCircle(Offset(x, y), 1, paint);
      }
    }
  }

  @override
  bool shouldRepaint(_Dots old) => old.color != color;
}

/// 「本次连接」: hh:mm:ss since the connection came on, once a second.
class _SessionTimer extends StatefulWidget {
  const _SessionTimer({required this.since});

  final DateTime? since;

  @override
  State<_SessionTimer> createState() => _SessionTimerState();
}

class _SessionTimerState extends State<_SessionTimer> {
  late final Timer _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));

  @override
  void dispose() {
    _tick.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final since = widget.since;
    final elapsed = since == null ? Duration.zero : DateTime.now().difference(since);
    String two(int n) => n.toString().padLeft(2, '0');
    final text = '${two(elapsed.inHours)}:${two(elapsed.inMinutes % 60)}:${two(elapsed.inSeconds % 60)}';
    return Column(
      crossAxisAlignment: CrossAxisAlignment.end,
      children: [
        Text(text, style: t.monoText(22, weight: FontWeight.w500, height: 28)),
        Text(S.current('thisSession'), style: t.note),
      ],
    );
  }
}

/// 本机 → the node's lines → 互联网. The line in use and those before it are
/// lit while connected; the rest wait (dashed).
class _Route extends StatelessWidget {
  const _Route({required this.lines, required this.current, required this.connected});

  final List<String> lines;
  final String? current;
  final bool connected;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final tone = connected ? t.ok : t.accent;
    final reached = current == null ? 0 : lines.indexOf(current!) + 1;
    Widget hop(String text, {IconData? icon, bool lit = false, bool wait = false, bool mono = false}) => Container(
      height: 26,
      padding: const EdgeInsets.symmetric(horizontal: 10),
      decoration: BoxDecoration(
        color: wait ? null : t.hop,
        borderRadius: BorderRadius.circular(13),
        border: Border.all(color: lit ? tone.withValues(alpha: 0.25) : (wait ? t.line : t.hair)),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (icon != null) ...[Icon(icon, size: 13, color: wait ? t.t3 : t.t2), const SizedBox(width: 6)],
          Text(
            text,
            style: mono
                ? t.monoText(12, weight: FontWeight.w500, color: lit ? tone : (wait ? t.t3 : t.t2))
                : t.note.copyWith(fontWeight: FontWeight.w500, color: wait ? t.t3 : t.t2),
          ),
        ],
      ),
    );
    Widget link(bool wait) => Flexible(
      child: ConstrainedBox(
        constraints: const BoxConstraints(maxWidth: 72),
        child: Container(
          height: 2,
          margin: const EdgeInsets.symmetric(horizontal: 4),
          child: CustomPaint(painter: _Dashes(wait ? t.t3.withValues(alpha: 0.4) : tone.withValues(alpha: 0.55), wait)),
        ),
      ),
    );
    final shown = lines.take(3).toList();
    return Semantics(
      label: [S.current('thisDevice'), ...shown, S.current('internet')].join(' → '),
      child: ExcludeSemantics(
        child: Row(
          children: [
            hop(S.current('thisDevice'), icon: FLucideIcons.monitor),
            for (var i = 0; i < shown.length; i++) ...[
              link(i >= reached),
              hop(shown[i], lit: i < reached, wait: i >= reached, mono: true),
            ],
            link(!connected),
            hop(S.current('internet'), icon: FLucideIcons.globe, wait: !connected),
          ],
        ),
      ),
    );
  }
}

class _Dashes extends CustomPainter {
  _Dashes(this.color, this.short);
  final Color color;
  final bool short;

  @override
  void paint(Canvas canvas, Size size) {
    final paint = Paint()..color = color;
    final (dash, gap) = short ? (3.0, 5.0) : (6.0, 4.0);
    for (var x = 0.0; x < size.width; x += dash + gap) {
      canvas.drawRRect(
        RRect.fromRectAndRadius(Rect.fromLTWH(x, 0, dash, size.height), const Radius.circular(1)),
        paint,
      );
    }
  }

  @override
  bool shouldRepaint(_Dashes old) => old.color != color || old.short != short;
}

/// System proxy, enhanced mode and the current node, 58-high rows.
class _Modes extends StatelessWidget {
  const _Modes({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final s = S.current;
    final phase = snapshot.phase;
    final busy = isBusy(phase);
    return CardBox(
      child: Column(
        children: [
          SettingRow(
            icon: FLucideIcons.monitor,
            title: s('stdMode'),
            description: s('stdDesc'),
            trailing: TriSwitch(
              key: const ValueKey('switch-system-proxy'),
              label: s('stdMode'),
              state: phase == rust.UiPhase.connected
                  ? SwitchState.on
                  : busy
                  ? SwitchState.busy
                  : SwitchState.off,
              onChanged: (on) => on ? app.connect() : app.disconnect(),
            ),
          ),
          SettingRow(
            icon: FLucideIcons.zap,
            title: s('tunMode'),
            description: s('protoNoTun'),
            divider: true,
            trailing: TriSwitch(label: s('tunMode'), state: SwitchState.off, enabled: false, onChanged: null),
          ),
          SettingRow(
            icon: FLucideIcons.mapPin,
            title: s('currentNode'),
            divider: true,
            trailing: NodePicker(app: app, selectedId: snapshot.selectedNodeId),
          ),
        ],
      ),
    );
  }
}

/// The current-node dropdown (34 high): flag, name, latency, chevron.
class NodePicker extends StatelessWidget {
  const NodePicker({super.key, required this.app, required this.selectedId});

  final AppState app;
  final String? selectedId;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return ValueListenableBuilder(
      valueListenable: app.nodes,
      builder: (context, nodes, _) {
        final selected = app.nodeById(selectedId);
        return FPopoverMenu(
          maxHeight: 320,
          menuAnchor: Alignment.topRight,
          childAnchor: Alignment.bottomRight,
          menu: [
            FItemGroup(
              children: [
                for (final node in nodes)
                  FItem(
                    selected: node.id == selectedId,
                    prefix: Flag(countryCode: node.exitCountryCode),
                    title: Text(nodeTitle(node), maxLines: 1, overflow: TextOverflow.ellipsis),
                    suffix: ValueListenableBuilder(
                      valueListenable: app.latency,
                      builder: (context, latency, _) => LatencyPill(latency: latency[node.id]),
                    ),
                    onPress: () => app.selectNode(node.id),
                  ),
              ],
            ),
          ],
          builder: (context, controller, _) => Pressable(
            key: const ValueKey('node-picker'),
            onPress: controller.toggle,
            label: '${S.current('currentNode')}: ${selected == null ? '—' : nodeTitle(selected)}',
            builder: (context, hovered) => Container(
              height: 34,
              constraints: const BoxConstraints(maxWidth: 320),
              padding: const EdgeInsets.fromLTRB(8, 0, 10, 0),
              decoration: BoxDecoration(
                color: t.brightness == Brightness.light ? t.card : t.sunk,
                borderRadius: BorderRadius.circular(Tokens.controlRadius),
                border: Border.all(color: t.line),
              ),
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  if (selected != null) ...[Flag(countryCode: selected.exitCountryCode), const SizedBox(width: 10)],
                  Flexible(
                    child: Text(
                      selected == null ? '—' : nodeTitle(selected),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.control.copyWith(color: t.t1),
                    ),
                  ),
                  if (selected != null) ...[
                    const SizedBox(width: 10),
                    ValueListenableBuilder(
                      valueListenable: app.snapshot,
                      builder: (context, snapshot, _) => LatencyPill.ms(snapshot.latencyMs),
                    ),
                  ],
                  const SizedBox(width: 10),
                  Icon(FLucideIcons.chevronDown, size: 12, color: t.t3),
                ],
              ),
            ),
          ),
        );
      },
    );
  }
}

class _Problem extends StatelessWidget {
  const _Problem({required this.app, required this.error});

  final AppState app;
  final rust.UiError? error;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final reason = error?.code == 'SystemProxyFailed' || error == null ? s('fr_port') : error!.code;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        color: t.badSoft,
        borderRadius: BorderRadius.circular(Tokens.cardRadius),
        border: Border.all(color: t.hair),
      ),
      child: Row(
        children: [
          Icon(FLucideIcons.circleAlert, size: 16, color: t.bad),
          const SizedBox(width: 10),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(s('h_stdFail'), style: t.sectionTitle),
                Text(reason, style: t.note),
              ],
            ),
          ),
          Button(kind: ButtonKind.secondary, label: s('retry'), onPress: app.retry),
        ],
      ),
    );
  }
}

class _Traffic extends StatelessWidget {
  const _Traffic({required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    Widget rate(String label, Color key, int value) {
      final (number, unit) = formatRate(value);
      return Column(
        crossAxisAlignment: CrossAxisAlignment.start,
        children: [
          Row(
            children: [
              Container(
                width: 8,
                height: 3,
                decoration: BoxDecoration(color: key, borderRadius: BorderRadius.circular(2)),
              ),
              const SizedBox(width: 6),
              Text(label, style: t.note.copyWith(color: t.t2)),
            ],
          ),
          Text.rich(
            TextSpan(
              children: [
                TextSpan(text: number, style: t.rate),
                TextSpan(
                  text: ' $unit',
                  style: t.monoText(12, weight: FontWeight.w500, color: t.t3),
                ),
              ],
            ),
          ),
        ],
      );
    }

    return CardBox(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(s('traffic'), style: t.sectionTitle),
          const SizedBox(height: 10),
          ValueListenableBuilder(
            valueListenable: app.traffic,
            builder: (context, sample, _) => Row(
              children: [
                Expanded(child: rate(s('upload'), t.accent2, sample.up)),
                const SizedBox(width: 12),
                Expanded(child: rate(s('download'), t.ok, sample.down)),
              ],
            ),
          ),
          const SizedBox(height: 8),
          SizedBox(
            height: 44,
            child: ValueListenableBuilder(
              valueListenable: app.history,
              builder: (context, history, _) => CustomPaint(
                painter: _Sparkline(history, up: t.accent2, down: t.ok),
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// Download (filled) and upload lines over the last samples, on one scale.
class _Sparkline extends CustomPainter {
  _Sparkline(this.samples, {required this.up, required this.down});

  final List<rust.UiTraffic> samples;
  final Color up;
  final Color down;

  @override
  void paint(Canvas canvas, Size size) {
    if (samples.length < 2) return;
    var peak = 1;
    for (final s in samples) {
      if (s.down > peak) peak = s.down;
      if (s.up > peak) peak = s.up;
    }
    final step = size.width / (AppState.historyLength - 1);
    final start = size.width - step * (samples.length - 1);
    Path line(int Function(rust.UiTraffic) pick) {
      final path = Path();
      for (var i = 0; i < samples.length; i++) {
        final point = Offset(start + step * i, size.height - 2 - (size.height - 6) * pick(samples[i]) / peak);
        i == 0 ? path.moveTo(point.dx, point.dy) : path.lineTo(point.dx, point.dy);
      }
      return path;
    }

    Paint stroke(Color color) => Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = 1.6
      ..strokeJoin = StrokeJoin.round;
    final downLine = line((s) => s.down);
    final area = Path.from(downLine)
      ..lineTo(size.width, size.height)
      ..lineTo(start, size.height)
      ..close();
    canvas.drawPath(
      area,
      Paint()
        ..shader = LinearGradient(
          begin: Alignment.topCenter,
          end: Alignment.bottomCenter,
          colors: [down.withValues(alpha: 0.22), down.withValues(alpha: 0)],
        ).createShader(Offset.zero & size),
    );
    canvas.drawPath(downLine, stroke(down));
    canvas.drawPath(line((s) => s.up), stroke(up));
  }

  @override
  bool shouldRepaint(_Sparkline old) => !identical(old.samples, samples) || old.up != up || old.down != down;
}

class _LocalProxy extends StatelessWidget {
  const _LocalProxy({required this.app, required this.nodeId});

  final AppState app;
  final String? nodeId;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return CardBox(
      padding: const EdgeInsets.symmetric(horizontal: 20, vertical: 16),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Text(s('localProxy'), style: t.sectionTitle),
          Padding(padding: const EdgeInsets.only(top: 2), child: Text(s('proxyNote'), style: t.note)),
          ValueListenableBuilder(
            valueListenable: app.proxies,
            builder: (context, proxies, _) {
              final proxy = proxies[nodeId];
              if (proxy == null) {
                return Padding(
                  padding: const EdgeInsets.only(top: 9),
                  child: Text('—', style: t.note),
                );
              }
              return ProxyRows(proxy: proxy);
            },
          ),
        ],
      ),
    );
  }
}

/// HTTP and SOCKS5 with copy buttons, then the user name and the masked
/// password: 28-high sunk rows. The copied URL carries the credentials.
class ProxyRows extends StatelessWidget {
  const ProxyRows({super.key, required this.proxy});

  final rust.UiLocalProxy proxy;

  static String url(rust.UiLocalProxy proxy, String scheme) {
    final user = Uri.encodeComponent(proxy.username);
    final password = Uri.encodeComponent(proxy.password);
    return '$scheme://$user:$password@${proxy.host}:${proxy.port}';
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    Widget row(String key, String value, Widget trailing) => Container(
      height: 28,
      margin: const EdgeInsets.only(top: 9),
      padding: const EdgeInsets.fromLTRB(10, 0, 4, 0),
      decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(Tokens.segmentRadius)),
      child: Row(
        children: [
          // 54 in the design (Chinese); no wrapping for longer labels.
          ConstrainedBox(
            constraints: const BoxConstraints(minWidth: 54),
            child: Text(key, maxLines: 1, softWrap: false, style: t.note),
          ),
          const SizedBox(width: 8),
          Expanded(
            child: Text(
              value,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: t.monoText(12),
            ),
          ),
          trailing,
        ],
      ),
    );
    final endpoint = '${proxy.host}:${proxy.port}';
    return Column(
      children: [
        row('HTTP', 'http://$endpoint', CopyButton(text: url(proxy, 'http'))),
        row('SOCKS5', 'socks5://$endpoint', CopyButton(text: url(proxy, 'socks5h'))),
        row(
          S.current('authUser'),
          proxy.username,
          Padding(
            padding: const EdgeInsets.only(right: 8),
            child: Text('${S.current('authPass')} ••••••', style: t.monoText(12, color: t.t3)),
          ),
        ),
      ],
    );
  }
}

/// Access limited: no subscription, subscription expired, team disabled
/// (a full empty state replaces the page).
class LimitedAccess extends StatelessWidget {
  const LimitedAccess._({required this.icon, required this.title, required this.description});

  final IconData icon;
  final String title;
  final String description;

  static Widget? of(rust.UiSnapshot snapshot) {
    final s = S.current;
    return switch (snapshot.profile) {
      rust.UiProfile.noSubscription => LimitedAccess._(
        icon: FLucideIcons.shield,
        title: s('noSubT'),
        description: s('noSubD'),
      ),
      rust.UiProfile.subscriptionExpired => LimitedAccess._(
        icon: FLucideIcons.clock,
        title: s('expiredT'),
        description: s('expiredD', {'d': snapshot.expiresAt ?? '—'}),
      ),
      rust.UiProfile.teamDisabled => LimitedAccess._(
        icon: FLucideIcons.circleAlert,
        title: s('teamOffT'),
        description: s('teamOffD', {'team': snapshot.teamName ?? '—'}),
      ),
      _ => null,
    };
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(
      child: Padding(
        padding: Tokens.pagePadding,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 44, color: t.t3),
            const SizedBox(height: 12),
            Semantics(header: true, child: Text(title, style: t.heroTitle, textAlign: TextAlign.center)),
            const SizedBox(height: 6),
            Text(description, style: t.sub, textAlign: TextAlign.center),
          ],
        ),
      ),
    );
  }
}

class LoadingNodes extends StatelessWidget {
  const LoadingNodes({super.key});

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Spinner(color: t.accent, size: 24),
          const SizedBox(height: 12),
          Text(S.current('loadingT'), style: t.sectionTitle),
          const SizedBox(height: 4),
          Text(S.current('loadingD'), style: t.sub),
        ],
      ),
    );
  }
}

class ConfigWarning extends StatelessWidget {
  const ConfigWarning({super.key});

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 16, vertical: 10),
      decoration: BoxDecoration(
        color: t.warnSoft,
        borderRadius: BorderRadius.circular(Tokens.cardRadius),
        border: Border.all(color: t.hair),
      ),
      child: Row(
        children: [
          Icon(FLucideIcons.circleAlert, size: 16, color: t.warn),
          const SizedBox(width: 10),
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(text: '${S.current('cfgT')} · ', style: t.sectionTitle),
                  TextSpan(text: S.current('cfgD'), style: t.sub),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
