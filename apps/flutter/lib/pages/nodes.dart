// Nodes (design DesktopNodes): the tool row (test method, refresh, test
// all), the table card (current mark, node with flag, tier tag, region,
// lines in failover order, latency pill) and the footer card with the
// selected node's local proxy. Click selects a row, double-click makes it
// the current node. Not in the prototype: the search field and the row
// context menu.

import 'package:flutter/widgets.dart';

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
              _Tools(
                method: _method,
                onMethod: (method) => setState(() => _method = method),
                testing: _testing,
                canTest: snapshot.standardReady,
                onTestAll: _testAll,
                onRefresh: app.refreshNodes,
              ),
              Expanded(
                child: Padding(
                  padding: const EdgeInsets.symmetric(horizontal: Tokens.pageInset),
                  child: _Table(
                    app: app,
                    nodes: nodes,
                    current: snapshot.selectedNodeId,
                    selected: selectedRow,
                    onSelect: (id) => setState(() => _selectedRow = id),
                  ),
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

class _Tools extends StatelessWidget {
  const _Tools({
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
    Widget segment(rust.UiProbeMethod value, String label) {
      final on = value == method;
      return Pressable(
        onPress: () => onMethod(value),
        selected: on,
        label: label,
        radius: 6,
        builder: (context, hovered) => Container(
          height: 26,
          padding: const EdgeInsets.symmetric(horizontal: 11),
          alignment: Alignment.center,
          decoration: BoxDecoration(
            color: on ? t.card : null,
            borderRadius: BorderRadius.circular(6),
            boxShadow: on ? const [BoxShadow(color: Color(0x140F172A), offset: Offset(0, 1), blurRadius: 2)] : null,
          ),
          child: Text(
            label,
            style: t.note.copyWith(
              fontWeight: on ? FontWeight.w600 : FontWeight.w500,
              color: on ? t.t1 : t.t2,
            ),
          ),
        ),
      );
    }

    return Padding(
      padding: const EdgeInsets.fromLTRB(Tokens.pageInset, 0, Tokens.pageInset, 12),
      child: Row(
        children: [
          Semantics(
            label: s('probe'),
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
                children: [
                  segment(rust.UiProbeMethod.icmp, 'Ping'),
                  segment(rust.UiProbeMethod.tcp, 'TCP'),
                  segment(rust.UiProbeMethod.connect, 'Connect'),
                ],
              ),
            ),
          ),
          const Spacer(),
          Button(kind: ButtonKind.secondary, label: s('refreshNodes'), onPress: onRefresh),
          const SizedBox(width: 8),
          Button(
            kind: ButtonKind.primary,
            height: Tokens.controlHeight,
            label: s('testAll'),
            onPress: testing || !canTest ? null : onTestAll,
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

  // Columns: mark 20, node (flex), tier 52, region 68, lines 150, latency
  // 84; 12 apart.
  static Widget _cells(List<Widget> c) => Row(
    children: [
      SizedBox(width: 20, child: c[0]),
      const SizedBox(width: 12),
      Expanded(child: c[1]),
      const SizedBox(width: 12),
      SizedBox(width: 52, child: c[2]),
      const SizedBox(width: 12),
      SizedBox(width: 68, child: c[3]),
      const SizedBox(width: 12),
      SizedBox(width: 150, child: c[4]),
      const SizedBox(width: 12),
      SizedBox(width: 84, child: Align(alignment: Alignment.centerRight, child: c[5])),
    ],
  );

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final head = t.note.copyWith(fontWeight: FontWeight.w500);
    return CardBox(
      child: Column(
        children: [
          Container(
            height: Tokens.tableHeaderHeight,
            padding: const EdgeInsets.symmetric(horizontal: 16),
            decoration: BoxDecoration(
              color: t.sunk,
              border: Border(bottom: BorderSide(color: t.hair)),
            ),
            child: _cells([
              const SizedBox(),
              Text('${s('colName')} · ${nodes.length}', style: head),
              Text(s('colTier'), style: head),
              Text(s('colRegion'), style: head),
              Text(s('colRoutes'), style: head),
              Text(s('colLatency'), style: head),
            ]),
          ),
          Expanded(
            child: ValueListenableBuilder(
              valueListenable: app.latency,
              builder: (context, latency, _) => ListView.builder(
                itemCount: nodes.length,
                itemExtent: Tokens.tableRowHeight,
                itemBuilder: (context, index) {
                  final node = nodes[index];
                  final isCurrent = node.id == current;
                  final isSelected = node.id == selected;
                  return Pressable(
                    radius: 0,
                    selected: isSelected,
                    label: nodeTitle(node),
                    onPress: () => onSelect(node.id),
                    onDoublePress: () => app.selectNode(node.id),
                    builder: (context, hovered) => Container(
                      padding: const EdgeInsets.symmetric(horizontal: 16),
                      decoration: BoxDecoration(
                        color: isSelected ? t.accentSoft : (hovered ? t.sunk : null),
                        border: Border(
                          top: index == 0 ? BorderSide.none : BorderSide(color: t.hair),
                          left: isSelected ? BorderSide(color: t.accent, width: 3) : BorderSide.none,
                        ),
                      ),
                      child: _cells([
                        Text(isCurrent ? '✓' : '', style: t.bodyStrong.copyWith(color: t.accent, fontSize: 13)),
                        Row(
                          children: [
                            Flag(countryCode: node.exitCountryCode),
                            const SizedBox(width: 10),
                            Flexible(
                              child: Text(
                                nodeTitle(node),
                                maxLines: 1,
                                overflow: TextOverflow.ellipsis,
                                style: t.body.copyWith(
                                  fontSize: 13,
                                  fontWeight: isCurrent ? FontWeight.w600 : FontWeight.w400,
                                ),
                              ),
                            ),
                          ],
                        ),
                        _Tier(label: node.entryLabel, entryKey: node.entryKey),
                        Text(
                          node.exitRegion ?? '',
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: t.body.copyWith(fontSize: 13, color: t.t2),
                        ),
                        Text(
                          node.lines.join(' → '),
                          maxLines: 1,
                          overflow: TextOverflow.ellipsis,
                          style: t.monoText(12, color: t.t2),
                        ),
                        LatencyPill(latency: latency[node.id]),
                      ]),
                    ),
                  );
                },
              ),
            ),
          ),
        ],
      ),
    );
  }
}

/// The entry tier tag (20 high, 11/600): optimised entries teal, the rest
/// (专线) accent.
class _Tier extends StatelessWidget {
  const _Tier({required this.label, required this.entryKey});

  final String? label;
  final String entryKey;

  @override
  Widget build(BuildContext context) {
    final text = label;
    if (text == null || text.isEmpty) return const SizedBox();
    final t = Tokens.of(context);
    final teal = entryKey.contains('optim');
    final color = teal ? Tokens.tierOptimized : t.accent;
    return Align(
      alignment: Alignment.centerLeft,
      child: Container(
        height: 20,
        padding: const EdgeInsets.symmetric(horizontal: 7),
        alignment: Alignment.center,
        decoration: BoxDecoration(
          color: teal ? Tokens.tierOptimized.withValues(alpha: 0.10) : t.accentSoft,
          borderRadius: BorderRadius.circular(5),
        ),
        child: Text(text, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.label.copyWith(color: color)),
      ),
    );
  }
}

/// The footer card (40 high): the selected node's local proxy, user name,
/// copy buttons; the node count and testing note at the end.
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
    return Padding(
      padding: const EdgeInsets.fromLTRB(Tokens.pageInset, 12, Tokens.pageInset, 16),
      child: Container(
        height: 40,
        padding: const EdgeInsets.fromLTRB(14, 0, 6, 0),
        decoration: t.cardDecoration().copyWith(borderRadius: BorderRadius.circular(10)),
        child: ValueListenableBuilder(
          valueListenable: app.proxies,
          builder: (context, proxies, _) {
            final proxy = proxies[node?.id];
            final selected = node;
            return Row(
              children: [
                if (selected != null && proxy != null) ...[
                  Flexible(
                    child: Text(
                      s('nodeProxyT', {'n': nodeTitle(selected)}),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: t.note.copyWith(color: t.t1, fontWeight: FontWeight.w500),
                    ),
                  ),
                  const SizedBox(width: 12),
                  Text(proxy.username, style: t.monoText(12, color: t.t3)),
                ] else
                  Text(
                    [s('nodeCount', {'n': '$count'}), if (testing) s('testing')].join(' · '),
                    style: t.note.copyWith(color: t.t2),
                  ),
                const Spacer(),
                if (proxy != null) ...[
                  CopyButton(
                    text: ProxyRows.url(proxy, 'http'),
                    label: s('copyHttpShort'),
                    kind: ButtonKind.soft,
                    height: 26,
                  ),
                  const SizedBox(width: 6),
                  CopyButton(
                    text: ProxyRows.url(proxy, 'socks5h'),
                    label: s('copySocksShort'),
                    kind: ButtonKind.soft,
                    height: 26,
                  ),
                ],
              ],
            );
          },
        ),
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
        padding: Tokens.pagePadding,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Text(S.current('invalidT'), style: t.heroTitle),
            const SizedBox(height: 6),
            Text(S.current('invalidD'), style: t.sub, textAlign: TextAlign.center),
          ],
        ),
      ),
    );
  }
}
