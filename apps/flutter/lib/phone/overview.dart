// The phone overview (#448 step 5): the connection card (radius 28: the
// status with the breathing capsule and the timer, the node with its flag
// and latency, the route, the main button), the problem card below it, the
// current node's row that opens 节点, live traffic. One connection, so no
// modes card; no local proxy card. The desktop's full-page gates and the
// invalid-profile warning as they are.

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
  const PhoneOverview({super.key, required this.app, required this.onNodes});

  final AppState app;

  /// The current node's row: the 节点 tab.
  final VoidCallback onNodes;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final limited = LimitedAccess.of(snapshot);
      final loading = snapshot.profile == rust.UiProfile.loading;
      return Swap(
        id: limited != null || loading ? snapshot.profile : null,
        expand: true,
        child: limited ?? (loading ? const LoadingNodes() : _page(context, snapshot)),
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
              ? Column(
                  crossAxisAlignment: CrossAxisAlignment.stretch,
                  children: [ConfigWarning(previous: snapshot.profilePrevious), const SizedBox(height: 12)],
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
        _CurrentNode(node: app.nodeById(snapshot.selectedNodeId), latencyMs: snapshot.latencyMs, onPress: onNodes),
        const SizedBox(height: 12),
        TrafficCard(app: app),
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
      padding: const EdgeInsets.all(20),
      decoration: t.phoneCardDecoration(radius: Tokens.phoneConnectionRadius),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Expanded(
                child: Semantics(
                  header: true,
                  liveRegion: true,
                  child: Text(statusTitle(snapshot), style: t.heroTitle),
                ),
              ),
              Swap(
                id: connected,
                alignment: AlignmentDirectional.centerEnd,
                child: connected ? _TimerCapsule(tone: tone, since: app.connectedSince) : const SizedBox.shrink(),
              ),
            ],
          ),
          const SizedBox(height: 8),
          // Off it shows the node too (the desktop's line about the local
          // proxy does not apply: a phone has none).
          Swap(
            id: detail ?? node?.id,
            child: detail != null
                ? Text(detail, style: t.phoneNote.copyWith(color: t.t2))
                : node == null
                ? const SizedBox.shrink()
                : Row(
                    children: [
                      Flag(countryCode: node.exitCountryCode, size: Tokens.phoneFlagSize),
                      const SizedBox(width: 10),
                      Flexible(
                        child: Text(nodeTitle(node), maxLines: 1, overflow: TextOverflow.ellipsis, style: t.phoneBodyStrong),
                      ),
                      const SizedBox(width: 8),
                      LatencyPill.ms(snapshot.latencyMs),
                    ],
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
                    ),
                  ),
          ),
          const SizedBox(height: 18),
          _MainButton(app: app, snapshot: snapshot),
        ],
      ),
    );
  }
}

/// The capsule while connected: the breathing dot and the session's time
/// (mono), in the status colour on its 10 % fill.
class _TimerCapsule extends StatefulWidget {
  const _TimerCapsule({required this.tone, required this.since});

  final Color tone;
  final DateTime? since;

  @override
  State<_TimerCapsule> createState() => _TimerCapsuleState();
}

class _TimerCapsuleState extends State<_TimerCapsule> with SingleTickerProviderStateMixin {
  late final Timer _tick;
  late final AnimationController _breath = AnimationController(vsync: this, duration: Tokens.pulseHalf)
    ..repeat(reverse: true);
  late final Animation<double> _opacity = _breath.drive(
    Tween(begin: 1.0, end: Tokens.pulseLow).chain(CurveTween(curve: Curves.easeInOut)),
  );

  @override
  void initState() {
    super.initState();
    _tick = Timer.periodic(const Duration(seconds: 1), (_) => setState(() {}));
  }

  @override
  void dispose() {
    _tick.cancel();
    _breath.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final since = widget.since;
    final elapsed = since == null ? Duration.zero : DateTime.now().difference(since);
    String two(int n) => n.toString().padLeft(2, '0');
    final text = '${two(elapsed.inHours)}:${two(elapsed.inMinutes % 60)}:${two(elapsed.inSeconds % 60)}';
    final dot = Container(
      width: Tokens.pulseDotSize,
      height: Tokens.pulseDotSize,
      decoration: BoxDecoration(color: widget.tone, shape: BoxShape.circle),
    );
    return Semantics(
      label: '${S.current('thisSession')} $text',
      child: ExcludeSemantics(
        child: Container(
          height: 28,
          padding: const EdgeInsets.symmetric(horizontal: 10),
          decoration: BoxDecoration(
            color: widget.tone.withValues(alpha: 0.10),
            borderRadius: BorderRadius.circular(14),
          ),
          child: Row(
            mainAxisSize: MainAxisSize.min,
            children: [
              if (MediaQuery.disableAnimationsOf(context)) dot else FadeTransition(opacity: _opacity, child: dot),
              const SizedBox(width: 7),
              Text(text, style: t.monoText(13, weight: FontWeight.w500, color: widget.tone)),
            ],
          ),
        ),
      ),
    );
  }
}

/// 「连接」 or 「断开连接」, 52 high. Connecting or disconnecting it shows the
/// spinner with the status and takes no press; the state is the Rust
/// layer's, with no optimistic flip (as the desktop switch).
class _MainButton extends StatelessWidget {
  const _MainButton({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final busy = isBusy(snapshot.phase);
    final on = snapshot.phase == rust.UiPhase.connected;
    final primary = !busy && !on;
    final light = t.brightness == Brightness.light;
    final label = busy ? statusTitle(snapshot) : s(on ? 'disconnectBtn' : 'connectBtn');
    final face = AnimatedContainer(
      duration: Tokens.motion(context, Tokens.motionBase),
      curve: Tokens.motionCurve,
      height: Tokens.phoneMainButton,
      alignment: Alignment.center,
      decoration: BoxDecoration(
        borderRadius: BorderRadius.circular(Tokens.phoneControlRadius),
        gradient: primary && light ? Tokens.primaryGradient : null,
        color: primary ? (light ? null : t.accent) : (busy ? t.accentSoft : t.card),
        border: primary || busy ? null : Border.all(color: t.line),
      ),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          if (busy) ...[Spinner(color: t.accent, size: 16), const SizedBox(width: 10)],
          Text(label, style: t.phoneBodyStrong.copyWith(color: primary ? t.onAccent : (busy ? t.accent : t.t1))),
        ],
      ),
    );
    if (busy) return Semantics(button: true, enabled: false, label: label, child: ExcludeSemantics(child: face));
    return Pressable(
      key: const ValueKey('connect'),
      onPress: () => unawaited(on ? app.disconnect() : app.connect()),
      label: label,
      radius: Tokens.phoneControlRadius,
      builder: (context, _) => face,
    );
  }
}

/// 当前节点: flag, name and latency in a 62 row; opens 节点.
class _CurrentNode extends StatelessWidget {
  const _CurrentNode({required this.node, required this.latencyMs, required this.onPress});

  final rust.UiNode? node;
  final int? latencyMs;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final node = this.node;
    return Pressable(
      key: const ValueKey('current-node'),
      onPress: onPress,
      label: node == null ? s('currentNode') : '${s('currentNode')}, ${nodeTitle(node)}',
      radius: Tokens.phoneCardRadius,
      builder: (context, _) => Container(
        height: Tokens.phoneRowTwoLines,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        decoration: t.phoneCardDecoration(),
        child: Row(
          children: [
            Flag(countryCode: node?.exitCountryCode, size: Tokens.phoneFlagSize),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(s('currentNode'), style: t.phoneNote),
                  Text(
                    node == null ? '—' : nodeTitle(node),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.phoneBodyStrong,
                  ),
                ],
              ),
            ),
            if (node != null) ...[LatencyPill.ms(latencyMs), const SizedBox(width: 6)],
            Icon(FLucideIcons.chevronRight, size: 18, color: t.t3),
          ],
        ),
      ),
    );
  }
}
