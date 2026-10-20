// Overview (handoff "2. 概览"): the invalid-configuration warning, the
// connection card, the problem notice, live traffic and the local proxy;
// a full empty state instead while access is limited. The prototype has
// the system proxy (standard mode) only: the enhanced-mode row is shown,
// switched off and disabled.

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';

/// Busy phases, in which the switch shows its in-between state.
bool isBusy(rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.preparing ||
  rust.UiPhase.waitingPermission ||
  rust.UiPhase.connecting ||
  rust.UiPhase.reconnecting ||
  rust.UiPhase.disconnecting => true,
  _ => false,
};

/// The status title for the system proxy (handoff "状态标题规则").
String statusTitle(rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.on => S.current('h_on'),
  rust.UiPhase.disconnecting => S.current('h_stdStopping'),
  rust.UiPhase.error || rust.UiPhase.contended => S.current('h_stdFail'),
  _ when isBusy(phase) => S.current('h_stdStarting'),
  _ => S.current('h_idle'),
};

Color statusColor(Tokens t, rust.UiPhase phase) => switch (phase) {
  rust.UiPhase.on => t.success,
  rust.UiPhase.error || rust.UiPhase.contended => t.danger,
  _ when isBusy(phase) => t.accent,
  _ => t.idle,
};

class OverviewPage extends StatelessWidget {
  const OverviewPage({super.key, required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final limited = LimitedAccess.of(snapshot);
      if (limited != null) return limited;
      if (snapshot.profile == rust.UiProfile.loading) return const LoadingNodes();
      return SingleChildScrollView(
        padding: Tokens.contentPadding,
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.stretch,
          children: [
            if (snapshot.profile == rust.UiProfile.invalid) ...[
              const ConfigWarning(),
              const SizedBox(height: Tokens.sectionGap),
            ],
            _ConnectionCard(app: app, snapshot: snapshot),
            if (snapshot.phase == rust.UiPhase.error) ...[
              const SizedBox(height: Tokens.sectionGap),
              _Problem(app: app, error: snapshot.error),
            ],
            const SizedBox(height: Tokens.sectionGap),
            _Traffic(app: app),
            const SizedBox(height: Tokens.sectionGap),
            _LocalProxy(app: app, nodeId: snapshot.selectedNodeId),
          ],
        ),
      );
    },
  );
}

class _ConnectionCard extends StatelessWidget {
  const _ConnectionCard({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final phase = snapshot.phase;
    final busy = isBusy(phase);
    final color = statusColor(t, phase);
    final node = app.nodeById(snapshot.selectedNodeId);
    final detail = phase == rust.UiPhase.off
        ? s('d_idle')
        : [
            ?node?.name,
            ?snapshot.line,
            if (snapshot.latencyMs case final ms?) '$ms ms',
          ].join(' · ');
    return Group(
      padding: const EdgeInsets.fromLTRB(14, 14, 14, 6),
      child: Column(
        crossAxisAlignment: CrossAxisAlignment.stretch,
        children: [
          Row(
            children: [
              Container(
                width: Tokens.statusIconSize,
                height: Tokens.statusIconSize,
                alignment: Alignment.center,
                decoration: BoxDecoration(color: color.withValues(alpha: 0.14), shape: BoxShape.circle),
                child: busy
                    ? Spinner(color: color, size: 20)
                    : Icon(
                        phase == rust.UiPhase.on
                            ? FLucideIcons.shieldCheck
                            : phase == rust.UiPhase.error
                            ? FLucideIcons.circleAlert
                            : FLucideIcons.link2Off,
                        size: 22,
                        color: color,
                      ),
              ),
              const SizedBox(width: 12),
              Expanded(
                child: Column(
                  crossAxisAlignment: CrossAxisAlignment.start,
                  children: [
                    Semantics(
                      header: true,
                      liveRegion: true,
                      child: Text(statusTitle(phase), style: t.statusTitle),
                    ),
                    const SizedBox(height: 2),
                    Text(detail, style: t.secondary, maxLines: 1, overflow: TextOverflow.ellipsis),
                  ],
                ),
              ),
            ],
          ),
          const SizedBox(height: 10),
          _Row(
            title: s('stdMode'),
            description: s('stdDesc'),
            trailing: TriSwitch(
              label: s('stdMode'),
              state: phase == rust.UiPhase.on
                  ? SwitchState.on
                  : busy
                  ? SwitchState.busy
                  : SwitchState.off,
              // Not while preparing, waiting for permission or disconnecting.
              enabled:
                  phase != rust.UiPhase.preparing &&
                  phase != rust.UiPhase.waitingPermission &&
                  phase != rust.UiPhase.disconnecting,
              onChanged: (on) => on ? app.connect() : app.disconnect(),
            ),
          ),
          _Row(
            title: s('tunMode'),
            description: s('protoNoTun'),
            trailing: TriSwitch(label: s('tunMode'), state: SwitchState.off, enabled: false, onChanged: null),
          ),
          _Row(title: s('currentNode'), trailing: NodePicker(app: app, selected: node)),
        ],
      ),
    );
  }
}

/// A form row: title, the weakest-colour description in one line, and a
/// control at the end.
class _Row extends StatelessWidget {
  const _Row({required this.title, this.description, required this.trailing});

  final String title;
  final String? description;
  final Widget trailing;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      constraints: const BoxConstraints(minHeight: Tokens.rowMinHeight + 8),
      decoration: BoxDecoration(border: Border(top: BorderSide(color: t.border, width: 0.5))),
      padding: const EdgeInsets.symmetric(vertical: 6),
      child: Row(
        children: [
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(title, style: t.rowTitle),
                if (description case final text?)
                  Text(text, style: t.note, maxLines: 1, overflow: TextOverflow.ellipsis),
              ],
            ),
          ),
          trailing,
        ],
      ),
    );
  }
}

/// The current-node pop-up: name and latency per node.
class NodePicker extends StatelessWidget {
  const NodePicker({super.key, required this.app, required this.selected});

  final AppState app;
  final rust.UiNode? selected;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.nodes,
    builder: (context, nodes, _) => FPopoverMenu(
      maxHeight: 320,
      menuAnchor: Alignment.topRight,
      childAnchor: Alignment.bottomRight,
      menu: [
        FItemGroup(
          children: [
            for (final node in nodes)
              FItem(
                selected: node.id == selected?.id,
                prefix: node.id == selected?.id ? const Icon(FLucideIcons.check, size: 14) : const SizedBox(width: 14),
                title: Text(node.name, maxLines: 1, overflow: TextOverflow.ellipsis),
                suffix: ValueListenableBuilder(
                  valueListenable: app.latency,
                  builder: (context, latency, _) => LatencyLabel(latency: latency[node.id]),
                ),
                onPress: () => app.selectNode(node.id),
              ),
          ],
        ),
      ],
      builder: (context, controller, _) => SizedBox(
        height: Tokens.controlHeight + 4,
        child: FButton(
          variant: FButtonVariant.outline,
          size: FButtonSizeVariant.sm,
          mainAxisSize: MainAxisSize.min,
          onPress: controller.toggle,
          suffix: const Icon(FLucideIcons.chevronDown, size: 12),
          child: ConstrainedBox(
            constraints: const BoxConstraints(maxWidth: 240),
            child: Text(selected?.name ?? '—', maxLines: 1, overflow: TextOverflow.ellipsis),
          ),
        ),
      ),
    ),
  );
}

class _Problem extends StatelessWidget {
  const _Problem({required this.app, required this.error});

  final AppState app;
  final rust.UiError? error;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final reason = error?.code == 'SystemProxyFailed' || error == null ? s('fr_port') : error!.code;
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(color: t.dangerSoft, borderRadius: BorderRadius.circular(Tokens.groupRadius)),
      child: Row(
        children: [
          Icon(FLucideIcons.circleAlert, size: 16, color: t.danger),
          const SizedBox(width: 8),
          Expanded(
            child: Column(
              crossAxisAlignment: CrossAxisAlignment.start,
              children: [
                Text(s('h_stdFail'), style: t.rowTitle.copyWith(fontWeight: FontWeight.w600)),
                Text(reason, style: t.note),
              ],
            ),
          ),
          SizedBox(
            height: Tokens.controlHeight + 4,
            child: FButton(
              size: FButtonSizeVariant.sm,
              mainAxisSize: MainAxisSize.min,
              onPress: app.retry,
              child: Text(s('retry')),
            ),
          ),
        ],
      ),
    );
  }
}

class _Traffic extends StatelessWidget {
  const _Traffic({required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    Widget tile(String label, IconData icon, int rate) => Expanded(
      child: Group(
        padding: const EdgeInsets.all(12),
        child: Column(
          crossAxisAlignment: CrossAxisAlignment.start,
          children: [
            Row(
              children: [
                Icon(icon, size: 12, color: t.textSecondary),
                const SizedBox(width: 4),
                Text(label, style: t.secondary),
              ],
            ),
            const SizedBox(height: 4),
            Text(formatRate(rate), style: t.rate),
          ],
        ),
      ),
    );
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(s('traffic'), style: t.sectionTitle),
        Text(s('trafficNote'), style: t.note),
        const SizedBox(height: 8),
        ValueListenableBuilder(
          valueListenable: app.traffic,
          builder: (context, sample, _) => Row(
            children: [
              tile(s('upload'), FLucideIcons.arrowUp, sample.up),
              const SizedBox(width: 10),
              tile(s('download'), FLucideIcons.arrowDown, sample.down),
            ],
          ),
        ),
      ],
    );
  }
}

class _LocalProxy extends StatelessWidget {
  const _LocalProxy({required this.app, required this.nodeId});

  final AppState app;
  final String? nodeId;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    return Column(
      crossAxisAlignment: CrossAxisAlignment.start,
      children: [
        Text(s('localProxy'), style: t.sectionTitle),
        Text(s('proxyNote'), style: t.note),
        const SizedBox(height: 8),
        ValueListenableBuilder(
          valueListenable: app.proxies,
          builder: (context, proxies, _) {
            final proxy = proxies[nodeId];
            if (proxy == null) {
              return Group(
                padding: const EdgeInsets.all(12),
                child: Text('—', style: t.secondary),
              );
            }
            return Group(child: ProxyRows(proxy: proxy));
          },
        ),
      ],
    );
  }
}

/// HTTP and SOCKS5 with their copy buttons, then user name and password.
/// The copied URL carries the credentials.
class ProxyRows extends StatelessWidget {
  const ProxyRows({super.key, required this.proxy});

  final rust.UiLocalProxy proxy;

  static String url(rust.UiLocalProxy proxy, String scheme) {
    final user = Uri.encodeComponent(proxy.username);
    final password = Uri.encodeComponent(proxy.password);
    return '$scheme://$user:$password@${proxy.host}:${proxy.port}';
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    Widget row(String label, Widget value, [Widget? trailing]) => Container(
      constraints: const BoxConstraints(minHeight: Tokens.rowMinHeight + 4),
      child: Row(
        children: [
          SizedBox(width: 72, child: Text(label, style: t.secondary)),
          Expanded(child: value),
          ?trailing,
        ],
      ),
    );
    final endpoint = '${proxy.host}:${proxy.port}';
    return Column(
      children: [
        row('HTTP', Text('http://$endpoint', style: t.mono), CopyButton(text: url(proxy, 'http'))),
        row('SOCKS5', Text('socks5://$endpoint', style: t.mono), CopyButton(text: url(proxy, 'socks5h'))),
        row(S.current('authUser'), Text(proxy.username, style: t.mono)),
        row(S.current('authPass'), Text('••••••', style: t.mono)),
      ],
    );
  }
}

/// Access limited: no subscription, subscription expired, team disabled
/// (a full empty state replaces the page).
class LimitedAccess extends StatelessWidget {
  const LimitedAccess._({required this.icon, required this.title, required this.description});

  final IconData icon;
  final String title;
  final String description;

  static Widget? of(rust.UiSnapshot snapshot) {
    final s = S.current;
    return switch (snapshot.profile) {
      rust.UiProfile.noSubscription => LimitedAccess._(
        icon: FLucideIcons.shield,
        title: s('noSubT'),
        description: s('noSubD'),
      ),
      rust.UiProfile.subscriptionExpired => LimitedAccess._(
        icon: FLucideIcons.clock,
        title: s('expiredT'),
        description: s('expiredD', {'d': snapshot.expiresAt ?? '—'}),
      ),
      rust.UiProfile.teamDisabled => LimitedAccess._(
        icon: FLucideIcons.circleAlert,
        title: s('teamOffT'),
        description: s('teamOffD', {'team': snapshot.teamName ?? '—'}),
      ),
      _ => null,
    };
  }

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(
      child: Padding(
        padding: Tokens.contentPadding,
        child: Column(
          mainAxisSize: MainAxisSize.min,
          children: [
            Icon(icon, size: 44, color: t.idle),
            const SizedBox(height: 12),
            Semantics(header: true, child: Text(title, style: t.statusTitle, textAlign: TextAlign.center)),
            const SizedBox(height: 6),
            Text(description, style: t.secondary, textAlign: TextAlign.center),
          ],
        ),
      ),
    );
  }
}

class LoadingNodes extends StatelessWidget {
  const LoadingNodes({super.key});

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Center(
      child: Column(
        mainAxisSize: MainAxisSize.min,
        children: [
          Spinner(color: t.accent, size: 24),
          const SizedBox(height: 12),
          Text(S.current('loadingT'), style: t.sectionTitle),
          const SizedBox(height: 4),
          Text(S.current('loadingD'), style: t.secondary),
        ],
      ),
    );
  }
}

class ConfigWarning extends StatelessWidget {
  const ConfigWarning({super.key});

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Container(
      padding: const EdgeInsets.symmetric(horizontal: 12, vertical: 8),
      decoration: BoxDecoration(color: t.warningSoft, borderRadius: BorderRadius.circular(Tokens.groupRadius)),
      child: Row(
        children: [
          Icon(FLucideIcons.circleAlert, size: 16, color: t.warning),
          const SizedBox(width: 8),
          Expanded(
            child: Text.rich(
              TextSpan(
                children: [
                  TextSpan(text: '${S.current('cfgT')} · ', style: t.rowTitle.copyWith(fontWeight: FontWeight.w600)),
                  TextSpan(text: S.current('cfgD'), style: t.secondary),
                ],
              ),
            ),
          ),
        ],
      ),
    );
  }
}
