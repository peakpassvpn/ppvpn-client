// 我的 (#448 step 5): the account card (radius 18 on the brand gradient; a
// press switches teams), then grouped 50-high rows with 17 titles and the
// accent-tinted icon tiles: 连接 (routing), 通用 (appearance, language), 支持
// (日志, pushed; the web pages ↗, account deletion among them, with the
// version on 关于), and 退出登录 in red after a confirmation. A row with a
// value pushes its choices.

import 'dart:async';
import 'dart:io';

import 'package:flutter/widgets.dart';
import 'package:forui/forui.dart';

import '../app_state.dart';
import '../links.dart';
import '../pages/logs.dart';
import '../pages/settings.dart';
import '../settings.dart';
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'phone.dart';

class PhoneMe extends StatelessWidget {
  const PhoneMe({super.key, required this.app});

  final AppState app;

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: app.snapshot,
    builder: (context, snapshot, _) {
      final s = S.current;
      return ListView(
        padding: phoneScrollPadding(context),
        children: [
          _AccountCard(app: app, snapshot: snapshot),
          _Group(
            title: s('connection'),
            rows: [
              _Row(
                icon: FLucideIcons.split,
                title: s('routing'),
                value: snapshot.routingMode == rust.UiRoutingMode.rules ? s('rules') : s('global'),
                onPress: () => _choose<rust.UiRoutingMode>(
                  context,
                  title: s('routing'),
                  options: [(rust.UiRoutingMode.rules, s('rules')), (rust.UiRoutingMode.global, s('global'))],
                  value: snapshot.routingMode,
                  onChanged: (mode) => unawaited(app.setRoutingMode(mode)),
                ),
              ),
            ],
          ),
          _Group(
            title: s('general'),
            rows: [
              ValueListenableBuilder(
                valueListenable: Settings.appearance,
                builder: (context, appearance, _) {
                  final options = [
                    (rust.UiAppearance.system, s('sys')),
                    (rust.UiAppearance.light, s('light')),
                    (rust.UiAppearance.dark, s('dark')),
                  ];
                  return _Row(
                    icon: FLucideIcons.sunMoon,
                    title: s('appearance'),
                    value: _name(options, appearance),
                    onPress: () => _choose(
                      context,
                      title: s('appearance'),
                      options: options,
                      value: appearance,
                      onChanged: Settings.setAppearance,
                    ),
                  );
                },
              ),
              ValueListenableBuilder(
                valueListenable: Settings.language,
                builder: (context, language, _) {
                  // Each language in its own name.
                  final options = [
                    (rust.UiLanguage.system, s('sys')),
                    (rust.UiLanguage.chinese, '中文'),
                    (rust.UiLanguage.english, 'English'),
                  ];
                  return _Row(
                    icon: FLucideIcons.languages,
                    title: s('language'),
                    value: _name(options, language),
                    onPress: () => _choose(
                      context,
                      title: s('language'),
                      options: options,
                      value: language,
                      onChanged: Settings.setLanguage,
                    ),
                  );
                },
              ),
            ],
          ),
          _Group(
            title: s('support'),
            rows: [
              _Row(
                icon: FLucideIcons.list,
                title: s('logs'),
                onPress: () => unawaited(
                  pushPhonePage<void>(context, PhoneScaffold(title: s('logs'), back: true, child: const LogsPage())),
                ),
              ),
              // The website's console (ppvpn-web apps/client, under /dashboard):
              // subscriptions, and tickets for help. It has no device page.
              // Not on iOS: a way to buy a plan outside in-app purchase is
              // refused by App Review (guideline 3.1.1, #473); a plan bought
              // elsewhere still works in the app.
              if (!Platform.isIOS)
                _Row(
                  icon: FLucideIcons.receipt,
                  title: s('planWeb'),
                  web: true,
                  onPress: () => _open('/dashboard/subscriptions'),
                ),
              _Row(icon: FLucideIcons.megaphone, title: s('helpWeb'), web: true, onPress: () => _open('/dashboard/tickets')),
              // The site's deletion page (signed out, it signs in first, then
              // comes back): it confirms again and deletes the account.
              // App Store and Play need it reachable from the app (#473).
              _Row(
                icon: FLucideIcons.userX,
                title: s('deleteAccountWeb'),
                web: true,
                onPress: () => _open('/account/delete'),
              ),
              _Row(
                icon: FLucideIcons.info,
                title: s('about'),
                value: Settings.version.isEmpty ? null : Settings.version,
                web: true,
                onPress: () => _open(''),
              ),
            ],
          ),
          const SizedBox(height: 24),
          _SignOut(onPress: () => unawaited(_confirmSignOut(context))),
        ],
      );
    },
  );

  /// The site's pages behind the ↗ rows, in the system browser for now (the
  /// in-app browser and the one-time sign-in link come with the web pages,
  /// #387), on the configured backend's site (#517).
  void _open(String path) {
    try {
      unawaited(openUrl(rust.siteUrl(path: path)));
    } catch (error) {
      debugPrint('siteUrl: $error');
    }
  }

  Future<void> _confirmSignOut(BuildContext context) async {
    final s = S.current;
    final confirmed = await showFDialog<bool>(
      context: context,
      builder: (context, style, animation) => FDialog(
        animation: animation,
        semanticsLabel: s('signOutQ'),
        builder: (context, style) => ConfirmContent(title: s('signOutQ'), body: s('signOutD'), confirm: s('signOut')),
      ),
    );
    if (confirmed ?? false) await app.signOut();
  }
}

String _name<T>(List<(T, String)> options, T value) =>
    options.firstWhere((option) => option.$1 == value, orElse: () => options.first).$2;

/// Pushes the choices of a row: one 50 row each, the current one checked;
/// a press sets it and goes back.
void _choose<T>(
  BuildContext context, {
  required String title,
  required List<(T, String)> options,
  required T value,
  required ValueChanged<T> onChanged,
}) => unawaited(
  pushPhonePage<void>(
    context,
    PhoneScaffold(
      title: title,
      back: true,
      child: Builder(
        builder: (context) {
          final t = Tokens.of(context);
          return ListView(
            padding: phoneScrollPadding(context),
            children: [
              Container(
                clipBehavior: Clip.antiAlias,
                decoration: t.phoneCardDecoration(),
                child: Column(
                  children: [
                    for (final (i, (option, text)) in options.indexed)
                      Pressable(
                        radius: 0,
                        selected: option == value,
                        label: text,
                        onPress: () {
                          onChanged(option);
                          unawaited(Navigator.of(context).maybePop());
                        },
                        builder: (context, _) => Container(
                          height: Tokens.phoneRow,
                          padding: const EdgeInsets.symmetric(horizontal: 16),
                          decoration: BoxDecoration(
                            border: i == 0 ? null : Border(top: BorderSide(color: t.line, width: 0.5)),
                          ),
                          child: Row(
                            children: [
                              Expanded(child: Text(text, style: t.phoneRowTitle)),
                              if (option == value) Icon(FLucideIcons.check, size: 18, color: t.accent),
                            ],
                          ),
                        ),
                      ),
                  ],
                ),
              ),
            ],
          );
        },
      ),
    ),
  ),
);

/// The account card: the avatar, the name, the team (个人 for the personal
/// space) with the days left, white on the brand gradient. A press lists
/// the teams to switch to (as the desktop's account menu, #443).
class _AccountCard extends StatelessWidget {
  const _AccountCard({required this.app, required this.snapshot});

  final AppState app;
  final rust.UiSnapshot snapshot;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    const white = Color(0xFFFFFFFF);
    final name = snapshot.accountName ?? '';
    final initial = name.isEmpty ? '?' : name.characters.first.toUpperCase();
    final days = _daysLeft(snapshot.expiresAt);
    final team = snapshot.teamName ?? '—';
    return Pressable(
      key: const ValueKey('account'),
      onPress: () => _teams(context),
      label: '${s('account')}, $name, $team',
      radius: Tokens.phoneAccountRadius,
      builder: (context, _) => Container(
        padding: const EdgeInsets.all(16),
        decoration: t.phoneCardDecoration(radius: Tokens.phoneAccountRadius, gradient: Tokens.brandGradient),
        child: Row(
          children: [
            Container(
              width: 44,
              height: 44,
              alignment: Alignment.center,
              decoration: const BoxDecoration(color: Color(0x33FFFFFF), shape: BoxShape.circle),
              child: Text(initial, style: t.phoneBodyStrong.copyWith(fontSize: 18, color: white)),
            ),
            const SizedBox(width: 12),
            Expanded(
              child: Column(
                crossAxisAlignment: CrossAxisAlignment.start,
                children: [
                  Text(
                    name.isEmpty ? '—' : name,
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.phoneBodyStrong.copyWith(color: white),
                  ),
                  const SizedBox(height: 2),
                  Text(
                    [team, if (days != null) s('daysLeft', {'n': '$days'})].join(' · '),
                    maxLines: 1,
                    overflow: TextOverflow.ellipsis,
                    style: t.phoneNote.copyWith(color: const Color(0xC7FFFFFF)),
                  ),
                ],
              ),
            ),
            Icon(FLucideIcons.chevronRight, size: 18, color: const Color(0xA6FFFFFF)),
          ],
        ),
      ),
    );
  }

  /// The teams, pushed: the current one checked, a disabled one greyed
  /// with 已停用; a press switches and goes back.
  void _teams(BuildContext context) {
    unawaited(app.refreshTeams());
    unawaited(
      pushPhonePage<void>(
        context,
        PhoneScaffold(
          title: S.current('switchTeam'),
          back: true,
          child: ValueListenableBuilder(
            valueListenable: app.teams,
            builder: (context, teams, _) {
              final t = Tokens.of(context);
              final current = app.snapshot.value.teamId;
              if (teams.isEmpty) return Center(child: Spinner(color: t.accent, size: 20));
              return ListView(
                padding: phoneScrollPadding(context),
                children: [
                  Container(
                    clipBehavior: Clip.antiAlias,
                    decoration: t.phoneCardDecoration(),
                    child: Column(
                      children: [
                        for (final (i, team) in teams.indexed)
                          Pressable(
                            key: ValueKey('team-${team.id}'),
                            radius: 0,
                            selected: team.id == current,
                            label: team.personal ? S.current('personal') : team.name,
                            onPress: !team.active || team.id == current
                                ? null
                                : () {
                                    unawaited(app.switchTeam(team.id));
                                    unawaited(Navigator.of(context).maybePop());
                                  },
                            builder: (context, _) => Container(
                              height: Tokens.phoneRow,
                              padding: const EdgeInsets.symmetric(horizontal: 16),
                              decoration: BoxDecoration(
                                border: i == 0 ? null : Border(top: BorderSide(color: t.line, width: 0.5)),
                              ),
                              child: Row(
                                children: [
                                  Expanded(
                                    child: Text(
                                      team.personal ? S.current('personal') : team.name,
                                      maxLines: 1,
                                      overflow: TextOverflow.ellipsis,
                                      style: t.phoneRowTitle,
                                    ),
                                  ),
                                  if (!team.active) Text(S.current('teamDisabled'), style: t.phoneNote),
                                  if (team.id == current) Icon(FLucideIcons.check, size: 18, color: t.accent),
                                ],
                              ),
                            ),
                          ),
                      ],
                    ),
                  ),
                ],
              );
            },
          ),
        ),
      ),
    );
  }
}

/// Whole days until [rfc3339], none when unknown or past.
int? _daysLeft(String? rfc3339) {
  final time = rfc3339 == null ? null : DateTime.tryParse(rfc3339);
  if (time == null) return null;
  final days = time.difference(DateTime.now()).inDays;
  return days < 0 ? null : days;
}

/// A group: its heading (13, t2) 24 below the previous, then its rows in
/// one card with 0.5 px dividers.
class _Group extends StatelessWidget {
  const _Group({required this.title, required this.rows});

  final String title;
  final List<Widget> rows;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Column(
      crossAxisAlignment: CrossAxisAlignment.stretch,
      children: [
        Padding(
          padding: const EdgeInsets.fromLTRB(4, 24, 4, 8),
          child: Semantics(header: true, child: Text(title, style: t.phoneNote.copyWith(color: t.t2))),
        ),
        Container(
          clipBehavior: Clip.antiAlias,
          decoration: t.phoneCardDecoration(),
          child: Column(
            children: [
              for (final (i, row) in rows.indexed)
                DecoratedBox(
                  decoration: BoxDecoration(
                    border: i == 0 ? null : Border(top: BorderSide(color: t.line, width: 0.5)),
                  ),
                  child: row,
                ),
            ],
          ),
        ),
      ],
    );
  }
}

/// A 50 row: the icon tile, the 17 title, the value in t3, then the
/// chevron, or ↗ for a web page.
class _Row extends StatelessWidget {
  const _Row({required this.icon, required this.title, required this.onPress, this.value, this.web = false});

  final IconData icon;
  final String title;
  final String? value;
  final bool web;
  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Pressable(
      radius: 0,
      onPress: onPress,
      label: value == null ? title : '$title, $value',
      builder: (context, _) => Container(
        height: Tokens.phoneRow,
        padding: const EdgeInsets.symmetric(horizontal: 14),
        child: Row(
          children: [
            RowIcon(icon: icon),
            const SizedBox(width: 12),
            Expanded(child: Text(title, maxLines: 1, overflow: TextOverflow.ellipsis, style: t.phoneRowTitle)),
            if (value case final text?) ...[
              const SizedBox(width: 8),
              Text(text, style: t.phoneBody.copyWith(color: t.t3)),
            ],
            const SizedBox(width: 6),
            Icon(web ? FLucideIcons.arrowUpRight : FLucideIcons.chevronRight, size: 18, color: t.t3),
          ],
        ),
      ),
    );
  }
}

/// 退出登录: a 50 card row, the text red and centred.
class _SignOut extends StatelessWidget {
  const _SignOut({required this.onPress});

  final VoidCallback onPress;

  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    return Pressable(
      key: const ValueKey('sign-out'),
      onPress: onPress,
      label: S.current('signOut'),
      radius: Tokens.phoneCardRadius,
      builder: (context, _) => Container(
        height: Tokens.phoneRow,
        alignment: Alignment.center,
        decoration: t.phoneCardDecoration(),
        child: Text(S.current('signOut'), style: t.phoneRowTitle.copyWith(color: t.bad)),
      ),
    );
  }
}
