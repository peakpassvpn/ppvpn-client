// Small pieces the pages share: the three-state switch, the copy button,
// the spinner and the latency label.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import 'app_state.dart';
import 'strings.dart';
import 'theme.dart';

enum SwitchState { off, busy, on }

/// The connection switch (handoff "开关三态"): off, on, and in between
/// (thumb centred with a spinner in it, soft accent track, 1px accent
/// outline) while something is being set up. No status text beside it.
class TriSwitch extends StatelessWidget {
  const TriSwitch({super.key, required this.state, required this.onChanged, required this.label, this.enabled = true});

  final SwitchState state;
  final ValueChanged<bool>? onChanged;
  final String label;
  final bool enabled;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final size = Tokens.switchSize;
    final thumb = size.height - 4;
    final on = state == SwitchState.on;
    final busy = state == SwitchState.busy;
    final track = on
        ? t.accent
        : busy
        ? t.accentSoft
        : t.border.withValues(alpha: t.brightness == Brightness.light ? 0.16 : 0.3);
    final left = switch (state) {
      SwitchState.off => 2.0,
      SwitchState.busy => (size.width - thumb) / 2,
      SwitchState.on => size.width - thumb - 2,
    };
    final active = enabled && onChanged != null;
    return Semantics(
      label: label,
      toggled: on,
      enabled: active,
      child: GestureDetector(
        onTap: active ? () => onChanged!(!on && !busy) : null,
        child: MouseRegion(
          cursor: active ? SystemMouseCursors.click : SystemMouseCursors.basic,
          child: Opacity(
            opacity: enabled ? 1 : 0.5,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 150),
              width: size.width,
              height: size.height,
              decoration: BoxDecoration(
                color: track,
                borderRadius: BorderRadius.circular(size.height / 2),
                border: busy ? Border.all(color: t.accent, width: 1) : null,
              ),
              child: Stack(
                children: [
                  AnimatedPositioned(
                    duration: const Duration(milliseconds: 150),
                    left: left,
                    top: (size.height - thumb) / 2 - (busy ? 1 : 0),
                    child: Container(
                      width: thumb,
                      height: thumb,
                      decoration: BoxDecoration(
                        color: const Color(0xFFFFFFFF),
                        shape: BoxShape.circle,
                        boxShadow: const [BoxShadow(color: Color(0x33000000), blurRadius: 1, offset: Offset(0, 0.5))],
                      ),
                      child: busy ? Padding(padding: const EdgeInsets.all(2), child: Spinner(color: t.accent, size: thumb - 4)) : null,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A rotating arc, 0.9 s a turn, linear (handoff "动效").
class Spinner extends StatefulWidget {
  const Spinner({super.key, required this.color, this.size = 14});

  final Color color;
  final double size;

  @override
  State<Spinner> createState() => _SpinnerState();
}

class _SpinnerState extends State<Spinner> with SingleTickerProviderStateMixin {
  late final AnimationController _turn = AnimationController(vsync: this, duration: const Duration(milliseconds: 900))..repeat();

  @override
  void dispose() {
    _turn.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => RotationTransition(
    turns: _turn,
    child: CustomPaint(size: Size.square(widget.size), painter: _ArcPainter(widget.color)),
  );
}

class _ArcPainter extends CustomPainter {
  _ArcPainter(this.color);
  final Color color;

  @override
  void paint(Canvas canvas, Size size) {
    final stroke = math.max(1.5, size.width / 8);
    final paint = Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = stroke
      ..strokeCap = StrokeCap.round;
    canvas.drawArc((Offset.zero & size).deflate(stroke / 2), 0, math.pi * 1.5, false, paint);
  }

  @override
  bool shouldRepaint(_ArcPainter old) => old.color != color;
}

/// Copies [text]; the label turns into "✓ 已复制" for 1.6 s.
class CopyButton extends StatefulWidget {
  const CopyButton({super.key, required this.text, this.label});

  final String text;
  final String? label;

  @override
  State<CopyButton> createState() => _CopyButtonState();
}

class _CopyButtonState extends State<CopyButton> {
  Timer? _reset;

  @override
  void dispose() {
    _reset?.cancel();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final copied = _reset?.isActive ?? false;
    return SizedBox(
      height: Tokens.controlHeight,
      child: FButton(
        variant: FButtonVariant.outline,
        size: FButtonSizeVariant.sm,
        onPress: () async {
          await Clipboard.setData(ClipboardData(text: widget.text));
          _reset?.cancel();
          setState(() => _reset = Timer(const Duration(milliseconds: 1600), () => setState(() {})));
        },
        child: Text(copied ? '✓ ${S.current('copied')}' : widget.label ?? S.current('copy')),
      ),
    );
  }
}

/// Latency as the nodes page colours it: < 100 ms success, 100–199 warning,
/// ≥ 200 danger; timeout in the secondary colour, failure in danger; a
/// spinner only while testing.
class LatencyLabel extends StatelessWidget {
  const LatencyLabel({super.key, required this.latency});

  final Latency? latency;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return switch (latency) {
      null => Text('—', style: t.note),
      LatencyTesting() => Spinner(color: t.accent, size: 12),
      LatencyMs(:final ms) => Text(
        '$ms ms',
        style: t.mono.copyWith(color: ms < 100 ? t.success : (ms < 200 ? t.warning : t.danger)),
      ),
      LatencyTimeout() => Text(S.current('timeout'), style: t.secondary),
      LatencyFailed() => Text(S.current('failed'), style: t.secondary.copyWith(color: t.danger)),
    };
  }
}

/// Bytes per second, as the traffic tiles show it.
String formatRate(int bytesPerSecond) {
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  var value = bytesPerSecond.toDouble();
  var unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  final digits = unit == 0 || value >= 100 ? 0 : 1;
  return '${value.toStringAsFixed(digits)} ${units[unit]}';
}
