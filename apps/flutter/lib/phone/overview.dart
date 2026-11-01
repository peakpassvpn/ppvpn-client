// The phone overview (#448 step 5), to the canvas's 手机 · 概览 (#535): the
// brand and the bell on top, the page tinted by the status; the connection
// card (radius 28, tinted too: the status capsule with its breathing dot,
// the session's time, the 168 main button, the route), the problem card
// below it, one card with the current node (opens 节点) and the routing
// mode, live traffic. One connection, so no modes card; no local proxy
// card. The desktop's full-page gates and the invalid-profile warning as
// they are.

import 'dart:async';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../pages/overview.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'phone.dart';

class PhoneOverview extends StatelessWidget {
  const PhoneOverview({super.key, required this.app, required this.bell, required this.onNodes});

  final AppState app;
  final Widget bell;

  /// The current node's row: the 节点 tab.
  final VoidCallback onNodes;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final t = Tokens.of(context);
      final limited = LimitedAccess.of(snapshot);
      final loading = snapshot.profile == rust.UiProfile.loading;
      // The top of the page in the status colour, fading into the
      // background (spec: 状态铺色).
      final tint = Color.alphaBlend(statusColor(t, snapshot).withValues(alpha: 0.10), t.bg);
      return PhoneScaffold(
        title: S.current('overview'),
        brand: true,
        trailing: bell,
        background: BoxDecoration(
          gradient: LinearGradient(
            begin: Alignment.topCenter,
            end: Alignment.bottomCenter,
            stops: const [0, 0.45],
            colors: [tint, t.bg],
          ),
        ),
        child: Swap(
          id: limited != null || loading ? snapshot.profile : null,
          expand: true,
          child: limited ?? (loading ? const LoadingNodes() : _page(context, snapshot)),
        ),
      );
    },
  );

  Widget _page(BuildContext context, rust.UiSnapshot snapshot) {
    final problem = ProblemCard.of(app, snapshot);
    return ListView(
      padding: phoneScrollPadding(context),
      children: [
        Reveal(
          child: snapshot.profile == rust.UiProfile.invalid
              ? const Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [ConfigWarning(), SizedBox(height: 12)],
                )
              : null,
        ),
        _ConnectionCard(app: app, snapshot: snapshot),
        Reveal(
          id: problem?.title,
          child: problem == null
              ? null
              : Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [const SizedBox(height: 12), problem],
                ),
        ),
        const SizedBox(height: 12),
        _NodeCard(app: app, snapshot: snapshot, onNodes: onNodes),
        const SizedBox(height: 12),
        TrafficCard(app: app, phone: true),
      ],
    );
  }
}

class _ConnectionCard extends StatelessWidget {
  const _ConnectionCard({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final phase = snapshot.phase;
    final connected = phase == rust.UiPhase.connected;
    final tone = statusColor(t, snapshot);
    final node = app.nodeById(snapshot.selectedNodeId);
    final detail = enhancedDetail(snapshot);
    // One line on a phone: the one in use, else the first.
    final names = node == null ? const <String>[] : lineNames(node);
    final inUse = node == null ? -1 : node.lines.indexWhere((line) => line.key == snapshot.lineKey);
    return Container(
      clipBehavior: Clip.antiAlias,
      decoration: t.phoneCardDecoration(
        radius: Tokens.phoneConnectionRadius,
        gradient: RadialGradient(
          center: Alignment.topRight,
          radius: 1.1,
          colors: [Color.alphaBlend(tone.withValues(alpha: 0.16), t.card), t.card],
        ),
      ),
      child: Stack(
        children: [
          // The brand's glow at the bottom left, under the status's.
          Positioned.fill(
            child: IgnorePointer(
              child: DecoratedBox(
                decoration: BoxDecoration(
                  gradient: RadialGradient(
                    center: Alignment.bottomLeft,
                    radius: 0.9,
                    colors: [t.accent2.withValues(alpha: 0.10), t.accent2.withValues(alpha: 0)],
                  ),
                ),
              ),
            ),
          ),
          Padding(
            padding: const EdgeInsets.fromLTRB(18, 18, 18, 20),
            child: Column(
              children: [
                Row(
                  children: [
                    _StatusPill(snapshot: snapshot, tone: tone),
                    const Spacer(),
                    Swap(
                      id: connected,
                      alignment: AlignmentDirectional.centerEnd,
                      child: connected ? _Elapsed(since: app.connectedSince) : const SizedBox.shrink(),
                    ),
                  ],
                ),
                const SizedBox(height: 6),
                _Orb(app: app, snapshot: snapshot, tone: tone),
                Reveal(
                  child: detail == null
                      ? null
                      : Padding(
                          padding: const EdgeInsets.only(top: 4),
                          child: Text(detail, textAlign: TextAlign.center, style: t.phoneNote.copyWith(color: t.t2)),
                        ),
                ),
                Reveal(
                  child: node == null || phase == rust.UiPhase.off || names.isEmpty
                      ? null
                      : Padding(
                          padding: const EdgeInsets.only(top: 14),
                          child: RouteLine(
                            lines: [names[inUse < 0 ? 0 : inUse]],
                            current: inUse < 0 ? -1 : 0,
                            connected: connected,
                            compact: true,
                          ),
                        ),
                ),
              ],
            ),
          ),
        ],
      ),
    );
  }
}

/// The status capsule: the title in the status colour on its 10 % fill,
/// the dot breathing while connected, a spinner while busy.
class _StatusPill extends StatefulWidget {
  const _StatusPill({required this.snapshot, required this.tone});

  final rust.UiSnapshot snapshot;
  final Color tone;

  @override
  State<_StatusPill> createState() => _StatusPillState();
}

class _StatusPillState extends State<_StatusPill> with SingleTickerProviderStateMixin {
  late final AnimationController _breath = AnimationController(vsync: this, duration: Tokens.pulseHalf)
    ..repeat(reverse: true);
  late final Animation<double> _opacity = _breath.drive(
    Tween(begin: 1.0, end: Tokens.pulseLow).chain(CurveTween(curve: Curves.easeInOut)),
  );

  @override
  void dispose() {
    _breath.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final tone = widget.tone;
    final phase = widget.snapshot.phase;
    final busy = isBusy(phase);
    final dot = Container(
      width: 8,
      height: 8,
      decoration: BoxDecoration(
        color: tone,
        shape: BoxShape.circle,
        boxShadow: [BoxShadow(color: tone.withValues(alpha: 0.22), spreadRadius: 3)],
      ),
    );
    final breathing = phase == rust.UiPhase.connected && !MediaQuery.disableAnimationsOf(context);
    return Semantics(
      liveRegion: true,
      header: true,
      child: Container(
        height: 28,
        padding: const EdgeInsetsDirectional.fromSTEB(10, 0, 12, 0),
        decoration: BoxDecoration(color: tone.withValues(alpha: 0.10), borderRadius: BorderRadius.circular(14)),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (busy)
              Spinner(color: tone, size: 10)
            else if (breathing)
              FadeTransition(opacity: _opacity, child: dot)
            else
              dot,
            const SizedBox(width: 7),
            Text(statusTitle(widget.snapshot), style: t.phoneNote.copyWith(fontWeight: FontWeight.w600, color: tone)),
          ],
        ),
      ),
    );
  }
}

/// The session's time, mono 13 in t2 after a clock.
class _Elapsed extends StatefulWidget {
  const _Elapsed({required this.since});

  final DateTime? since;

  @override
  State<_Elapsed> createState() => _ElapsedState();
}

class _ElapsedState extends State<_Elapsed> {
  late final Timer _tick;

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

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
    return Semantics(
      label: '${S.current('thisSession')} $text',
      child: ExcludeSemantics(
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(FLucideIcons.clock, size: 13, color: t.t2),
            const SizedBox(width: 5),
            Text(text, style: t.monoText(13, color: t.t2)),
          ],
        ),
      ),
    );
  }
}

/// The main button: a 112 disc in a 168 ring of the status colour; green
/// while connected, the brand's blue otherwise, the power icon on it (a
/// spinner while busy, when it takes no press). The state is the Rust
/// layer's, with no optimistic flip (as the desktop switch).
class _Orb extends StatelessWidget {
  const _Orb({required this.app, required this.snapshot, required this.tone});

  final AppState app;
  final rust.UiSnapshot snapshot;
  final Color tone;

  static const _green = LinearGradient(
    begin: Alignment(-0.5, -0.87),
    end: Alignment(0.5, 0.87),
    colors: [Color(0xFF26A85A), Color(0xFF13763A)],
  );

  @override
  Widget build(BuildContext context) {
    final s = S.current;
    final busy = isBusy(snapshot.phase);
    final on = snapshot.phase == rust.UiPhase.connected;
    final glow = on ? const Color(0xFF15803D) : const Color(0xFF1F4390);
    final label = busy ? statusTitle(snapshot) : s(on ? 'disconnectBtn' : 'connectBtn');
    final face = Container(
      width: Tokens.phoneOrb,
      height: Tokens.phoneOrb,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        shape: BoxShape.circle,
        gradient: RadialGradient(
          stops: const [0, 0.58, 0.58, 0.70, 0.70],
          colors: [
            tone.withValues(alpha: 0.10),
            tone.withValues(alpha: 0.10),
            tone.withValues(alpha: 0.05),
            tone.withValues(alpha: 0.05),
            tone.withValues(alpha: 0),
          ],
        ),
      ),
      child: AnimatedContainer(
        duration: Tokens.motion(context, Tokens.motionBase),
        curve: Tokens.motionCurve,
        width: Tokens.phoneOrbDisc,
        height: Tokens.phoneOrbDisc,
        alignment: Alignment.center,
        decoration: BoxDecoration(
          shape: BoxShape.circle,
          gradient: on ? _green : Tokens.primaryGradient,
          boxShadow: [BoxShadow(color: glow.withValues(alpha: 0.35), offset: const Offset(0, 14), blurRadius: 30)],
        ),
        child: busy
            ? const Spinner(color: Color(0xFFFFFFFF), size: 34)
            : const Icon(FLucideIcons.power, size: 44, color: Color(0xFFFFFFFF)),
      ),
    );
    if (busy) return Semantics(button: true, enabled: false, label: label, child: ExcludeSemantics(child: face));
    return Pressable(
      key: const ValueKey('connect'),
      onPress: () => unawaited(on ? app.disconnect() : app.connect()),
      label: label,
      radius: Tokens.phoneOrb / 2,
      builder: (context, _) => face,
    );
  }
}

/// The current node (flag, 当前节点 over its name, latency; opens 节点) and
/// the routing mode (pushes its choices), 62 rows in one card.
class _NodeCard extends StatelessWidget {
  const _NodeCard({required this.app, required this.snapshot, required this.onNodes});

  final AppState app;
  final rust.UiSnapshot snapshot;
  final VoidCallback onNodes;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final node = app.nodeById(snapshot.selectedNodeId);
    final rules = snapshot.routingMode == rust.UiRoutingMode.rules;
    Widget row({
      required Key key,
      required String label,
      required Widget leading,
      required String title,
      required FontWeight weight,
      required VoidCallback onPress,
      Widget? end,
    }) => Pressable(
      key: key,
      radius: 0,
      onPress: onPress,
      label: '$label, $title',
      builder: (context, _) => Container(
        height: Tokens.phoneRowTwoLines,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        child: Row(
          children: [
            leading,
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(label, style: t.phoneNote.copyWith(fontSize: 12, height: 16 / 12)),
                  Text(
                    title,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.phoneRowTitle.copyWith(fontWeight: weight, letterSpacing: -0.17),
                  ),
                ],
              ),
            ),
            if (end != null) ...[const SizedBox(width: 8), end],
            const SizedBox(width: 6),
            Icon(FLucideIcons.chevronRight, size: 16, color: t.t3),
          ],
        ),
      ),
    );
    return PhoneCard(
      children: [
        row(
          key: const ValueKey('current-node'),
          label: s('currentNode'),
          leading: Flag(countryCode: node?.exitCountryCode, size: Tokens.phoneFlagSize),
          title: node == null ? '—' : nodeTitle(node),
          weight: FontWeight.w600,
          onPress: onNodes,
          end: node == null ? null : LatencyPill.ms(snapshot.latencyMs),
        ),
        row(
          key: const ValueKey('routing'),
          label: s('routing'),
          leading: const PhoneTile(icon: FLucideIcons.split, size: 28),
          title: rules ? s('rules') : s('global'),
          weight: FontWeight.w500,
          onPress: () => pushPhoneChoice<rust.UiRoutingMode>(
            context,
            title: s('routing'),
            options: [(rust.UiRoutingMode.rules, s('rules')), (rust.UiRoutingMode.global, s('global'))],
            value: snapshot.routingMode,
            onChanged: (mode) => unawaited(app.setRoutingMode(mode)),
          ),
        ),
      ],
    );
  }
}
