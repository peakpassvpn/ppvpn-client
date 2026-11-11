// The phone nodes page (#448 step 5), to the canvas's 手机 · 节点 (#535):
// the test method and 全部测速 in the top bar, the search field, the region
// chips (the selected one solid accent), then all nodes grouped by region
// with the selected row highlighted in place, 62 a
// row (flag, name, the tier tag and the lines below, latency) with 0.5 px
// separators. A tap makes a node the current one; pulling down refreshes
// the list.

import '../icons.dart';

import 'dart:async';

import 'package:flutter/cupertino.dart' show CupertinoSliverRefreshControl;
import 'package:flutter/services.dart';
import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart' show FTheme;

import '../app_state.dart';
import '../node_identity.dart';
import '../pages/overview.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'phone.dart';
import 'native_chrome.dart';

class PhoneNodes extends StatefulWidget {
  const PhoneNodes({super.key, required this.app});

  final AppState app;

  @override
  State<PhoneNodes> createState() => _PhoneNodesState();
}

class _PhoneNodesState extends State<PhoneNodes> {
  rust.UiProbeMethod _method = rust.UiProbeMethod.icmp;
  bool _testing = false;
  String _query = '';

  /// The region chip; null for 全部.
  String? _region;

  AppState get app => widget.app;

  Future<void> _testAll() async {
    setState(() => _testing = true);
    await app.testAll(_method);
    if (mounted) setState(() => _testing = false);
  }

  void _nextMethod() => setState(
    () => _method = switch (_method) {
      rust.UiProbeMethod.icmp => rust.UiProbeMethod.tcp,
      rust.UiProbeMethod.tcp => rust.UiProbeMethod.connect,
      rust.UiProbeMethod.connect => rust.UiProbeMethod.icmp,
    },
  );

  String get _methodName => switch (_method) {
    rust.UiProbeMethod.icmp => 'Ping',
    rust.UiProbeMethod.tcp => 'TCP',
    rust.UiProbeMethod.connect => 'Connect',
  };

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) => PhoneScaffold(
      title: S.current('nodes'),
      nativeActions: [
        NativeChromeAction(
          id: 'probe-method',
          label: _methodName,
          onPress: _nextMethod,
        ),
        NativeChromeAction(
          id: 'test-all',
          label: S.current('testAll'),
          enabled: !_testing && app.canProbe(snapshot, _method),
          onPress: () => unawaited(_testAll()),
        ),
      ],
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _MethodButton(method: _method, onPress: _nextMethod),
          const SizedBox(width: 8),
          _TestAllButton(
            testing: _testing,
            onPress: _testing || !app.canProbe(snapshot, _method)
                ? null
                : _testAll,
          ),
        ],
      ),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Padding(
            padding: const EdgeInsets.symmetric(
              horizontal: Tokens.phonePadding,
              vertical: 4,
            ),
            child: Text(
              S.current.probeNote(_method, snapshot.phase),
              style: Tokens.of(context).note,
            ),
          ),
          Expanded(child: _body(context, snapshot)),
        ],
      ),
    ),
  );

  Widget _body(BuildContext context, rust.UiSnapshot snapshot) {
    final limited = LimitedAccess.of(snapshot);
    if (limited != null) return limited;
    return ValueListenableBuilder(
      valueListenable: app.nodes,
      builder: (context, nodes, _) {
        if (nodes.isEmpty) {
          return snapshot.profile == rust.UiProfile.invalid
              ? Center(
                  child: Padding(
                    padding: const EdgeInsets.all(Tokens.phonePadding),
                    child: ConfigWarning(previous: snapshot.profilePrevious),
                  ),
                )
              : const LoadingNodes();
        }
        final regions = <String>{
          for (final node in nodes)
            if ((node.exitRegion ?? '').isNotEmpty) node.exitRegion!,
        }.toList();
        final query = _query.trim().toLowerCase();
        final shown = [
          for (final node in nodes)
            if ((_region == null || node.exitRegion == _region) &&
                (query.isEmpty ||
                    nodeTitle(node).toLowerCase().contains(query) ||
                    (node.exitRegion ?? '').toLowerCase().contains(query)))
              node,
        ];
        final padding = phoneScrollPadding(context);
        // Keep every node in its original region and position.
        final groups = <String, List<rust.UiNode>>{};
        final counts = <String, int>{};
        for (final node in shown) {
          final region = node.exitRegion ?? '';
          counts[region] = (counts[region] ?? 0) + 1;
          (groups[region] ??= []).add(node);
        }
        final s = S.current;
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            MediaQuery.withClampedTextScaling(
              maxScaleFactor: Tokens.phoneChromeTextMax,
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.stretch,
                children: [
                  Padding(
                    padding: const EdgeInsets.fromLTRB(
                      Tokens.phonePadding,
                      4,
                      Tokens.phonePadding,
                      12,
                    ),
                    child: _SearchField(
                      onChanged: (value) => setState(() => _query = value),
                    ),
                  ),
                  if (regions.length > 1)
                    _RegionChips(
                      regions: regions,
                      selected: _region,
                      onSelect: (region) => setState(() => _region = region),
                    ),
                ],
              ),
            ),
            Expanded(
              child: CustomScrollView(
                physics: const BouncingScrollPhysics(
                  parent: AlwaysScrollableScrollPhysics(),
                ),
                slivers: [
                  CupertinoSliverRefreshControl(onRefresh: app.refreshNodes),
                  SliverPadding(
                    padding: padding.copyWith(top: 0),
                    sliver: shown.isEmpty
                        ? SliverToBoxAdapter(
                            child: Padding(
                              padding: const EdgeInsets.only(top: 40),
                              child: Text(
                                s('noMatches'),
                                textAlign: TextAlign.center,
                                style: Tokens.of(context).phoneNote,
                              ),
                            ),
                          )
                        : ValueListenableBuilder(
                            valueListenable: app.latency,
                            builder: (context, latency, _) {
                              Widget rowOf(
                                rust.UiNode node, {
                                required bool first,
                                required bool last,
                              }) => _NodeRow(
                                node: node,
                                first: first,
                                last: last,
                                current: node.id == snapshot.selectedNodeId,
                                latency: latency[node.id],
                                onPress: () =>
                                    unawaited(app.selectNode(node.id)),
                                onLongPress: () =>
                                    openNodeLines(context, app, node),
                              );
                              return SliverList.list(
                                children: [
                                  for (final MapEntry(key: region, value: nodes)
                                      in groups.entries) ...[
                                    PhoneCaption(
                                      text: region.isEmpty
                                          ? s('allRegions')
                                          : region,
                                      trailing: s('nodesCount', {
                                        'n': '${counts[region]}',
                                      }),
                                    ),
                                    PhoneCard(
                                      inset: 56,
                                      children: [
                                        for (final (index, node)
                                            in nodes.indexed)
                                          rowOf(
                                            node,
                                            first: index == 0,
                                            last: index == nodes.length - 1,
                                          ),
                                      ],
                                    ),
                                  ],
                                ],
                              );
                            },
                          ),
                  ),
                ],
              ),
            ),
          ],
        );
      },
    );
  }
}

/// The test method as a 36 pill in the accent colour on accent-soft (Ping,
/// TCP, Connect); a press moves to the next.
class _MethodButton extends StatelessWidget {
  const _MethodButton({required this.method, required this.onPress});

  final rust.UiProbeMethod method;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final name = switch (method) {
      rust.UiProbeMethod.icmp => 'Ping',
      rust.UiProbeMethod.tcp => 'TCP',
      rust.UiProbeMethod.connect => 'Connect',
    };
    return Pressable(
      key: const ValueKey('probe-method'),
      onPress: onPress,
      label: '${S.current('probe')}, $name',
      radius: Tokens.phoneSmallButton / 2,
      builder: (context, _) => SizedBox(
        height: Tokens.phoneControl,
        child: Center(
          child: Container(
            height: Tokens.phoneSmallButton,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            alignment: Alignment.center,
            decoration: BoxDecoration(
              color: t.accentSoft,
              borderRadius: BorderRadius.circular(Tokens.phoneSmallButton / 2),
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(
                  name,
                  style: t.phoneBodyStrong.copyWith(
                    fontSize: 14,
                    height: 20 / 14,
                    color: t.accent,
                  ),
                ),
                const SizedBox(width: 5),
                Icon(AppIcons.chevronDown, size: 12, color: t.accent),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// 全部测速: the primary pill (36, the gradient, the bolt and the label);
/// the spinner while the test runs.
class _TestAllButton extends StatelessWidget {
  const _TestAllButton({required this.testing, required this.onPress});

  final bool testing;
  final VoidCallback? onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final light = t.brightness == Brightness.light;
    final label = S.current('testAll');
    return Pressable(
      key: const ValueKey('test-all'),
      onPress: onPress,
      label: label,
      radius: Tokens.phoneSmallButton / 2,
      builder: (context, _) => SizedBox(
        height: Tokens.phoneControl,
        child: Center(
          child: Container(
            height: Tokens.phoneSmallButton,
            padding: const EdgeInsets.symmetric(horizontal: 14),
            decoration: BoxDecoration(
              gradient: light ? Tokens.primaryGradient : null,
              color: light ? null : t.accent,
              borderRadius: BorderRadius.circular(Tokens.phoneSmallButton / 2),
              boxShadow: light
                  ? const [
                      BoxShadow(
                        color: Color(0x471F4390),
                        offset: Offset(0, 4),
                        blurRadius: 12,
                      ),
                    ]
                  : null,
            ),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                if (testing)
                  Spinner(color: t.onAccent, size: 14)
                else
                  Icon(AppIcons.zap, size: 14, color: t.onAccent),
                const SizedBox(width: 6),
                Text(
                  label,
                  style: t.phoneBodyStrong.copyWith(
                    fontSize: 14,
                    height: 20 / 14,
                    color: t.onAccent,
                  ),
                ),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// The search field: 44 high on the phone's sunk fill, radius 12.
class _SearchField extends StatefulWidget {
  const _SearchField({required this.onChanged});

  final ValueChanged<String> onChanged;

  @override
  State<_SearchField> createState() => _SearchFieldState();
}

class _SearchFieldState extends State<_SearchField> {
  final TextEditingController _controller = TextEditingController();
  final FocusNode _focus = FocusNode();

  @override
  void dispose() {
    _controller.dispose();
    _focus.dispose();
    super.dispose();
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final hint = S.current('searchNodes');
    final style = t.phoneBody;
    return GestureDetector(
      onTap: _focus.requestFocus,
      child: Semantics(
        label: hint,
        textField: true,
        child: Container(
          height: Tokens.phoneControl,
          padding: const EdgeInsets.symmetric(horizontal: 12),
          decoration: BoxDecoration(
            color: t.phoneSunk,
            borderRadius: BorderRadius.circular(Tokens.phoneControlRadius),
          ),
          child: Row(
            children: [
              Icon(AppIcons.search, size: 18, color: t.t3),
              const SizedBox(width: 8),
              Expanded(
                child: Stack(
                  alignment: Alignment.centerLeft,
                  children: [
                    ValueListenableBuilder(
                      valueListenable: _controller,
                      builder: (context, value, _) => value.text.isEmpty
                          ? ExcludeSemantics(
                              child: Text(
                                hint,
                                maxLines: 1,
                                style: style.copyWith(color: t.t3),
                              ),
                            )
                          : const SizedBox(),
                    ),
                    EditableText(
                      controller: _controller,
                      focusNode: _focus,
                      style: style,
                      cursorColor: t.accent,
                      backgroundCursorColor: t.t3,
                      selectionColor: t.accentSoft,
                      maxLines: 1,
                      autocorrect: false,
                      textInputAction: TextInputAction.search,
                      onChanged: widget.onChanged,
                    ),
                  ],
                ),
              ),
            ],
          ),
        ),
      ),
    );
  }
}

/// The regions as chips (34 high), scrolling sideways; 全部 first.
class _RegionChips extends StatelessWidget {
  const _RegionChips({
    required this.regions,
    required this.selected,
    required this.onSelect,
  });

  final List<String> regions;
  final String? selected;
  final ValueChanged<String?> onSelect;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    Widget chip(String? region) {
      final on = region == selected;
      final text = region ?? S.current('allRegions');
      return Pressable(
        onPress: () => onSelect(region),
        selected: on,
        label: text,
        radius: 17,
        builder: (context, _) => AnimatedContainer(
          duration: Tokens.motion(context, Tokens.motionFast),
          curve: Tokens.motionFastCurve,
          height: 34,
          padding: const EdgeInsets.symmetric(horizontal: 15),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: on ? t.accent : t.phoneSunk,
            borderRadius: BorderRadius.circular(17),
          ),
          child: Text(
            text,
            style: t.phoneBody.copyWith(
              fontSize: 14,
              height: 20 / 14,
              fontWeight: on ? FontWeight.w600 : FontWeight.w500,
              color: on ? t.onAccent : t.t2,
            ),
          ),
        ),
      );
    }

    return SizedBox(
      height: 34,
      child: ListView(
        scrollDirection: Axis.horizontal,
        padding: const EdgeInsets.symmetric(horizontal: Tokens.phonePadding),
        children: [
          for (final (i, region) in <String?>[null, ...regions].indexed) ...[
            if (i > 0) const SizedBox(width: 8),
            chip(region),
          ],
        ],
      ),
    );
  }
}

class _NodeRow extends StatelessWidget {
  const _NodeRow({
    required this.node,
    required this.first,
    required this.last,
    required this.current,
    required this.latency,
    required this.onPress,
    required this.onLongPress,
  });

  final rust.UiNode node;
  final bool first;
  final bool last;
  final bool current;
  final Latency? latency;
  final VoidCallback onPress;
  final VoidCallback onLongPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    // PhoneCard pads its children inside its outline. Match that inner curve
    // so the selected row's stroke follows the card instead of being clipped.
    final cardInset =
        t
            .phoneCardDecoration()
            .border
            ?.dimensions
            .resolve(Directionality.of(context))
            .top ??
        0;
    final corner = Radius.circular(
      (Tokens.phoneCardRadius - cardInset)
          .clamp(0, Tokens.phoneCardRadius)
          .toDouble(),
    );
    return Pressable(
      key: ValueKey('node-${node.id}'),
      radius: 0,
      selected: current,
      onLongPress: onLongPress,
      label: nodeTitle(node),
      onPress: onPress,
      builder: (context, pressed) => AnimatedContainer(
        duration: Tokens.motion(
          context,
          pressed ? Duration.zero : Tokens.motionFast,
        ),
        decoration: BoxDecoration(
          borderRadius: BorderRadius.vertical(
            top: first ? corner : Radius.zero,
            bottom: last ? corner : Radius.zero,
          ),
          color: pressed
              ? t.phonePress
              : current
              ? t.accentSoft
              : t.phonePress.withValues(alpha: 0),
          // An inset stroke marks selection without stealing name/tag width.
          border: Border.all(
            color: current
                ? t.accent.withValues(alpha: 0.35)
                : t.accent.withValues(alpha: 0),
          ),
        ),
        constraints: const BoxConstraints(minHeight: Tokens.phoneRowTwoLines),
        padding: const EdgeInsetsDirectional.fromSTEB(16, 8, 14, 8),
        child: Row(
          children: [
            Flag(countryCode: node.exitCountryCode, size: Tokens.phoneFlagSize),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  NodeIdentity(
                    node: node,
                    phone: true,
                    strong: true,
                    tag: true,
                  ),
                  if (linesText(node).isNotEmpty)
                    Padding(
                      padding: const EdgeInsets.only(top: 3),
                      child: Text(
                        linesText(node),
                        maxLines: 1,
                        overflow: TextOverflow.ellipsis,
                        style: t.monoText(12, color: t.t3, height: 16),
                      ),
                    ),
                ],
              ),
            ),
            const SizedBox(width: 8),
            LatencyPill(latency: latency),
          ],
        ),
      ),
    );
  }
}

Future<void> openNodeLines(
  BuildContext context,
  AppState app,
  rust.UiNode node,
) async {
  final s = S.current;
  final labels = [s('autoLine'), ...lineNames(node)];
  final pinned = app.pinnedLine(node.id);
  final selected = pinned == null
      ? 0
      : node.lines.indexWhere((line) => line.key == pinned) + 1;
  int? choice;
  try {
    choice = await const MethodChannel('com.peakpassvpn.ppvpn/app')
        .invokeMethod<int>('chooseLine', {
          'title': s('chooseLine'),
          'message':
              node.lines.any((line) => line.label?.trim().isNotEmpty != true)
              ? s('lineNameMissing')
              : nodeDisplayName(node),
          'options': labels,
          'selected': selected,
          'cancel': s('cancel'),
        });
  } on MissingPluginException {
    if (!context.mounted) return;
    // Widget tests and older runners still use a popup, never a pushed page.
    final t = Tokens.of(context);
    choice = await showGeneralDialog<int>(
      context: context,
      barrierDismissible: true,
      barrierLabel: s('cancel'),
      barrierColor: const Color(0x66000000),
      transitionDuration: Tokens.motion(context, Tokens.motionPage),
      pageBuilder: (context, _, _) => FTheme(
        data: t.forui,
        child: DefaultTextStyle(
          style: t.body,
          child: Align(
            alignment: Alignment.bottomCenter,
            child: SafeArea(
              top: false,
              child: Padding(
                padding: const EdgeInsets.all(16),
                child: DecoratedBox(
                  decoration: t.cardDecoration(),
                  child: SingleChildScrollView(
                    child: Padding(
                      padding: const EdgeInsets.symmetric(vertical: 16),
                      child: Column(
                        mainAxisSize: MainAxisSize.min,
                        children: [
                          Text(s('chooseLine'), style: t.bodyStrong),
                          for (final (index, label) in labels.indexed)
                            Pressable(
                              onPress: () => Navigator.of(context).pop(index),
                              selected: selected == index,
                              builder: (context, _) => Padding(
                                padding: const EdgeInsets.all(16),
                                child: Text(label, style: t.body),
                              ),
                            ),
                        ],
                      ),
                    ),
                  ),
                ),
              ),
            ),
          ),
        ),
      ),
      transitionBuilder: (context, animation, _, child) => SlideTransition(
        position: Tween(begin: const Offset(0, 1), end: Offset.zero).animate(
          CurvedAnimation(parent: animation, curve: Tokens.motionCurve),
        ),
        child: child,
      ),
    );
  } on PlatformException catch (error) {
    app.actionError.value = error.message ?? error.code;
    return;
  }
  if (choice == null || choice < 0 || choice >= labels.length) return;
  await app.pinLine(node.id, choice == 0 ? null : node.lines[choice - 1].key);
}
