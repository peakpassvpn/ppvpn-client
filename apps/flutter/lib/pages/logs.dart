// Client and desktop core logs are daily UTC files. Mobile core logs come
// from the actual tunnel host's state directory and use current/previous
// size archives. Polling is bounded, preserves source identity, and exposes
// missing, empty and unreadable files separately.

import '../icons.dart';

import 'dart:async';
import 'dart:collection';
import 'dart:convert';
import 'dart:io';
import 'dart:math' as math;
import 'dart:typed_data';

import 'package:flutter/foundation.dart';
import 'package:flutter/scheduler.dart';
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../sample.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

/// The names match the string keys (`client`, `core`, `today`, `yesterday`).
enum LogKind { client, core }

// Calendar choices remain for client/desktop. Mobile core archives are
// size-rotated files, so current/previous are deliberately not dates.
enum LogDay { today, yesterday, current, previous }

enum LogReadState { loading, ready, empty, missing, unreadable, unavailable }

List<LogDay> logChoices(LogKind kind, {required bool mobile}) =>
    mobile && kind == LogKind.core
    ? const [LogDay.current, LogDay.previous]
    : const [LogDay.today, LogDay.yesterday];

String hostCoreLogDirectory(String filesDir) =>
    '$filesDir${Platform.pathSeparator}tunnel-state${Platform.pathSeparator}logs';

String logDayKey(LogDay day) => switch (day) {
  LogDay.current => 'logCurrent',
  LogDay.previous => 'logPrevious',
  _ => day.name,
};

/// crates/desktop's daily file, e.g. `ppvpn-client.2026-10-21.log`, dated
/// in UTC.
String logFileName(LogKind kind, LogDay day, DateTime nowUtc) {
  if (day == LogDay.current || day == LogDay.previous) {
    if (kind != LogKind.core)
      throw ArgumentError('Only host core logs are size-rotated');
    return day == LogDay.current ? 'ppvpn-core.log' : 'ppvpn-core.log.1';
  }
  final date = day == LogDay.today
      ? nowUtc
      : nowUtc.subtract(const Duration(days: 1));
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
  Uint8List _prefix = Uint8List(0);
  Uint8List _anchor = Uint8List(0);

  /// The lines added since the previous call; [reset] when the file got
  /// shorter (cut or replaced): what was shown before is gone.
  Future<({List<String> lines, bool reset, LogReadState state})> poll() async {
    const none = <String>[];
    final file = File(path);
    final Uint8List bytes;
    var reset = false;
    var start = 0;
    try {
      final raf = await file.open();
      try {
        final length = await raf.length();
        final prefix = await raf.read(math.min(length, 128));
        var replaced =
            length < _offset ||
            (_prefix.isNotEmpty &&
                (prefix.length < _prefix.length ||
                    !listEquals(_prefix, prefix.sublist(0, _prefix.length))));
        if (!replaced && _anchor.isNotEmpty) {
          await raf.setPosition(_offset - _anchor.length);
          replaced = !listEquals(_anchor, await raf.read(_anchor.length));
        }
        if (replaced) {
          _offset = 0;
          _carry = Uint8List(0);
          _anchor = Uint8List(0);
          reset = true;
        }
        _prefix = prefix;
        if (length == _offset) {
          return (
            lines: none,
            reset: reset,
            state: length == 0 ? LogReadState.empty : LogReadState.ready,
          );
        }
        start = math.max(_offset, length - maxRead);
        if (start > _offset) _carry = Uint8List(0);
        await raf.setPosition(start);
        bytes = await raf.read(length - start);
        final readEnd = start + bytes.length;
        await raf.setPosition(math.max(0, readEnd - 64));
        _anchor = await raf.read(math.min(readEnd, 64));
      } finally {
        await raf.close();
      }
    } on FileSystemException catch (error) {
      reset = _offset != 0 || _carry.isNotEmpty || _prefix.isNotEmpty;
      _offset = 0;
      _carry = Uint8List(0);
      _prefix = Uint8List(0);
      _anchor = Uint8List(0);
      final missing =
          error.osError?.errorCode == 2 || error.osError?.errorCode == 3;
      return (
        lines: none,
        reset: reset,
        state: missing ? LogReadState.missing : LogReadState.unreadable,
      );
    }
    final skipped = start > _offset;
    _offset = start + bytes.length;
    var from = 0;
    if (skipped && start > 0) {
      // Skipped ahead: the first line is a part; drop it.
      final newline = bytes.indexOf(10);
      if (newline < 0)
        return (lines: none, reset: reset, state: LogReadState.ready);
      from = newline + 1;
    }
    final lines = <String>[];
    while (true) {
      final newline = bytes.indexOf(10, from);
      if (newline < 0) break;
      final part = Uint8List.sublistView(bytes, from, newline);
      lines.add(
        _decode(
          _carry.isEmpty
              ? part
              : (BytesBuilder(copy: false)
                      ..add(_carry)
                      ..add(part))
                    .takeBytes(),
        ),
      );
      _carry = Uint8List(0);
      from = newline + 1;
    }
    if (from < bytes.length) {
      // Copied, so that the read buffer can go.
      _carry =
          (BytesBuilder()
                ..add(_carry)
                ..add(Uint8List.sublistView(bytes, from)))
              .takeBytes();
      if (_carry.length > maxCarry) {
        lines.add(_decode(_carry));
        _carry = Uint8List(0);
      }
    }
    return (lines: lines, reset: reset, state: LogReadState.ready);
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
  String? _clientDir;
  String? _coreDir;
  bool _directoriesLoaded = false;
  LogReadState _readState = LogReadState.loading;
  bool get _mobile =>
      defaultTargetPlatform == TargetPlatform.iOS ||
      defaultTargetPlatform == TargetPlatform.android;
  String? get _dir => _mobile && _kind == LogKind.core ? _coreDir : _clientDir;
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
      _clientDir = _coreDir = sampleLogDir;
      _directoriesLoaded = true;
      _readState = LogReadState.ready;
      _lines.addAll(sampleLogLines);
      return;
    }
    unawaited(_loadDirectories());
  }

  Future<void> _loadDirectories() async {
    String? client;
    String? core;
    try {
      client = rust.logDir();
    } catch (error) {
      debugPrint('log_dir: $error');
    }
    if (_mobile) {
      try {
        // The same existing channel main.dart uses: App Group on iOS,
        // real filesDir on Android. No alternate container or permission bypass.
        final base = await const MethodChannel('com.peakpassvpn.ppvpn/app')
            .invokeMethod<String>('filesDir');
        if (base != null && base.isNotEmpty) core = hostCoreLogDirectory(base);
      } on PlatformException {
        core = null;
      } on MissingPluginException {
        core = null;
      }
    } else {
      core = client;
    }
    if (!mounted) return;
    setState(() {
      _clientDir = client;
      _coreDir = core;
      _directoriesLoaded = true;
    });
    _timer = Timer.periodic(_pollEvery, (_) => unawaited(_poll()));
    await _poll();
  }

  @override
  void dispose() {
    _timer?.cancel();
    _scroll.dispose();
    super.dispose();
  }

  DateTime get _nowUtc =>
      samplePage != null ? sampleLogDate : DateTime.now().toUtc();

  String? get _path {
    final dir = _dir;
    return dir == null
        ? null
        : '$dir${Platform.pathSeparator}${logFileName(_kind, _day, _nowUtc)}';
  }

  Future<void> _poll() async {
    final path = _path;
    if (_polling) return;
    if (path == null) {
      final state = _directoriesLoaded
          ? LogReadState.unavailable
          : LogReadState.loading;
      if (mounted && _readState != state) setState(() => _readState = state);
      return;
    }
    _polling = true;
    try {
      // A new file: another choice, or the UTC day turned.
      if (_tail?.path != path) {
        _tail = LogTail(path);
        if (_lines.isNotEmpty) setState(_lines.clear);
      }
      final tail = _tail!;
      final (:lines, :reset, :state) = await tail.poll();
      if (!mounted ||
          !identical(tail, _tail) ||
          (lines.isEmpty && !reset && state == _readState))
        return;
      setState(() {
        _readState = state;
        if (reset) _lines.clear();
        _lines.addAll(lines.skip(math.max(0, lines.length - _maxLines)));
        while (_lines.length > _maxLines) {
          _lines.removeFirst();
        }
      });
      if (_following) _toEnd();
    } finally {
      _polling = false;
      if (mounted && _path != null && _tail?.path != _path) unawaited(_poll());
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
      _readState = LogReadState.loading;
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
    if (SchedulerBinding.instance.schedulerPhase ==
        SchedulerPhase.persistentCallbacks) {
      WidgetsBinding.instance.addPostFrameCallback(
        (_) => mounted ? _onScroll() : null,
      );
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
    return LayoutBuilder(
      builder: (context, box) {
        final inset = box.maxWidth < Tokens.phoneBreakpoint
            ? Tokens.phonePadding
            : Tokens.pageInset;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            _Tools(
              inset: inset,
              kind: _kind,
              day: _day,
              mobile: _mobile,
              nowUtc: _nowUtc,
              onSelect: _select,
              following: _following,
              onFollow: _follow,
              onReveal: _dir == null ? null : _reveal,
            ),
            Expanded(
              child: Padding(
                padding: EdgeInsets.fromLTRB(
                  inset,
                  0,
                  inset,
                  16 + MediaQuery.paddingOf(context).bottom,
                ),
                child: CardBox(
                  child: Column(
                    crossAxisAlignment: CrossAxisAlignment.stretch,
                    children: [
                      Expanded(
                        child: Semantics(
                          label: S.current('logs'),
                          container: true,
                          child: _lines.isEmpty
                              ? Center(
                                  child: Padding(
                                    padding: const EdgeInsets.all(20),
                                    child: Text(
                                      S.current(switch (_readState) {
                                        LogReadState.loading => 'logLoading',
                                        LogReadState.missing => 'logMissing',
                                        LogReadState.unreadable =>
                                          'logUnreadable',
                                        LogReadState.unavailable =>
                                          'logUnavailable',
                                        LogReadState.empty => 'logEmpty',
                                        LogReadState.ready => 'logWaitingLine',
                                      }),
                                      textAlign: TextAlign.center,
                                      style: t.body.copyWith(color: t.t3),
                                    ),
                                  ),
                                )
                              : ListView.builder(
                                  controller: _scroll,
                                  padding: Tokens.logPadding,
                                  itemCount: _lines.length,
                                  itemBuilder: (context, index) => Text(
                                    _lines.elementAt(index),
                                    style: t.logLine,
                                  ),
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
      },
    );
  }
}

class _Tools extends StatelessWidget {
  const _Tools({
    required this.inset,
    required this.kind,
    required this.day,
    required this.mobile,
    required this.nowUtc,
    required this.onSelect,
    required this.following,
    required this.onFollow,
    required this.onReveal,
  });

  final double inset;
  final LogKind kind;
  final LogDay day;
  final bool mobile;
  final DateTime nowUtc;
  final void Function(LogKind kind, LogDay day) onSelect;
  final bool following;
  final VoidCallback onFollow;
  final VoidCallback? onReveal;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    String label(LogKind k, LogDay d) => '${s(k.name)} · ${s(logDayKey(d))}';
    final reveal = Platform.isMacOS
        ? s('revealMac')
        : Platform.isWindows
        ? s('revealWin')
        : s('revealLinux');
    final canReveal =
        Platform.isMacOS || Platform.isWindows || Platform.isLinux;
    return Padding(
      padding: EdgeInsets.fromLTRB(inset, 8, inset, 12),
      child: LayoutBuilder(
        builder: (context, box) {
          final compact = box.maxWidth < 720;
          final menu = FPopoverMenu(
            menuAnchor: Alignment.topLeft,
            childAnchor: Alignment.bottomLeft,
            menu: [
              FItemGroup(
                children: [
                  for (final k in LogKind.values)
                    for (final d in logChoices(k, mobile: mobile))
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
                width: compact ? null : Tokens.logMenuWidth,
                height: compact ? Tokens.phoneControl : Tokens.controlHeight,
                padding: const EdgeInsets.fromLTRB(11, 0, 10, 0),
                decoration: BoxDecoration(
                  color: t.brightness == Brightness.light ? t.card : t.sunk,
                  borderRadius: BorderRadius.circular(
                    compact ? Tokens.phoneControlRadius : Tokens.controlRadius,
                  ),
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
                    Icon(AppIcons.chevronDown, size: 12, color: t.t3),
                  ],
                ),
              ),
            ),
          );
          final follow = following
              ? Semantics(
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
              : Button(
                  kind: ButtonKind.text,
                  icon: AppIcons.arrowDown,
                  label: s('live'),
                  height: 26,
                  fontSize: 12,
                  onPress: onFollow,
                );
          final revealButton = Button(
            kind: ButtonKind.secondary,
            icon: AppIcons.folderOpen,
            label: reveal,
            onPress: onReveal,
          );
          if (compact) {
            return Column(
              crossAxisAlignment: CrossAxisAlignment.stretch,
              children: [
                Row(
                  children: [
                    Text(s('logFile'), style: t.sub),
                    const SizedBox(width: 8),
                    Expanded(child: menu),
                  ],
                ),
                const SizedBox(height: 10),
                Wrap(
                  spacing: 12,
                  runSpacing: 8,
                  alignment: WrapAlignment.end,
                  crossAxisAlignment: WrapCrossAlignment.center,
                  children: [follow, if (canReveal) revealButton],
                ),
              ],
            );
          }
          return Wrap(
            spacing: 12,
            runSpacing: 8,
            crossAxisAlignment: WrapCrossAlignment.center,
            children: [
              Text(s('logFile'), style: t.sub),
              menu,
              follow,
              if (canReveal) revealButton,
            ],
          );
        },
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

class _BreathingDotState extends State<_BreathingDot>
    with SingleTickerProviderStateMixin {
  late final AnimationController _breath = AnimationController(
    vsync: this,
    duration: Tokens.pulseHalf,
  )..repeat(reverse: true);
  late final Animation<double> _opacity = _breath.drive(
    Tween(
      begin: 1.0,
      end: Tokens.pulseLow,
    ).chain(CurveTween(curve: Curves.easeInOut)),
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
    return MediaQuery.disableAnimationsOf(context)
        ? dot
        : FadeTransition(opacity: _opacity, child: dot);
  }
}
