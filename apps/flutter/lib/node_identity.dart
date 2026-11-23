// Shared node hierarchy for phone and desktop: name, then the exit address.
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import 'src/rust/api.dart' as rust;
import 'strings.dart';
import 'theme.dart';
import 'widgets.dart';

({String name, String? address}) nodeIdentity(rust.UiNode node) {
  final title = nodeTitle(node);
  final parts = title.split(RegExp(r'\s+'));
  var index = parts.length - 1;
  // The backend may append a replica marker after the address.
  if (RegExp(r'^[①-⑳]$|^\([0-9]+\)$').hasMatch(parts[index]) && index > 0)
    index--;
  final address = InternetAddress.tryParse(
    parts[index].replaceAll(RegExp(r'^\[|\]$'), ''),
  );
  if (address == null || parts.length == 1) return (name: title, address: null);
  return (
    name: [
      for (var i = 0; i < parts.length; i++)
        if (i != index) parts[i],
    ].join(' '),
    address: address.address,
  );
}

// These are the backend memberlink naming contract's localized exit classes.
// Unknown names remain unchanged instead of being guessed from arbitrary text.
final _exitClass = RegExp(
  r'(?:^|\s)(Native IP|Residential IP|Dual ISP|原生 IP|家宽 IP|双 ISP)(?=\s|$)',
);
String? nodeKind(rust.UiNode node) =>
    _exitClass.firstMatch(nodeIdentity(node).name)?.group(1);
String nodeDisplayName(rust.UiNode node) {
  final name = nodeIdentity(node).name;
  final kind = nodeKind(node);
  final plain = kind == null
      ? name
      : name.replaceFirst(kind, '').replaceAll(RegExp(r'\s+'), ' ').trim();
  return plain.isEmpty ? name : plain;
}

final _entryClass = RegExp(r'(?:^|\s)(CN Optimized|Economy|三网优化|经济)(?=\s|$)');

/// Entry type label: prefers the profile's `entry_label`, falls back to a
/// known entry word in the node name, then finalized `entry_key` mappings.
String? nodeEntryLabel(rust.UiNode node) {
  final explicit = node.entryLabel?.trim();
  if (explicit != null && explicit.isNotEmpty) return explicit;
  final matched = _entryClass.firstMatch(nodeIdentity(node).name)?.group(1);
  if (matched != null) return matched;
  return switch (node.entryKey.trim()) {
    'cn-optimized' => S.current('entryCnOptimized'),
    'lite' || 'lite-direct' => S.current('entryEconomy'),
    _ => null,
  };
}

/// Node label for native tray/menu-bar items and accessibility labels,
/// appending the entry type when not already present in the title so nodes
/// sharing an IP and name remain distinguishable.
String trayNodeTitle(rust.UiNode node) {
  final title = nodeTitle(node);
  final entry = nodeEntryLabel(node);
  if (entry == null || title.contains(entry)) return title;
  return '$title · $entry';
}

List<String> nodeTagLabels(
  rust.UiNode node, {
  bool type = true,
  bool includeEntry = false,
}) {
  final kind = nodeKind(node);
  final entry = nodeEntryLabel(node);
  if (!type) return [entry ?? S.current('nodeDetails')];
  if (kind != null) {
    return [kind, if (includeEntry && entry != null && entry != kind) entry];
  }
  return [entry ?? S.current('nodeDetails')];
}

class NodeIdentity extends StatelessWidget {
  const NodeIdentity({
    super.key,
    required this.node,
    this.phone = false,
    this.strong = false,
    this.tag = false,
  });
  final rust.UiNode node;
  final bool phone;
  final bool strong;
  final bool tag;
  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final value = nodeIdentity(node);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      mainAxisSize: MainAxisSize.min,
      children: [
        if (tag || (!phone && nodeKind(node) != null))
          LayoutBuilder(
            builder: (context, box) {
              final dual = nodeTagLabels(node, includeEntry: tag).length > 1;
              return Row(
                crossAxisAlignment: CrossAxisAlignment.center,
                children: [
                  Flexible(
                    child: Text(
                      nodeDisplayName(node),
                      maxLines: 1,
                      overflow: TextOverflow.ellipsis,
                      style: (phone ? t.phoneBody : t.body).copyWith(
                        fontWeight: strong ? FontWeight.w600 : FontWeight.w500,
                        height: 1.2,
                        leadingDistribution: TextLeadingDistribution.even,
                      ),
                    ),
                  ),
                  const SizedBox(width: 6),
                  ConstrainedBox(
                    constraints: BoxConstraints(
                      maxWidth: box.maxWidth * (dual ? 0.62 : 0.45),
                    ),
                    child: NodeInfoTag(node: node, includeEntry: tag),
                  ),
                ],
              );
            },
          )
        else
          Text(
            nodeDisplayName(node),
            maxLines: phone ? 2 : 1,
            overflow: TextOverflow.ellipsis,
            style: (phone ? t.phoneBody : t.body).copyWith(
              fontWeight: strong ? FontWeight.w600 : FontWeight.w500,
              height: 1.2,
              leadingDistribution: TextLeadingDistribution.even,
            ),
          ),
        if (value.address != null)
          Padding(
            padding: const EdgeInsets.only(top: 3),
            child: Text(
              value.address!,
              maxLines: 1,
              overflow: TextOverflow.ellipsis,
              style: t.monoText(12, color: t.t3),
            ),
          ),
      ],
    );
  }
}

class NodeInfoTag extends StatelessWidget {
  const NodeInfoTag({
    super.key,
    required this.node,
    this.type = true,
    this.includeEntry = false,
  });
  final bool type;
  final bool includeEntry;
  final rust.UiNode node;
  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final value = nodeIdentity(node);
    final entry = nodeEntryLabel(node);
    final labels = nodeTagLabels(node, type: type, includeEntry: includeEntry);
    Widget chip(String text) => Container(
      padding: const EdgeInsets.symmetric(horizontal: 7, vertical: 3),
      decoration: BoxDecoration(
        color: t.accentSoft,
        borderRadius: BorderRadius.circular(5),
      ),
      child: Text(
        text,
        maxLines: 1,
        overflow: TextOverflow.ellipsis,
        style: t.label.copyWith(
          color: t.accent,
          height: 1.2,
          leadingDistribution: TextLeadingDistribution.even,
        ),
      ),
    );
    return FPopover(
      popoverBuilder: (context, controller) => SizedBox(
        width: 240,
        child: Padding(
          padding: const EdgeInsets.all(14),
          child: Column(
            mainAxisSize: MainAxisSize.min,
            crossAxisAlignment: CrossAxisAlignment.start,
            children: [
              Text(value.name, style: t.bodyStrong),
              if (value.address != null) ...[
                const SizedBox(height: 8),
                Text('${s('nodeAddress')}: ${value.address}', style: t.note),
              ],
              if (entry != null) ...[
                const SizedBox(height: 8),
                Text(entry, style: t.note),
              ],
              if (linesText(node).isNotEmpty) ...[
                const SizedBox(height: 8),
                Text(linesText(node), style: t.note),
              ],
            ],
          ),
        ),
      ),
      builder: (context, controller, child) => Pressable(
        label: s('nodeDetails'),
        onPress: controller.toggle,
        builder: (context, pressed) => labels.length == 1
            ? chip(labels.single)
            : Row(
                mainAxisSize: MainAxisSize.min,
                children: [
                  for (final (i, label) in labels.indexed) ...[
                    if (i > 0) const SizedBox(width: 4),
                    Flexible(child: chip(label)),
                  ],
                ],
              ),
      ),
    );
  }
}

/// Single-line controls keep the same name/address hierarchy without changing
/// the desktop picker height or introducing a nested popover into its menu.
class NodeInlineName extends StatelessWidget {
  const NodeInlineName({super.key, required this.node, this.style});
  final rust.UiNode node;
  final TextStyle? style;
  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final value = nodeIdentity(node);
    return Text.rich(
      TextSpan(
        children: [
          TextSpan(text: value.name),
          if (value.address != null)
            TextSpan(
              text: '  ${value.address}',
              style: t.monoText(12, color: t.t3),
            ),
        ],
      ),
      maxLines: 1,
      overflow: TextOverflow.ellipsis,
      style: style ?? t.bodyStrong,
    );
  }
}
