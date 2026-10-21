// Logs (handoff §4 日志): the tool row (the log file menu, client or core
// × today or yesterday; 「正在跟随末尾」 with a breathing dot while the view
// sits at the end, else 「实时」 to jump back to it; show the file in the
// file manager), then the log card: monospace lines, new ones appended and
// followed, the file's path at its foot. The files are crates/desktop's
// daily ones (UTC dates) in the client's log directory, tailed here with
// dart:io: a poll a second reads only the bytes added since the last, and
// at most [_LogsPageState._maxLines] lines stay in memory, so the UI
// process stays flat however long the page is open (#387). Not in the
// prototype: selecting text.

import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../sample.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

/// The names match the string keys (`client`, `core`, `today`, `yesterday`).
enum LogKind { client, core }

enum LogDay { today, yesterday }

/// crates/desktop's daily file, e.g. `ppvpn-client.2026-10-21.log`, dated
/// in UTC.
String logFileName(LogKind kind, LogDay day, DateTime nowUtc) {
  final date = day == LogDay.today ? nowUtc : nowUtc.subtract(const Duration(days: 1));
  String two(int n) => n.toString().padLeft(2, '0');
  return 'ppvpn-${kind.name}.${date.year}-${two(date.month)}-${two(date.day)}.log';
}

/// Follows one file. Each [poll] reads what was appended since the previous
/// one and returns its complete lines. Bounded: a read takes at most
/// [maxRead] bytes (older ones are skipped), an unfinished line is kept up
/// to [maxCarry] bytes, and a line is cut at [maxLine] characters.
class LogTail {
  LogTail(this.path);

  final String path;

  static const maxRead = 512 * 1024;
  static const maxCarry = 64 * 1024;
  static const maxLine = 2000;

  int _offset = 0;
  Uint8List _carry = Uint8List(0);

  /// The lines added since the previous call; [reset] when the file got
  /// shorter (cut or replaced): what was shown before is gone.
  Future<({List<String> lines, bool reset})> poll() async {
    const none = <String>[];
    final file = File(path);
    final Uint8List bytes;
    var reset = false;
    var start = 0;
    try {
      final length = await file.length();
      if (length < _offset) {
        _offset = 0;
        _carry = Uint8List(0);
        reset = true;
      }
      if (length == _offset) return (lines: none, reset: reset);
      start = math.max(_offset, length - maxRead);
      if (start > _offset) _carry = Uint8List(0);
      final raf = await file.open();
      try {
        await raf.setPosition(start);
        bytes = await raf.read(length - start);
      } finally {
        await raf.close();
      }
    } on FileSystemException {
      // Not written yet (no such day), or removed by the log retention.
      return (lines: none, reset: reset);
    }
    final skipped = start > _offset;
    _offset = start + bytes.length;
    var from = 0;
    if (skipped && start > 0) {
      // Skipped ahead: the first line is a part; drop it.
      final newline = bytes.indexOf(10);
      if (newline < 0) return (lines: none, reset: reset);
      from = newline + 1;
    }
    final lines = <String>[];
    while (true) {
      final newline = bytes.indexOf(10, from);
      if (newline < 0) break;
      final part = Uint8List.sublistView(bytes, from, newline);
      lines.add(_decode(_carry.isEmpty ? part : (BytesBuilder(copy: false)..add(_carry)..add(part)).takeBytes()));
      _carry = Uint8List(0);
      from = newline + 1;
    }
    if (from < bytes.length) {
      // Copied, so that the read buffer can go.
      _carry = (BytesBuilder()..add(_carry)..add(Uint8List.sublistView(bytes, from))).takeBytes();
      if (_carry.length > maxCarry) {
        lines.add(_decode(_carry));
        _carry = Uint8List(0);
      }
    }
    return (lines: lines, reset: reset);
  }

  static String _decode(Uint8List bytes) {
    var line = utf8.decode(bytes, allowMalformed: true);
    if (line.endsWith('\r')) line = line.substring(0, line.length - 1);
    return line.length > maxLine ? '${line.substring(0, maxLine)}…' : line;
  }
}

class LogsPage extends StatefulWidget {
  const LogsPage({super.key});

  @override
  State<LogsPage> createState() => _LogsPageState();
}

class _LogsPageState extends State<LogsPage> {
  static const _maxLines = 2000;
  static const _pollEvery = Duration(seconds: 1);

  /// How close to the end still counts as at the end.
  static const _endSlack = 4.0;

  LogKind _kind = LogKind.client;
  LogDay _day = LogDay.today;
  String? _dir;
  LogTail? _tail;
  final _lines = ListQueue<String>();
  final _scroll = ScrollController();
  Timer? _timer;
  bool _polling = false;
  bool _following = true;

  /// Set while the page itself scrolls to the end.
  bool _jumping = false;

  @override
  void initState() {
    super.initState();
    _scroll.addListener(_onScroll);
    if (samplePage != null) {
      // Screenshot mode: fixed lines, no file is read.
      _dir = sampleLogDir;
      _lines.addAll(sampleLogLines);
      return;
    }
    try {
      _dir = rust.logDir();
    } catch (error) {
      debugPrint('log_dir: $error');
      return;
    }
    _timer = Timer.periodic(_pollEvery, (_) => unawaited(_poll()));
    unawaited(_poll());
  }

  @override
  void dispose() {
    _timer?.cancel();
    _scroll.dispose();
    super.dispose();
  }

  DateTime get _nowUtc => samplePage != null ? sampleLogDate : DateTime.now().toUtc();

  String? get _path {
    final dir = _dir;
    return dir == null ? null : '$dir${Platform.pathSeparator}${logFileName(_kind, _day, _nowUtc)}';
  }

  Future<void> _poll() async {
    final path = _path;
    if (_polling || path == null) return;
    _polling = true;
    try {
      // A new file: another choice, or the UTC day turned.
      if (_tail?.path != path) {
        _tail = LogTail(path);
        if (_lines.isNotEmpty) setState(_lines.clear);
      }
      final tail = _tail!;
      final (:lines, :reset) = await tail.poll();
      if (!mounted || !identical(tail, _tail) || (lines.isEmpty && !reset)) return;
      setState(() {
        if (reset) _lines.clear();
        _lines.addAll(lines.skip(math.max(0, lines.length - _maxLines)));
        while (_lines.length > _maxLines) {
          _lines.removeFirst();
        }
      });
      if (_following) _toEnd();
    } finally {
      _polling = false;
    }
  }

  void _select(LogKind kind, LogDay day) {
    if (kind == _kind && day == _day) return;
    setState(() {
      _kind = kind;
      _day = day;
      // A poll under way for the old file is dropped (see [_poll]).
      _tail = null;
      _lines.clear();
      _following = true;
    });
    unawaited(_poll());
  }

  /// Only a scroll the user makes changes [_following]; content growing
  /// moves no pixels and leaves it as it is.
  void _onScroll() {
    if (_jumping || !_scroll.hasClients) return;
    final position = _scroll.position;
    final atEnd = position.pixels >= position.maxScrollExtent - _endSlack;
    if (atEnd == _following) return;
    // A position corrected during layout must not rebuild in that frame.
    if (SchedulerBinding.instance.schedulerPhase == SchedulerPhase.persistentCallbacks) {
      WidgetsBinding.instance.addPostFrameCallback((_) => mounted ? _onScroll() : null);
    } else {
      setState(() => _following = atEnd);
    }
  }

  /// To the end after the next layout; again while the lines laid out
  /// late still move the end (their heights are estimates until then).
  void _toEnd([int tries = 3]) {
    WidgetsBinding.instance.addPostFrameCallback((_) {
      if (!mounted || !_following || !_scroll.hasClients) return;
      final position = _scroll.position;
      if (position.pixels >= position.maxScrollExtent) return;
      _jumping = true;
      _scroll.jumpTo(position.maxScrollExtent);
      _jumping = false;
      if (tries > 1) _toEnd(tries - 1);
    });
  }

  void _follow() {
    setState(() => _following = true);
    _toEnd();
  }

  /// The file selected in the file manager, or the directory when the
  /// file is not there (yet). Arguments go to the program as they are; no
  /// shell.
  Future<void> _reveal() async {
    final dir = _dir;
    final path = _path;
    if (dir == null || path == null) return;
    final exists = File(path).existsSync();
    final (String program, List<String> args) = Platform.isMacOS
        ? ('open', exists ? ['-R', path] : [dir])
        : Platform.isWindows
        // `explorer /select, "<path>"`: the path is its own argument.
        ? ('explorer.exe', exists ? ['/select,', path] : [dir])
        : ('xdg-open', [dir]);
    try {
      await Process.start(program, args, mode: ProcessStartMode.detached);
    } on ProcessException catch (error) {
      debugPrint('reveal: $error');
    }
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        _Tools(
          kind: _kind,
          day: _day,
          nowUtc: _nowUtc,
          onSelect: _select,
          following: _following,
          onFollow: _follow,
          onReveal: _dir == null ? null : _reveal,
        ),
        Expanded(
          child: Padding(
            padding: const EdgeInsets.fromLTRB(Tokens.pageInset, 0, Tokens.pageInset, 16),
            child: CardBox(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Expanded(
                    child: Semantics(
                      label: S.current('logs'),
                      container: true,
                      child: ListView.builder(
                        controller: _scroll,
                        padding: Tokens.logPadding,
                        itemCount: _lines.length,
                        itemBuilder: (context, index) => Text(_lines.elementAt(index), style: t.logLine),
                      ),
                    ),
                  ),
                  Container(
                    height: Tokens.logFooterHeight,
                    padding: const EdgeInsets.symmetric(horizontal: 14),
                    alignment: Alignment.centerLeft,
                    decoration: BoxDecoration(
                      color: t.sunk,
                      border: Border(top: BorderSide(color: t.hair)),
                    ),
                    child: Text(
                      _path ?? '—',
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.logPath,
                    ),
                  ),
                ],
              ),
            ),
          ),
        ),
      ],
    );
  }
}

class _Tools extends StatelessWidget {
  const _Tools({
    required this.kind,
    required this.day,
    required this.nowUtc,
    required this.onSelect,
    required this.following,
    required this.onFollow,
    required this.onReveal,
  });

  final LogKind kind;
  final LogDay day;
  final DateTime nowUtc;
  final void Function(LogKind kind, LogDay day) onSelect;
  final bool following;
  final VoidCallback onFollow;
  final VoidCallback? onReveal;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    String label(LogKind k, LogDay d) => '${s(k.name)} · ${s(d.name)}';
    final reveal = Platform.isMacOS
        ? s('revealMac')
        : Platform.isWindows
        ? s('revealWin')
        : s('revealLinux');
    return Padding(
      padding: const EdgeInsets.fromLTRB(Tokens.pageInset, 0, Tokens.pageInset, 12),
      child: Row(
        children: [
          Text(s('logFile'), style: t.sub),
          const SizedBox(width: 8),
          FPopoverMenu(
            menuAnchor: Alignment.topLeft,
            childAnchor: Alignment.bottomLeft,
            menu: [
              FItemGroup(
                children: [
                  for (final k in LogKind.values)
                    for (final d in LogDay.values)
                      FItem(
                        selected: k == kind && d == day,
                        title: Text(label(k, d)),
                        subtitle: Text(logFileName(k, d, nowUtc)),
                        onPress: () => onSelect(k, d),
                      ),
                ],
              ),
            ],
            builder: (context, controller, _) => Pressable(
              onPress: controller.toggle,
              label: '${s('logFile')}: ${label(kind, day)}',
              builder: (context, hovered) => Container(
                width: Tokens.logMenuWidth,
                height: Tokens.controlHeight,
                padding: const EdgeInsets.fromLTRB(11, 0, 10, 0),
                decoration: BoxDecoration(
                  color: t.brightness == Brightness.light ? t.card : t.sunk,
                  borderRadius: BorderRadius.circular(Tokens.controlRadius),
                  border: Border.all(color: t.line),
                ),
                child: Row(
                  children: [
                    Expanded(
                      child: Text(
                        label(kind, day),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: t.control.copyWith(color: t.t1),
                      ),
                    ),
                    const SizedBox(width: 8),
                    Icon(FLucideIcons.chevronDown, size: 12, color: t.t3),
                  ],
                ),
              ),
            ),
          ),
          const Spacer(),
          if (following)
            Semantics(
              container: true,
              child: Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  const _BreathingDot(),
                  const SizedBox(width: 6),
                  Text(s('following'), style: t.note.copyWith(color: t.t2)),
                ],
              ),
            )
          else
            Button(kind: ButtonKind.text, icon: FLucideIcons.arrowDown, label: s('live'), height: 26, fontSize: 12, onPress: onFollow),
          const SizedBox(width: 12),
          Button(kind: ButtonKind.secondary, icon: FLucideIcons.folderOpen, label: reveal, onPress: onReveal),
        ],
      ),
    );
  }
}

/// The ok-coloured dot, its opacity breathing (still when the system asks
/// for no animations).
class _BreathingDot extends StatefulWidget {
  const _BreathingDot();

  @override
  State<_BreathingDot> createState() => _BreathingDotState();
}

class _BreathingDotState extends State<_BreathingDot> with SingleTickerProviderStateMixin {
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
    final dot = Container(
      width: Tokens.pulseDotSize,
      height: Tokens.pulseDotSize,
      decoration: BoxDecoration(color: t.ok, shape: BoxShape.circle),
    );
    return MediaQuery.disableAnimationsOf(context) ? dot : FadeTransition(opacity: _opacity, child: dot);
  }
}
