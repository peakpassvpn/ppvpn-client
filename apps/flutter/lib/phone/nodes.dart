// The phone nodes page (#448 step 5): the test method and test-all in the
// top bar, the search field, the region chips (the selected one solid
// accent), then the nodes in one card, 62 a row (flag, name, the current
// node's check, latency; the tier tag and the lines below) with 0.5 px
// dividers. A tap makes a node the current one; pulling down refreshes
// the list.

import 'dart:async';

import 'package:flutter/cupertino.dart' show CupertinoSliverRefreshControl;
import 'package:flutter/services.dart' show TextInputAction;
import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../pages/nodes.dart';
import '../pages/overview.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'phone.dart';

class PhoneNodes extends StatefulWidget {
  const PhoneNodes({super.key, required this.app});

  final AppState app;

  @override
  State<PhoneNodes> createState() => _PhoneNodesState();
}

class _PhoneNodesState extends State<PhoneNodes> {
  rust.UiProbeMethod _method = rust.UiProbeMethod.tcp;
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

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) => PhoneScaffold(
      title: S.current('nodes'),
      trailing: Row(
        mainAxisSize: MainAxisSize.min,
        children: [
          _MethodButton(
            method: _method,
            onPress: () => setState(
              () => _method = switch (_method) {
                rust.UiProbeMethod.icmp => rust.UiProbeMethod.tcp,
                rust.UiProbeMethod.tcp => rust.UiProbeMethod.connect,
                rust.UiProbeMethod.connect => rust.UiProbeMethod.icmp,
              },
            ),
          ),
          _TestAllButton(testing: _testing, onPress: _testing || !snapshot.standardReady ? null : _testAll),
        ],
      ),
      child: _body(context, snapshot),
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
              ? const Center(
                  child: Padding(padding: EdgeInsets.all(Tokens.phonePadding), child: ConfigWarning()),
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
        return Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            Padding(
              padding: const EdgeInsets.fromLTRB(Tokens.phonePadding, 4, Tokens.phonePadding, 10),
              child: _SearchField(onChanged: (value) => setState(() => _query = value)),
            ),
            if (regions.length > 1)
              _RegionChips(regions: regions, selected: _region, onSelect: (region) => setState(() => _region = region)),
            Expanded(
              child: CustomScrollView(
                physics: const BouncingScrollPhysics(parent: AlwaysScrollableScrollPhysics()),
                slivers: [
                  CupertinoSliverRefreshControl(onRefresh: app.refreshNodes),
                  SliverPadding(
                    padding: padding.copyWith(top: 10),
                    sliver: shown.isEmpty
                        ? SliverToBoxAdapter(
                            child: Padding(
                              padding: const EdgeInsets.only(top: 40),
                              child: Text(
                                S.current('noMatches'),
                                textAlign: TextAlign.center,
                                style: Tokens.of(context).phoneNote,
                              ),
                            ),
                          )
                        : ValueListenableBuilder(
                            valueListenable: app.latency,
                            builder: (context, latency, _) => DecoratedSliver(
                              decoration: Tokens.of(context).phoneCardDecoration(),
                              sliver: SliverList.builder(
                                itemCount: shown.length,
                                itemBuilder: (context, index) {
                                  final node = shown[index];
                                  return _NodeRow(
                                    node: node,
                                    current: node.id == snapshot.selectedNodeId,
                                    latency: latency[node.id],
                                    first: index == 0,
                                    onPress: () => unawaited(app.selectNode(node.id)),
                                  );
                                },
                              ),
                            ),
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

/// The test method as a small capsule (Ping, TCP, Connect); a press moves
/// to the next.
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
      radius: 15,
      builder: (context, _) => SizedBox(
        height: Tokens.phoneControl,
        child: Center(
          child: Container(
            height: 30,
            padding: const EdgeInsets.symmetric(horizontal: 12),
            alignment: Alignment.center,
            decoration: BoxDecoration(color: t.phoneSunk, borderRadius: BorderRadius.circular(15)),
            child: Row(
              mainAxisSize: MainAxisSize.min,
              children: [
                Text(name, style: t.monoText(13, weight: FontWeight.w500, color: t.t1)),
                const SizedBox(width: 4),
                Icon(FLucideIcons.chevronDown, size: 14, color: t.t3),
              ],
            ),
          ),
        ),
      ),
    );
  }
}

/// 全部测速: 44, the bolt; the spinner while the test runs.
class _TestAllButton extends StatelessWidget {
  const _TestAllButton({required this.testing, required this.onPress});

  final bool testing;
  final VoidCallback? onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Pressable(
      key: const ValueKey('test-all'),
      onPress: onPress,
      label: S.current('testAll'),
      builder: (context, _) => SizedBox(
        width: Tokens.phoneControl,
        height: Tokens.phoneControl,
        child: Center(
          child: testing ? Spinner(color: t.accent, size: 18) : Icon(FLucideIcons.zap, size: 20, color: t.accent),
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
              Icon(FLucideIcons.search, size: 18, color: t.t3),
              const SizedBox(width: 8),
              Expanded(
                child: Stack(
                  alignment: Alignment.centerLeft,
                  children: [
                    ValueListenableBuilder(
                      valueListenable: _controller,
                      builder: (context, value, _) => value.text.isEmpty
                          ? ExcludeSemantics(child: Text(hint, maxLines: 1, style: style.copyWith(color: t.t3)))
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

/// The regions as chips (32 high), scrolling sideways; 全部 first.
class _RegionChips extends StatelessWidget {
  const _RegionChips({required this.regions, required this.selected, required this.onSelect});

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
        radius: 16,
        builder: (context, _) => AnimatedContainer(
          duration: Tokens.motion(context, Tokens.motionFast),
          curve: Tokens.motionFastCurve,
          height: 32,
          padding: const EdgeInsets.symmetric(horizontal: 14),
          alignment: Alignment.center,
          decoration: BoxDecoration(color: on ? t.accent : t.phoneSunk, borderRadius: BorderRadius.circular(16)),
          child: Text(
            text,
            style: t.phoneNote.copyWith(fontWeight: FontWeight.w500, color: on ? t.onAccent : t.t2),
          ),
        ),
      );
    }

    return SizedBox(
      height: 32,
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
    required this.current,
    required this.latency,
    required this.first,
    required this.onPress,
  });

  final rust.UiNode node;
  final bool current;
  final Latency? latency;
  final bool first;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final lines = linesText(node);
    return Pressable(
      radius: 0,
      selected: current,
      label: nodeTitle(node),
      onPress: onPress,
      builder: (context, _) => Container(
        height: Tokens.phoneRowTwoLines,
        padding: const EdgeInsets.symmetric(horizontal: 16),
        decoration: BoxDecoration(
          border: first ? null : Border(top: BorderSide(color: t.line, width: 0.5)),
        ),
        child: Row(
          children: [
            Flag(countryCode: node.exitCountryCode, size: Tokens.phoneFlagSize),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                mainAxisAlignment: MainAxisAlignment.center,
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Row(
                    children: [
                      Flexible(
                        child: Text(nodeTitle(node), maxLines: 1, overflow: TextOverflow.ellipsis, style: t.phoneBodyStrong),
                      ),
                      if (current) ...[const SizedBox(width: 6), Icon(FLucideIcons.check, size: 16, color: t.accent)],
                    ],
                  ),
                  const SizedBox(height: 3),
                  Row(
                    children: [
                      TierTag(label: node.entryLabel, entryKey: node.entryKey),
                      if (lines.isNotEmpty) ...[
                        const SizedBox(width: 6),
                        Flexible(
                          child: Text(
                            lines,
                            maxLines: 1,
                            overflow: TextOverflow.ellipsis,
                            style: t.monoText(12, color: t.t2),
                          ),
                        ),
                      ],
                    ],
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
