// Pieces the pages share, drawn to the signed-off design: the pressable
// base (keyboard, focus, screen readers), buttons, the segmented control,
// the card row, the text field, the three-state switch, the spinner, the
// latency pill, the copy button, flags, the brand mark, the overview's
// shield and the install dialog. Colours and sizes come from theme.dart.

import 'dart:async';
import 'dart:math' as math;

import 'package:flutter/gestures.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:flutter_svg/flutter_svg.dart';
import 'package:forui/forui.dart' show FDialog, showFDialog;

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
        // As wide as its label, centred in its height.
        child: Center(
          widthFactor: 1,
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
      ),
    );
  }
}

/// A segmented control (26-high segments on a sunk track): one of
/// [options], announced as a group named [label] with each segment a
/// selectable button.
class Segmented<T> extends StatelessWidget {
  const Segmented({
    super.key,
    required this.label,
    required this.options,
    required this.value,
    required this.onChanged,
  });

  final String label;
  final List<(T, String)> options;
  final T value;
  final ValueChanged<T>? onChanged;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    Widget segment(T option, String text) {
      final on = option == value;
      return Pressable(
        onPress: onChanged == null ? null : () => onChanged!(option),
        selected: on,
        label: text,
        radius: 6,
        builder: (context, hovered) => Container(
          height: Tokens.segmentHeight,
          padding: const EdgeInsets.symmetric(horizontal: 11),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: on ? t.card : null,
            borderRadius: BorderRadius.circular(6),
            boxShadow: on ? Tokens.segmentShadow : null,
          ),
          child: Text(
            text,
            style: t.note.copyWith(fontWeight: on ? FontWeight.w600 : FontWeight.w500, color: on ? t.t1 : t.t2),
          ),
        ),
      );
    }

    return Semantics(
      label: label,
      container: true,
      child: Container(
        padding: const EdgeInsets.all(2),
        decoration: BoxDecoration(
          color: t.sunk,
          borderRadius: BorderRadius.circular(Tokens.controlRadius),
          border: Border.all(color: t.hair),
        ),
        child: Row(
          mainAxisSize: MainAxisSize.min,
          children: [for (final (option, text) in options) segment(option, text)],
        ),
      ),
    );
  }
}

/// A 58-high row of a card: the icon tile, title and a one-line
/// description, the control at its end; [divider] draws the hair above.
class SettingRow extends StatelessWidget {
  const SettingRow({
    super.key,
    required this.icon,
    required this.title,
    this.description,
    required this.trailing,
    this.divider = false,
  });

  final IconData icon;
  final String title;
  final String? description;
  final Widget trailing;
  final bool divider;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      height: Tokens.rowHeight,
      padding: const EdgeInsets.symmetric(horizontal: 20),
      decoration: BoxDecoration(border: divider ? Border(top: BorderSide(color: t.hair)) : null),
      child: Row(
        children: [
          RowIcon(icon: icon),
          const SizedBox(width: 14),
          Expanded(
            child: Column(
              mainAxisAlignment: MainAxisAlignment.center,
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title, style: t.rowTitle),
                if (description case final text?)
                  Padding(
                    padding: const EdgeInsets.only(top: 1),
                    child: Text(text, style: t.note, maxLines: 1, overflow: TextOverflow.ellipsis),
                  ),
              ],
            ),
          ),
          trailing,
        ],
      ),
    );
  }
}

/// A row's 32 icon tile: the accent icon on accent-soft.
class RowIcon extends StatelessWidget {
  const RowIcon({super.key, required this.icon});

  final IconData icon;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      width: Tokens.rowIconSize,
      height: Tokens.rowIconSize,
      decoration: BoxDecoration(color: t.accentSoft, borderRadius: BorderRadius.circular(Tokens.controlRadius)),
      child: Icon(icon, size: 17, color: t.accent),
    );
  }
}

/// A one-line text field (32 high, mono): the placeholder while empty, the
/// accent outline while focused. Click, drag and double-click select (the
/// platform's text gestures); [onSubmit] runs on Enter and when focus
/// leaves. Announced as a text field named [label].
class TextInput extends StatefulWidget {
  const TextInput({
    super.key,
    required this.label,
    required this.initial,
    required this.onSubmit,
    this.placeholder,
    this.invalid = false,
  });

  final String label;
  final String initial;
  final String? placeholder;
  final ValueChanged<String> onSubmit;

  /// Outlined in the error colour.
  final bool invalid;

  @override
  State<TextInput> createState() => _TextInputState();
}

class _TextInputState extends State<TextInput> implements TextSelectionGestureDetectorBuilderDelegate {
  late final TextEditingController _controller = TextEditingController(text: widget.initial);
  final FocusNode _focus = FocusNode();
  late final _gestures = _FieldGestures(this);

  @override
  final GlobalKey<EditableTextState> editableTextKey = GlobalKey<EditableTextState>();

  @override
  bool get forcePressEnabled => false;

  @override
  bool get selectionEnabled => true;

  @override
  void initState() {
    super.initState();
    _focus.addListener(_onFocus);
  }

  void _onFocus() {
    setState(() {});
    if (!_focus.hasFocus) widget.onSubmit(_controller.text);
  }

  @override
  void dispose() {
    _focus.removeListener(_onFocus);
    _focus.dispose();
    _controller.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final style = t.monoText(13, color: t.t1, height: 18);
    final border = widget.invalid ? t.bad : (_focus.hasFocus ? t.accent : t.line);
    return MergeSemantics(
      child: Semantics(
        label: widget.label,
        child: Container(
          height: Tokens.controlHeight,
          padding: Tokens.fieldPadding,
          alignment: Alignment.centerLeft,
          decoration: BoxDecoration(
            color: t.brightness == Brightness.light ? t.card : t.sunk,
            borderRadius: BorderRadius.circular(Tokens.controlRadius),
            border: Border.all(color: border),
          ),
          child: MouseRegion(
            cursor: SystemMouseCursors.text,
            child: _gestures.buildGestureDetector(
              behavior: HitTestBehavior.translucent,
              child: Stack(
                alignment: Alignment.centerLeft,
                children: [
                  if (widget.placeholder case final hint?)
                    ValueListenableBuilder(
                      valueListenable: _controller,
                      builder: (context, value, _) => value.text.isEmpty
                          ? ExcludeSemantics(child: Text(hint, maxLines: 1, style: style.copyWith(color: t.t3)))
                          : const SizedBox(),
                    ),
                  EditableText(
                    key: editableTextKey,
                    controller: _controller,
                    focusNode: _focus,
                    style: style,
                    cursorColor: t.accent,
                    backgroundCursorColor: t.t3,
                    selectionColor: t.accentSoft,
                    maxLines: 1,
                    autocorrect: false,
                    enableSuggestions: false,
                    keyboardType: TextInputType.url,
                    textInputAction: TextInputAction.done,
                    rendererIgnoresPointer: true,
                    onSubmitted: widget.onSubmit,
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

/// The platform's text gestures; a click also focuses the field.
class _FieldGestures extends TextSelectionGestureDetectorBuilder {
  _FieldGestures(this._state) : super(delegate: _state);

  final _TextInputState _state;

  @override
  void onSingleTapUp(TapDragUpDetails details) {
    super.onSingleTapUp(details);
    _state._focus.requestFocus();
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

/// A node's lines in failover order, by name: the label, else 线路 N by
/// place (keys are internal and never shown), as in the native apps.
List<String> lineNames(rust.UiNode node) => [
  for (final (i, line) in node.lines.indexed) line.label ?? S.current('routeN', {'n': '${i + 1}'}),
];

/// The lines as one text, "HKG-A → 线路 2"; a single line without a label
/// shows nothing.
String linesText(rust.UiNode node) =>
    node.lines.length == 1 ? (node.lines.single.label ?? '') : lineNames(node).join(' → ');

/// Flags bundled under assets/flags (flag-icons, 4:3).
const _flags = {
  'ae', 'ar', 'at', 'au', 'be', 'ca', 'ch', 'cn', 'de', 'dk', 'fi', 'fr', 'gb', 'hk',
  'id', 'ie', 'il', 'in', 'it', 'jp', 'kr', 'mo', 'my', 'nl', 'no', 'nz', 'ph', 'pl',
  'ru', 'se', 'sg', 'th', 'tr', 'tw', 'us', 'vn', 'za',
};

/// The exit country's flag: a 4:3 image, 20×15 with radius 3 and a 0.5 px
/// inner outline (no emoji flags: platforms draw them differently, Windows
/// as letters). A country without a bundled flag gets its code on a tile.
class Flag extends StatelessWidget {
  const Flag({super.key, required this.countryCode});

  final String? countryCode;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final code = (countryCode ?? '').toLowerCase();
    return ExcludeSemantics(
      child: Container(
        width: Tokens.flagSize.width,
        height: Tokens.flagSize.height,
        clipBehavior: Clip.antiAlias,
        alignment: Alignment.center,
        decoration: BoxDecoration(color: t.sunk, borderRadius: BorderRadius.circular(3)),
        foregroundDecoration: BoxDecoration(
          borderRadius: BorderRadius.circular(3),
          border: Border.all(color: t.flagOutline, width: 0.5),
        ),
        child: _flags.contains(code)
            ? SvgPicture.asset('assets/flags/$code.svg', fit: BoxFit.cover, width: Tokens.flagSize.width, height: Tokens.flagSize.height)
            : Text(code.toUpperCase(), style: t.label.copyWith(fontSize: 8, height: 1, color: t.t3)),
      ),
    );
  }
}

/// The brand mark: the app icon, the double P (sign-in and the sidebar).
/// Drawn 1.22 times [size], as the native login did, so that the mark
/// itself fills [size] and the icon's transparent margin overflows it.
class BrandMark extends StatelessWidget {
  const BrandMark({super.key, required this.size});

  final double size;

  @override
  Widget build(BuildContext context) => SizedBox.square(
    dimension: size,
    child: OverflowBox(
      maxWidth: size * 1.22,
      maxHeight: size * 1.22,
      child: Image.asset(
        'assets/brand/app-icon.png',
        width: size * 1.22,
        height: size * 1.22,
        filterQuality: FilterQuality.medium,
        excludeFromSemantics: true,
      ),
    ),
  );
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

/// The explanation before the privileged service is installed (handoff
/// "7. 增强模式安装授权"): true on "Continue"; Cancel, Esc or a click outside
/// give false, which is no failure (the switch stays off).
Future<bool> confirmInstall(BuildContext context) async {
  final s = S.current;
  final go = await showFDialog<bool>(
    context: context,
    builder: (context, _, animation) {
      final t = Tokens.of(context);
      return FDialog(
        animation: animation,
        semanticsLabel: s('installT'),
        constraints: const BoxConstraints(minWidth: 280, maxWidth: Tokens.dialogWidth),
        builder: (context, _) => Padding(
          padding: Tokens.dialogPadding,
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              Semantics(header: true, child: Text(s('installT'), style: t.pageTitle)),
              const SizedBox(height: 8),
              Text(s('installD'), style: t.sub),
              const SizedBox(height: 20),
              Row(
                mainAxisAlignment: MainAxisAlignment.end,
                children: [
                  Button(
                    kind: ButtonKind.secondary,
                    label: s('cancel'),
                    onPress: () => Navigator.of(context).pop(false),
                  ),
                  const SizedBox(width: 8),
                  Button(
                    kind: ButtonKind.primary,
                    label: s('installGo'),
                    height: Tokens.controlHeight,
                    onPress: () => Navigator.of(context).pop(true),
                  ),
                ],
              ),
            ],
          ),
        ),
      );
    },
  );
  return go ?? false;
}
