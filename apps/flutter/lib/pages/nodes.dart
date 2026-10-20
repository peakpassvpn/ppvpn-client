// Nodes (handoff "3. 节点"): the action bar (test method, test all,
// refresh), the table (current mark, node, entry tier, region, lines in
// failover order, latency) and the footer with the selected node's local
// proxy. Click selects a row, double-click makes it the current node. The
// row context menu is not in the prototype.

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'overview.dart';

class NodesPage extends StatefulWidget {
  const NodesPage({super.key, required this.app});

  final AppState app;

  @override
  State<NodesPage> createState() => _NodesPageState();
}

class _NodesPageState extends State<NodesPage> {
  rust.UiProbeMethod _method = rust.UiProbeMethod.tcp;
  String? _selectedRow;
  bool _testing = false;

  AppState get app => widget.app;

  Future<void> _testAll() async {
    setState(() => _testing = true);
    await app.testAll(_method);
    if (mounted) setState(() => _testing = false);
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final limited = LimitedAccess.of(snapshot);
      if (limited != null) return limited;
      return ValueListenableBuilder(
        valueListenable: app.nodes,
        builder: (context, nodes, _) {
          if (nodes.isEmpty) {
            return snapshot.profile == rust.UiProfile.invalid ? const _InvalidEmpty() : const LoadingNodes();
          }
          final selectedRow = _selectedRow ?? snapshot.selectedNodeId;
          return Column(
            crossAxisAlignment: CrossAxisAlignment.stretch,
            children: [
              _ActionBar(
                method: _method,
                onMethod: (method) => setState(() => _method = method),
                testing: _testing,
                canTest: snapshot.standardReady,
                onTestAll: _testAll,
                onRefresh: app.refreshNodes,
              ),
              Expanded(
                child: _Table(
                  app: app,
                  nodes: nodes,
                  current: snapshot.selectedNodeId,
                  selected: selectedRow,
                  onSelect: (id) => setState(() => _selectedRow = id),
                ),
              ),
              _Footer(app: app, node: app.nodeById(selectedRow), count: nodes.length, testing: _testing),
            ],
          );
        },
      );
    },
  );
}

class _ActionBar extends StatelessWidget {
  const _ActionBar({
    required this.method,
    required this.onMethod,
    required this.testing,
    required this.canTest,
    required this.onTestAll,
    required this.onRefresh,
  });

  final rust.UiProbeMethod method;
  final ValueChanged<rust.UiProbeMethod> onMethod;
  final bool testing;
  final bool canTest;
  final VoidCallback onTestAll;
  final VoidCallback onRefresh;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    Widget segment(rust.UiProbeMethod value, String label) => SizedBox(
      height: Tokens.controlHeight,
      child: FButton(
        variant: value == method ? FButtonVariant.secondary : FButtonVariant.ghost,
        size: FButtonSizeVariant.xs,
        mainAxisSize: MainAxisSize.min,
        selected: value == method,
        onPress: () => onMethod(value),
        child: Text(label),
      ),
    );
    return Container(
      height: 40,
      padding: const EdgeInsets.symmetric(horizontal: 24),
      decoration: BoxDecoration(border: Border(bottom: BorderSide(color: t.border, width: 0.5))),
      child: Row(
        children: [
          Text(s('probe'), style: t.secondary),
          const SizedBox(width: 8),
          Semantics(
            label: s('probe'),
            child: Row(
              children: [
                segment(rust.UiProbeMethod.icmp, 'Ping'),
                segment(rust.UiProbeMethod.tcp, 'TCP'),
                segment(rust.UiProbeMethod.connect, 'Connect'),
              ],
            ),
          ),
          const Spacer(),
          SizedBox(
            height: Tokens.controlHeight,
            child: FButton(
              variant: FButtonVariant.outline,
              size: FButtonSizeVariant.xs,
              mainAxisSize: MainAxisSize.min,
              onPress: testing || !canTest ? null : onTestAll,
              prefix: const Icon(FLucideIcons.gauge, size: 12),
              child: Text(s('testAll')),
            ),
          ),
          const SizedBox(width: 6),
          SizedBox(
            height: Tokens.controlHeight,
            child: FButton(
              variant: FButtonVariant.outline,
              size: FButtonSizeVariant.xs,
              mainAxisSize: MainAxisSize.min,
              onPress: onRefresh,
              prefix: const Icon(FLucideIcons.refreshCw, size: 12),
              child: Text(s('refreshNodes')),
            ),
          ),
        ],
      ),
    );
  }
}

class _Table extends StatelessWidget {
  const _Table({
    required this.app,
    required this.nodes,
    required this.current,
    required this.selected,
    required this.onSelect,
  });

  final AppState app;
  final List<rust.UiNode> nodes;
  final String? current;
  final String? selected;
  final ValueChanged<String> onSelect;

  // Column widths: mark, node (flex), entry, region, lines (flex), latency.
  static const _mark = 22.0;
  static const _entry = 56.0;
  static const _region = 88.0;
  static const _latency = 72.0;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    Widget header(String text, {double? width, int flex = 0, TextAlign align = TextAlign.start}) {
      final label = Text(text, style: t.note.copyWith(fontWeight: FontWeight.w600), textAlign: align);
      return width != null ? SizedBox(width: width, child: label) : Expanded(flex: flex, child: label);
    }

    return Column(
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(24, 8, 24, 4),
          child: Row(
            children: [
              const SizedBox(width: _mark),
              header(s('colName'), flex: 3),
              header(s('colTier'), width: _entry),
              header(s('colRegion'), width: _region),
              header(s('colRoutes'), flex: 2),
              header(s('colLatency'), width: _latency, align: TextAlign.end),
            ],
          ),
        ),
        Expanded(
          child: ValueListenableBuilder(
            valueListenable: app.latency,
            builder: (context, latency, _) => ListView.builder(
              padding: const EdgeInsets.symmetric(horizontal: 16),
              itemCount: nodes.length,
              itemExtent: Tokens.rowMinHeight,
              itemBuilder: (context, index) {
                final node = nodes[index];
                final isCurrent = node.id == current;
                final isSelected = node.id == selected;
                final background = isSelected
                    ? t.accent
                    : index.isOdd
                    ? t.border.withValues(alpha: 0.04)
                    : null;
                final fg = isSelected ? t.onAccent : null;
                return Semantics(
                  selected: isSelected,
                  button: true,
                  label: node.name,
                  child: GestureDetector(
                    onTap: () => onSelect(node.id),
                    onDoubleTap: () => app.selectNode(node.id),
                    child: Container(
                      padding: const EdgeInsets.symmetric(horizontal: 8),
                      decoration: BoxDecoration(color: background, borderRadius: BorderRadius.circular(5)),
                      child: Row(
                        children: [
                          SizedBox(
                            width: _mark,
                            child: isCurrent ? Icon(FLucideIcons.check, size: 13, color: fg ?? t.accent) : null,
                          ),
                          Expanded(
                            flex: 3,
                            child: Text(
                              node.name,
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: (isCurrent ? t.nodeName : t.rowTitle).copyWith(color: fg),
                            ),
                          ),
                          SizedBox(width: _entry, child: _Tier(label: node.entryLabel, onAccent: isSelected)),
                          SizedBox(
                            width: _region,
                            child: Text(
                              node.exitRegion ?? '',
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: t.secondary.copyWith(color: fg),
                            ),
                          ),
                          Expanded(
                            flex: 2,
                            child: Text(
                              node.lines.join(' → '),
                              maxLines: 1,
                              overflow: TextOverflow.ellipsis,
                              style: t.mono.copyWith(color: fg ?? t.textSecondary),
                            ),
                          ),
                          SizedBox(
                            width: _latency,
                            child: Align(
                              alignment: Alignment.centerRight,
                              child: LatencyLabel(latency: latency[node.id]),
                            ),
                          ),
                        ],
                      ),
                    ),
                  ),
                );
              },
            ),
          ),
        ),
      ],
    );
  }
}

class _Tier extends StatelessWidget {
  const _Tier({required this.label, required this.onAccent});

  final String? label;
  final bool onAccent;

  @override
  Widget build(BuildContext context) {
    final text = label;
    if (text == null || text.isEmpty) return const SizedBox();
    final t = Tokens.of(context);
    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        padding: const EdgeInsets.symmetric(horizontal: 5, vertical: 1),
        decoration: BoxDecoration(
          color: onAccent ? t.onAccent.withValues(alpha: 0.2) : t.accentSoft,
          borderRadius: BorderRadius.circular(4),
        ),
        child: Text(
          text,
          maxLines: 1,
          overflow: TextOverflow.ellipsis,
          style: t.note.copyWith(color: onAccent ? t.onAccent : t.accent),
        ),
      ),
    );
  }
}

class _Footer extends StatelessWidget {
  const _Footer({required this.app, required this.node, required this.count, required this.testing});

  final AppState app;
  final rust.UiNode? node;
  final int count;
  final bool testing;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 24, vertical: 8),
      decoration: BoxDecoration(border: Border(top: BorderSide(color: t.border, width: 0.5))),
      child: ValueListenableBuilder(
        valueListenable: app.proxies,
        builder: (context, proxies, _) {
          final proxy = proxies[node?.id];
          return Row(
            children: [
              if (node != null && proxy != null) ...[
                Flexible(
                  child: Text(
                    s('nodeProxyT', {'n': node!.name}),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.rowTitle,
                  ),
                ),
                const SizedBox(width: 8),
                Text(proxy.username, style: t.mono.copyWith(color: t.textSecondary)),
                const SizedBox(width: 8),
                CopyButton(text: ProxyRows.url(proxy, 'http'), label: 'HTTP'),
                const SizedBox(width: 4),
                CopyButton(text: ProxyRows.url(proxy, 'socks5h'), label: 'SOCKS5'),
              ],
              const Spacer(),
              Text(
                [s('nodeCount', {'n': '$count'}), if (testing) s('testing')].join(' · '),
                style: t.note,
              ),
            ],
          );
        },
      ),
    );
  }
}

class _InvalidEmpty extends StatelessWidget {
  const _InvalidEmpty();

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(
      child: Padding(
        padding: Tokens.contentPadding,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(FLucideIcons.circleAlert, size: 44, color: t.warning),
            const SizedBox(height: 12),
            Text(S.current('invalidT'), style: t.statusTitle),
            const SizedBox(height: 6),
            Text(S.current('invalidD'), style: t.secondary, textAlign: TextAlign.center),
          ],
        ),
      ),
    );
  }
}
