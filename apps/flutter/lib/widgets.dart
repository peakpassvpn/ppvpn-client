// Pieces the pages share, drawn to the signed-off design: the pressable
// base (keyboard, focus, screen readers), buttons, the three-state switch,
// the spinner, the latency pill, the copy button, flags, the brand mark and
// the overview's shield. Colours and sizes come from theme.dart.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';

import 'app_state.dart';
import 'src/rust/api.dart' as rust;
import 'strings.dart';
import 'theme.dart';

/// Tap, Enter and Space activate it; it is a button to screen readers and
/// shows a focus ring when reached by keyboard. [builder] gets the hover
/// state.
class Pressable extends StatefulWidget {
  const Pressable({
    super.key,
    required this.onPress,
    required this.builder,
    this.label,
    this.selected,
    this.onDoublePress,
    this.radius = Tokens.controlRadius,
  });

  final VoidCallback? onPress;
  final VoidCallback? onDoublePress;
  final Widget Function(BuildContext context, bool hovered) builder;
  final String? label;
  final bool? selected;
  final double radius;

  @override
  State<Pressable> createState() => _PressableState();
}

class _PressableState extends State<Pressable> {
  bool _hovered = false;
  bool _focused = false;

  @override
  Widget build(BuildContext context) {
    final enabled = widget.onPress != null;
    final t = Tokens.of(context);
    return Semantics(
      button: true,
      enabled: enabled,
      selected: widget.selected,
      label: widget.label,
      child: FocusableActionDetector(
        enabled: enabled,
        mouseCursor: enabled ? SystemMouseCursors.click : SystemMouseCursors.basic,
        onShowHoverHighlight: (value) => setState(() => _hovered = value),
        onShowFocusHighlight: (value) => setState(() => _focused = value),
        actions: {ActivateIntent: CallbackAction<ActivateIntent>(onInvoke: (_) => widget.onPress?.call())},
        child: GestureDetector(
          onTap: widget.onPress,
          onDoubleTap: widget.onDoublePress,
          child: DecoratedBox(
            position: DecorationPosition.foreground,
            decoration: BoxDecoration(
              borderRadius: BorderRadius.circular(widget.radius),
              border: _focused ? Border.all(color: t.accent, width: 2) : null,
            ),
            child: Opacity(opacity: enabled ? 1 : 0.5, child: widget.builder(context, _hovered && enabled)),
          ),
        ),
      ),
    );
  }
}

enum ButtonKind {
  /// The gradient primary button.
  primary,

  /// Card fill with a line outline.
  secondary,

  /// Accent text on accent-soft.
  soft,

  /// Accent text only; accent-soft on hover.
  text,

  /// An icon, t2; a faint fill on hover.
  icon,
}

/// The design's buttons: 36 (primary) or 32 high, radius 8.
class Button extends StatelessWidget {
  const Button({
    super.key,
    required this.kind,
    required this.onPress,
    this.label,
    this.icon,
    this.height,
    this.semanticsLabel,
    this.fontSize = 13,
  });

  final ButtonKind kind;
  final VoidCallback? onPress;
  final String? label;
  final IconData? icon;
  final double? height;
  final String? semanticsLabel;
  final double fontSize;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final h = height ?? (kind == ButtonKind.primary ? Tokens.primaryHeight : Tokens.controlHeight);
    final fg = switch (kind) {
      ButtonKind.primary => t.onAccent,
      ButtonKind.secondary => t.t1,
      ButtonKind.soft || ButtonKind.text => t.accent,
      ButtonKind.icon => t.t2,
    };
    return Pressable(
      onPress: onPress,
      label: semanticsLabel ?? label,
      builder: (context, hovered) => Container(
        height: h,
        width: kind == ButtonKind.icon ? h : null,
        padding: kind == ButtonKind.icon ? null : EdgeInsets.symmetric(horizontal: h >= 32 ? 12 : 8),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          borderRadius: BorderRadius.circular(h <= 26 ? 6 : Tokens.controlRadius),
          gradient: kind == ButtonKind.primary && t.brightness == Brightness.light ? Tokens.primaryGradient : null,
          color: switch (kind) {
            ButtonKind.primary => t.brightness == Brightness.light ? null : t.accent,
            ButtonKind.secondary => t.card,
            ButtonKind.soft => t.accentSoft,
            ButtonKind.text || ButtonKind.icon => hovered ? t.accentSoft : null,
          },
          border: kind == ButtonKind.secondary ? Border.all(color: t.line) : null,
          boxShadow: switch (kind) {
            ButtonKind.primary => const [BoxShadow(color: Color(0x591F4390), offset: Offset(0, 1), blurRadius: 2)],
            ButtonKind.secondary => const [BoxShadow(color: Color(0x0A0F172A), offset: Offset(0, 1), blurRadius: 2)],
            _ => null,
          },
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [
            if (icon != null) Icon(icon, size: kind == ButtonKind.icon ? 18 : 14, color: fg),
            if (icon != null && label != null) const SizedBox(width: 6),
            if (label != null)
              Text(
                label!,
                maxLines: 1,
                softWrap: false,
                style: t.control.copyWith(fontSize: fontSize, color: fg),
              ),
          ],
        ),
      ),
    );
  }
}

enum SwitchState { off, busy, on }

/// The connection switch, 36×20: off, on, and in between while connecting
/// or disconnecting (a spinner on a soft accent track with an accent ring;
/// not pressable then). The state is the Rust layer's: no optimistic flip.
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
    final on = state == SwitchState.on;
    final busy = state == SwitchState.busy;
    final active = enabled && !busy && onChanged != null;
    return Semantics(
      label: label,
      toggled: on,
      value: busy ? S.current('st_connecting') : null,
      enabled: active,
      child: FocusableActionDetector(
        enabled: active,
        mouseCursor: active ? SystemMouseCursors.click : SystemMouseCursors.basic,
        actions: {ActivateIntent: CallbackAction<ActivateIntent>(onInvoke: (_) => onChanged?.call(!on))},
        child: GestureDetector(
          onTap: active ? () => onChanged!(!on) : null,
          child: Opacity(
            opacity: enabled ? 1 : 0.5,
            child: AnimatedContainer(
              duration: const Duration(milliseconds: 150),
              width: size.width,
              height: size.height,
              decoration: BoxDecoration(
                color: on ? t.accent : (busy ? t.accentSoft : t.switchOff),
                borderRadius: BorderRadius.circular(size.height / 2),
                border: busy ? Border.all(color: t.accent) : null,
              ),
              child: busy
                  ? Center(child: Spinner(color: t.accent, size: 10))
                  : AnimatedAlign(
                      duration: const Duration(milliseconds: 150),
                      alignment: on ? Alignment.centerRight : Alignment.centerLeft,
                      child: Container(
                        width: 16,
                        height: 16,
                        margin: const EdgeInsets.all(2),
                        decoration: BoxDecoration(
                          color: on ? const Color(0xFFFFFFFF) : t.thumbOff,
                          shape: BoxShape.circle,
                          boxShadow: const [BoxShadow(color: Color(0x400F172A), offset: Offset(0, 1), blurRadius: 3)],
                        ),
                      ),
                    ),
            ),
          ),
        ),
      ),
    );
  }
}

/// A rotating three-quarter ring, 0.9 s a turn, linear.
class Spinner extends StatefulWidget {
  const Spinner({super.key, required this.color, this.size = 14, this.track});

  final Color color;
  final double size;

  /// The rest of the ring, when drawn (the sign-in wait ring).
  final Color? track;

  @override
  State<Spinner> createState() => _SpinnerState();
}

class _SpinnerState extends State<Spinner> with SingleTickerProviderStateMixin {
  late final AnimationController _turn = AnimationController(vsync: this, duration: const Duration(milliseconds: 900))
    ..repeat();

  @override
  void dispose() {
    _turn.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) => RotationTransition(
    turns: _turn,
    child: CustomPaint(
      size: Size.square(widget.size),
      painter: _ArcPainter(widget.color, widget.track),
    ),
  );
}

class _ArcPainter extends CustomPainter {
  _ArcPainter(this.color, this.track);
  final Color color;
  final Color? track;

  @override
  void paint(Canvas canvas, Size size) {
    final stroke = 2.0;
    final rect = (Offset.zero & size).deflate(stroke / 2);
    Paint paint(Color color) => Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = stroke
      ..strokeCap = StrokeCap.round;
    if (track != null) canvas.drawArc(rect, 0, math.pi * 2, false, paint(track!));
    canvas.drawArc(rect, -math.pi / 2, track != null ? math.pi / 2 : math.pi * 1.5, false, paint(color));
  }

  @override
  bool shouldRepaint(_ArcPainter old) => old.color != color || old.track != track;
}

/// Copies [text]; the label turns into 「✓ 已复制」 for 1.6 s.
class CopyButton extends StatefulWidget {
  const CopyButton({super.key, required this.text, this.label, this.kind = ButtonKind.text, this.height = 22});

  final String text;
  final String? label;
  final ButtonKind kind;
  final double height;

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
    return Button(
      kind: widget.kind,
      height: widget.height,
      fontSize: 12,
      label: copied ? '✓ ${S.current('copied')}' : widget.label ?? S.current('copy'),
      onPress: () async {
        await Clipboard.setData(ClipboardData(text: widget.text));
        _reset?.cancel();
        setState(() => _reset = Timer(const Duration(milliseconds: 1600), () => setState(() {})));
      },
    );
  }
}

/// Latency as a pill (22 high, mono 12): < 100 ms ok, 100–199 warn, ≥ 200
/// bad; timeout and failure dim; a spinner while testing.
class LatencyPill extends StatelessWidget {
  const LatencyPill({super.key, required this.latency});

  final Latency? latency;

  static LatencyPill ms(int? ms) => LatencyPill(latency: ms == null ? null : LatencyMs(ms));

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final (String text, Color fg, Color bg) = switch (latency) {
      null => ('—', t.t3, t.sunk),
      LatencyTesting() => ('', t.accent, t.accentSoft),
      LatencyMs(:final ms) when ms < 100 => ('$ms ms', t.ok, t.okSoft),
      LatencyMs(:final ms) when ms < 200 => ('$ms ms', t.warn, t.warnSoft),
      LatencyMs(:final ms) => ('$ms ms', t.bad, t.badSoft),
      LatencyTimeout() => (S.current('timeout'), t.t3, t.sunk),
      LatencyFailed() => (S.current('failed'), t.bad, t.badSoft),
    };
    return Container(
      height: 22,
      padding: const EdgeInsets.symmetric(horizontal: 8),
      decoration: BoxDecoration(color: bg, borderRadius: BorderRadius.circular(11)),
      child: Row(
        mainAxisSize: MainAxisSize.min,
        children: latency is LatencyTesting
            ? [Spinner(color: fg, size: 10)]
            : [
                Container(width: 6, height: 6, decoration: BoxDecoration(color: fg, shape: BoxShape.circle)),
                const SizedBox(width: 5),
                Text(text, style: t.monoText(12, weight: FontWeight.w500, color: fg)),
              ],
      ),
    );
  }
}

/// A node's name without the flag emoji the backend puts in front.
String nodeTitle(rust.UiNode node) {
  final runes = node.name.runes.toList();
  var start = 0;
  while (start < runes.length && runes[start] >= 0x1F1E6 && runes[start] <= 0x1F1FF) {
    start++;
  }
  return String.fromCharCodes(runes.skip(start)).trim();
}

/// The exit country's flag, 20×15 with radius 3 and a thin outline. The
/// prototype draws the emoji flag of the country code (no bundled images
/// yet; Windows draws letters for it).
class Flag extends StatelessWidget {
  const Flag({super.key, required this.countryCode});

  final String? countryCode;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final code = (countryCode ?? '').toUpperCase();
    final emoji = code.length == 2 && RegExp(r'^[A-Z]{2}$').hasMatch(code)
        ? String.fromCharCodes(code.codeUnits.map((c) => 0x1F1E6 + c - 0x41))
        : null;
    return ExcludeSemantics(
      child: Container(
        width: Tokens.flagSize.width,
        height: Tokens.flagSize.height,
        clipBehavior: Clip.antiAlias,
        decoration: BoxDecoration(
          color: t.sunk,
          borderRadius: BorderRadius.circular(3),
          border: Border.all(color: t.hair, width: 0.5),
        ),
        child: emoji == null ? null : FittedBox(fit: BoxFit.cover, child: Text(emoji)),
      ),
    );
  }
}

/// The brand mark: a rounded tile with the mountain (DesktopLogin and the
/// sidebar). [tile] is the tile's fill, [mountain] and [snow] the shapes'.
class BrandMark extends StatelessWidget {
  const BrandMark({super.key, required this.size, this.light = false});

  final double size;

  /// White tile with a blue mountain (on the brand gradient); otherwise the
  /// blue gradient tile with a white mountain.
  final bool light;

  @override
  Widget build(BuildContext context) => CustomPaint(size: Size.square(size), painter: _BrandMarkPainter(light));
}

class _BrandMarkPainter extends CustomPainter {
  _BrandMarkPainter(this.light);
  final bool light;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 24);
    final tile = RRect.fromRectAndRadius(const Rect.fromLTWH(0, 0, 24, 24), const Radius.circular(6.5));
    final fill = Paint();
    if (light) {
      fill.color = const Color(0xFFFFFFFF);
    } else {
      fill.shader = const LinearGradient(
        begin: Alignment.topLeft,
        end: Alignment.bottomRight,
        colors: [Color(0xFF3D63C9), Color(0xFF1F4390)],
      ).createShader(const Rect.fromLTWH(0, 0, 24, 24));
    }
    canvas.drawRRect(tile, fill);
    // M3.5 17.5 9.2 9l3.3 4.8 2.1-2.9 5.9 6.6z
    final mountain = Path()
      ..moveTo(3.5, 17.5)
      ..lineTo(9.2, 9)
      ..lineTo(12.5, 13.8)
      ..lineTo(14.6, 10.9)
      ..lineTo(20.5, 17.5)
      ..close();
    // M9.2 9l1.4 2.1-1.4.8-1.3-1z
    final snow = Path()
      ..moveTo(9.2, 9)
      ..lineTo(10.6, 11.1)
      ..lineTo(9.2, 11.9)
      ..lineTo(7.9, 10.9)
      ..close();
    canvas.drawPath(mountain, Paint()..color = light ? const Color(0xFF1F4390) : const Color(0xFFFFFFFF));
    canvas.drawPath(snow, Paint()..color = light ? const Color(0xFF8FA8F5) : const Color(0xFFC9D6FF));
  }

  @override
  bool shouldRepaint(_BrandMarkPainter old) => old.light != light;
}

/// The overview's icon: the shield `M12 22s8-4 8-10V5l-8-3-8 3v7c0 6 8 10
/// 8 10z` with the check `m8.5 12 2.5 2.5 4.5-5`, stroke 1.9. Outlined, or
/// [filled] (selected) with the check in white.
class ShieldCheckIcon extends StatelessWidget {
  const ShieldCheckIcon({super.key, required this.size, required this.color, this.filled = false, this.stroke = 1.9});

  final double size;
  final Color color;
  final bool filled;
  final double stroke;

  @override
  Widget build(BuildContext context) => CustomPaint(
    size: Size.square(size),
    painter: _ShieldCheckPainter(color: color, filled: filled, stroke: stroke),
  );
}

class _ShieldCheckPainter extends CustomPainter {
  _ShieldCheckPainter({required this.color, required this.filled, required this.stroke});

  final Color color;
  final bool filled;
  final double stroke;

  @override
  void paint(Canvas canvas, Size size) {
    canvas.scale(size.width / 24);
    final shield = Path()
      ..moveTo(12, 22)
      ..cubicTo(12, 22, 20, 18, 20, 12)
      ..lineTo(20, 5)
      ..lineTo(12, 2)
      ..lineTo(4, 5)
      ..lineTo(4, 12)
      ..cubicTo(4, 18, 12, 22, 12, 22)
      ..close();
    final check = Path()
      ..moveTo(8.5, 12)
      ..lineTo(11, 14.5)
      ..lineTo(15.5, 9.5);
    Paint line(Color color) => Paint()
      ..color = color
      ..style = PaintingStyle.stroke
      ..strokeWidth = stroke
      ..strokeCap = StrokeCap.round
      ..strokeJoin = StrokeJoin.round;
    if (filled) canvas.drawPath(shield, Paint()..color = color);
    canvas.drawPath(shield, line(color));
    canvas.drawPath(check, line(filled ? const Color(0xFFFFFFFF) : color));
  }

  @override
  bool shouldRepaint(_ShieldCheckPainter old) =>
      old.color != color || old.filled != filled || old.stroke != stroke;
}

/// Bytes per second, split into number and unit for the rate tiles.
(String, String) formatRate(int bytesPerSecond) {
  const units = ['B/s', 'KB/s', 'MB/s', 'GB/s'];
  var value = bytesPerSecond.toDouble();
  var unit = 0;
  while (value >= 1000 && unit < units.length - 1) {
    value /= 1024;
    unit++;
  }
  final digits = unit == 0 || value >= 100 ? 0 : 1;
  return (value.toStringAsFixed(digits), units[unit]);
}
