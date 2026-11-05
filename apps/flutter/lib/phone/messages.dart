// Phone inbox: touch-sized rows, pull to refresh, native pushed details.
import 'dart:async';

import 'package:flutter/cupertino.dart' show CupertinoSliverRefreshControl;
import 'package:flutter/widgets.dart';
import 'package:flutter/material.dart' show SelectableText;

import '../app_state.dart';
import '../icons.dart';
import '../messages_state.dart';
import '../pages/messages.dart' show relativeTime, absoluteTime;
import '../src/rust/api.dart' as rust;
import '../strings.dart';
import '../theme.dart';
import '../widgets.dart';
import 'phone.dart';

class PhoneMessagesPage extends StatefulWidget {
  const PhoneMessagesPage({super.key, required this.app});
  final AppState app;
  @override
  State<PhoneMessagesPage> createState() => _PhoneMessagesPageState();
}

class _PhoneMessagesPageState extends State<PhoneMessagesPage> {
  MessagesState get messages => widget.app.messages;
  @override
  void initState() {
    super.initState();
    if (!messages.view.value.loaded) unawaited(messages.reload());
    messages.openRequests.addListener(_requested);
    WidgetsBinding.instance.addPostFrameCallback((_) => _requested());
  }

  @override
  void dispose() {
    messages.openRequests.removeListener(_requested);
    super.dispose();
  }

  void _requested() => unawaited(_openRequested());
  Future<void> _openRequested() async {
    final request = messages.takeOpen();
    if (request == null) return;
    final index = request.messageId == null
        ? null
        : await messages.locate(request.messageId!);
    if (!mounted) return;
    _open(index == null ? request.push : messages.view.value.items[index]);
  }

  void _open(rust.UiMessage message) {
    unawaited(messages.setRead(message, true));
    unawaited(
      pushPhonePage<void>(
        context,
        _MessageDetail(message: message, messages: messages),
      ),
    );
  }

  @override
  Widget build(BuildContext context) => ValueListenableBuilder(
    valueListenable: messages.view,
    builder: (context, view, _) {
      final t = Tokens.of(context);
      final s = S.current;
      return NotificationListener<ScrollNotification>(
        onNotification: (event) {
          if (event.metrics.extentAfter < 240) unawaited(messages.loadMore());
          return false;
        },
        child: CustomScrollView(
          physics: const AlwaysScrollableScrollPhysics(
            parent: BouncingScrollPhysics(),
          ),
          slivers: [
            CupertinoSliverRefreshControl(onRefresh: messages.reload),
            SliverPadding(
              padding: const EdgeInsets.fromLTRB(16, 8, 16, 12),
              sliver: SliverToBoxAdapter(
                child: Row(
                  children: [
                    Expanded(
                      child: Text(s('msgCenter'), style: t.phoneBodyStrong),
                    ),
                    Button(
                      kind: ButtonKind.text,
                      height: 44,
                      label: s('markAllRead'),
                      onPress: view.items.any((m) => !m.read)
                          ? () => unawaited(messages.markAllRead())
                          : null,
                    ),
                  ],
                ),
              ),
            ),
            if (view.failed)
              SliverToBoxAdapter(
                child: Padding(
                  padding: const EdgeInsets.all(16),
                  child: Button(
                    kind: ButtonKind.secondary,
                    label: s('retry'),
                    onPress: () => unawaited(messages.retry()),
                  ),
                ),
              ),
            if (!view.loaded && !view.failed)
              SliverFillRemaining(
                hasScrollBody: false,
                child: Center(child: Spinner(size: 24, color: t.accent)),
              )
            else if (view.items.isEmpty)
              SliverFillRemaining(
                hasScrollBody: false,
                child: Center(child: Text(s('allRead'), style: t.phoneNote)),
              )
            else
              SliverPadding(
                padding: const EdgeInsets.symmetric(horizontal: 16),
                sliver: SliverList.builder(
                  itemCount: view.items.length,
                  itemBuilder: (context, index) {
                    final message = view.items[index];
                    return Padding(
                      padding: const EdgeInsets.only(bottom: 8),
                      child: Pressable(
                        label: message.title,
                        onPress: () => _open(message),
                        radius: Tokens.phoneCardRadius,
                        builder: (context, pressed) => Container(
                          decoration: t.phoneCardDecoration(
                            color: pressed ? t.sunk : t.card,
                          ),
                          padding: const EdgeInsets.all(14),
                          child: Row(
                            crossAxisAlignment: CrossAxisAlignment.start,
                            children: [
                              Container(
                                width: 36,
                                height: 36,
                                decoration: BoxDecoration(
                                  color: t.accentSoft,
                                  borderRadius: BorderRadius.circular(11),
                                ),
                                child: Icon(
                                  message.category ==
                                          rust.UiMessageCategory.announcement
                                      ? AppIcons.megaphone
                                      : AppIcons.inbox,
                                  size: 20,
                                  color: t.accent,
                                ),
                              ),
                              const SizedBox(width: 12),
                              Expanded(
                                child: Column(
                                  crossAxisAlignment: CrossAxisAlignment.start,
                                  children: [
                                    Row(
                                      children: [
                                        Expanded(
                                          child: Text(
                                            message.title,
                                            maxLines: 2,
                                            overflow: TextOverflow.ellipsis,
                                            style: t.phoneBody.copyWith(
                                              fontWeight: message.read
                                                  ? FontWeight.w500
                                                  : FontWeight.w600,
                                            ),
                                          ),
                                        ),
                                        if (!message.read) ...[
                                          const SizedBox(width: 8),
                                          Container(
                                            width: 7,
                                            height: 7,
                                            decoration: BoxDecoration(
                                              color: t.accent,
                                              shape: BoxShape.circle,
                                            ),
                                          ),
                                        ],
                                      ],
                                    ),
                                    const SizedBox(height: 5),
                                    Text(
                                      message.content,
                                      maxLines: 2,
                                      overflow: TextOverflow.ellipsis,
                                      style: t.phoneNote,
                                    ),
                                    const SizedBox(height: 8),
                                    Text(
                                      relativeTime(
                                        DateTime.tryParse(message.createdAt)
                                            ?.toLocal(),
                                        DateTime.now(),
                                      ),
                                      style: t.note.copyWith(color: t.t3),
                                    ),
                                  ],
                                ),
                              ),
                              const SizedBox(width: 4),
                              Icon(
                                AppIcons.chevronRight,
                                size: 16,
                                color: t.t3,
                              ),
                            ],
                          ),
                        ),
                      ),
                    );
                  },
                ),
              ),
            if (view.loadingMore)
              SliverToBoxAdapter(
                child: Padding(
                  padding: EdgeInsets.all(20),
                  child: Center(child: Spinner(size: 20, color: t.accent)),
                ),
              ),
            const SliverToBoxAdapter(child: SizedBox(height: 32)),
          ],
        ),
      );
    },
  );
}

class _MessageDetail extends StatelessWidget {
  const _MessageDetail({required this.message, required this.messages});
  final rust.UiMessage message;
  final MessagesState messages;
  @override
  Widget build(BuildContext context) {
    final t = Tokens.of(context);
    final s = S.current;
    final date = DateTime.tryParse(message.createdAt)?.toLocal();
    return PhoneScaffold(
      title: s('msgCenter'),
      back: true,
      child: ListView(
        padding: EdgeInsets.fromLTRB(
          20,
          16,
          20,
          24 + MediaQuery.paddingOf(context).bottom,
        ),
        children: [
          Text(message.title, style: t.phoneTitle),
          if (date != null)
            Padding(
              padding: const EdgeInsets.only(top: 10),
              child: Text(absoluteTime(date), style: t.phoneNote),
            ),
          const SizedBox(height: 24),
          SelectableText(
            message.content,
            style: t.phoneBody.copyWith(height: 1.6),
          ),
          const SizedBox(height: 28),
          if (message.deepLink != null)
            Button(
              kind: ButtonKind.primary,
              label: s('viewDetails'),
              onPress: () => messages.openLink(message),
            ),
          const SizedBox(height: 8),
          ValueListenableBuilder(
            valueListenable: messages.view,
            builder: (context, view, _) {
              final current = view.items
                  .where((m) => m.id == message.id)
                  .firstOrNull;
              if (current == null) return const SizedBox.shrink();
              return Button(
                kind: ButtonKind.text,
                label: s(current.read ? 'markUnread' : 'markRead'),
                onPress: () =>
                    unawaited(messages.setRead(current, !current.read)),
              );
            },
          ),
        ],
      ),
    );
  }
}
