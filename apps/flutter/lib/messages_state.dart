// The message center's list (#401): pages of [MessagesState.pageSize]
// from the client, newest first. Kept while the app runs, so the page
// reopens on what it showed and a failed reload keeps the list. A read
// state change shows at once and goes back if the call fails. The unread
// count is the snapshot's (the server's total), not counted here. A
// notification click asks the page to open its message ([requestOpen]).

import 'dart:async';

import 'package:flutter/foundation.dart';

import 'links.dart';
import 'src/rust/api.dart' as rust;

/// What the message center shows.
class MessagesView {
  const MessagesView({
    this.items = const [],
    this.total = 0,
    this.loaded = false,
    this.loading = false,
    this.loadingMore = false,
    this.failed = false,
  });

  /// Loaded so far, newest first.
  final List<rust.UiMessage> items;

  /// All messages of the account.
  final int total;

  /// A page arrived at least once: until then the skeleton shows.
  final bool loaded;

  /// The first page is being (re)loaded.
  final bool loading;
  final bool loadingMore;

  /// The latest load failed: the error banner shows.
  final bool failed;

  bool get hasMore => items.length < total;

  MessagesView copyWith({
    List<rust.UiMessage>? items,
    int? total,
    bool? loaded,
    bool? loading,
    bool? loadingMore,
    bool? failed,
  }) => MessagesView(
    items: items ?? this.items,
    total: total ?? this.total,
    loaded: loaded ?? this.loaded,
    loading: loading ?? this.loading,
    loadingMore: loadingMore ?? this.loadingMore,
    failed: failed ?? this.failed,
  );
}

/// What a notification click opens: inbox message [messageId], else (or
/// when the inbox does not have it) the push itself, read-only.
typedef OpenRequest = ({int? messageId, rust.UiMessage push});

class MessagesState {
  MessagesState({required this.fixed, required this.onError});

  static const pageSize = 20;

  /// The most pages [locate] reads (as the native apps).
  static const searchPages = 10;

  /// Sample data with no client behind it (sample.dart): nothing is
  /// loaded, read changes stay local, links do not open.
  final bool fixed;

  /// A read change failed (the message went back).
  final void Function(String error) onError;

  final ValueNotifier<MessagesView> view = ValueNotifier(const MessagesView());

  int _pages = 0;

  /// Bumped by [reload] and owner changes; stale answers cannot update the view.
  int _generation = 0;

  /// The failed load was a further page (retry loads it again).
  bool _failedMore = false;

  /// The loads in flight, for [locate] to wait on.
  Future<void>? _reloading;
  Future<void>? _loadingMore;

  String? _accountId;
  String? _teamId;
  bool _ownerKnown = false;
  bool _initialOwnerPending = true;

  /// Changes only when auth ownership changes, not for ordinary reloads.
  final ValueNotifier<int> ownerChanges = ValueNotifier(0);
  int get ownerEpoch => ownerChanges.value;

  /// Stable account ids identify owners; display-name changes retain data.
  /// Initial restoration keeps a cold tap until its authenticated owner is known.
  void followOwner({
    String? accountId,
    String? teamId,
    bool restoring = false,
  }) {
    if (restoring && !_ownerKnown) return;
    if (!restoring &&
        _ownerKnown &&
        accountId == _accountId &&
        teamId == _teamId)
      return;
    // Resolving the initial team is metadata hydration of the same owner.
    if (!restoring && _ownerKnown && accountId != null &&
        accountId == _accountId && _teamId == null && teamId != null) {
      _teamId = teamId;
      return;
    }
    final keepPendingOpen = _initialOwnerPending && !restoring && accountId != null;
    _initialOwnerPending = false;
    _ownerKnown = !restoring;
    _accountId = restoring ? null : accountId;
    _teamId = restoring ? null : teamId;
    _generation++;
    _readChanges.clear();
    _confirmedReads.clear();
    _readEffects = Future.value();
    _pendingReadWrites = 0;
    writeErrors.value = const {};
    _pages = 0;
    _failedMore = false;
    _reloading = null;
    _loadingMore = null;
    if (!keepPendingOpen) _openRequest = null;
    view.value = const MessagesView();
    ownerChanges.value++;
  }

  OpenRequest? _openRequest;

  /// Counts the notification clicks' requests; the page takes the latest
  /// with [takeOpen].
  final ValueNotifier<int> openRequests = ValueNotifier(0);

  void requestOpen(OpenRequest request) {
    _openRequest = request;
    openRequests.value++;
  }

  /// The latest request, once.
  OpenRequest? takeOpen() {
    final request = _openRequest;
    _openRequest = null;
    return request;
  }

  /// Inbox message [id]'s index in the list: the first page loaded again
  /// (or the load in flight), then further pages, at most [searchPages].
  /// Null when it is not there or a load fails.
  Future<int?> locate(int id) async {
    if (fixed) return null;
    final owner = ownerEpoch;
    final reloading = _reloading;
    await (view.value.loading && reloading != null ? reloading : reload());
    if (owner != ownerEpoch) return null;
    for (var pages = 1; ; pages++) {
      final current = view.value;
      final index = current.items.indexWhere((message) => message.id == id);
      if (index >= 0) return index;
      if (current.failed || !current.hasMore || pages >= searchPages) return null;
      await loadMore();
      if (owner != ownerEpoch) return null;
    }
  }

  /// Loads the first page again; the list stays while it loads.
  Future<void> reload() => _reloading = _reload();

  Future<void> _reload() async {
    if (fixed) return;
    final generation = ++_generation;
    final readRevision = _readRevision;
    final pendingReads = _pendingReads;
    view.value = view.value.copyWith(loading: true, loadingMore: false, failed: false);
    try {
      final page = await rust.notifications(page: 1, pageSize: pageSize);
      if (generation != _generation) return;
      _pages = 1;
      view.value = MessagesView(
        items: _overlayReads(page.items, readRevision, pendingReads),
        total: page.total,
        loaded: true,
      );
    } catch (error) {
      if (generation != _generation) return;
      debugPrint('notifications: $error');
      _failedMore = false;
      view.value = view.value.copyWith(loading: false, failed: true);
    }
  }

  /// The next page, when there is one and nothing else loads (the one
  /// loading, when that is the next page). Not after a failure (the list's
  /// end would ask again at once): [retry] does.
  Future<void> loadMore() {
    final current = view.value;
    if (current.loadingMore) return _loadingMore ?? Future.value();
    if (fixed || !current.loaded || current.loading || current.failed || !current.hasMore) {
      return Future.value();
    }
    return _loadingMore = _loadMore(current);
  }

  Future<void> _loadMore(MessagesView current) async {
    final generation = _generation;
    final readRevision = _readRevision;
    final pendingReads = _pendingReads;
    view.value = current.copyWith(loadingMore: true);
    try {
      final page = await rust.notifications(page: _pages + 1, pageSize: pageSize);
      if (generation != _generation) return;
      _pages++;
      // New messages shift the pages: skip what is already shown.
      final shown = {for (final message in view.value.items) message.id};
      final items = [
        ...view.value.items,
        ..._overlayReads(
          page.items,
          readRevision,
          pendingReads,
        ).where((message) => !shown.contains(message.id)),
      ];
      view.value = view.value.copyWith(
        items: items,
        // An empty page: the total was stale, stop asking.
        total: page.items.isEmpty ? items.length : page.total,
        loadingMore: false,
      );
    } catch (error) {
      if (generation != _generation) return;
      debugPrint('notifications: $error');
      _failedMore = true;
      view.value = view.value.copyWith(loadingMore: false, failed: true);
    }
  }

  /// 「重试」 on the error banner.
  Future<void> retry() {
    if (_failedMore && view.value.loaded) {
      view.value = view.value.copyWith(failed: false);
      return loadMore();
    }
    return reload();
  }

  int _readRevision = 0;
  int _pendingReadWrites = 0;
  Future<void> _readEffects = Future.value();
  final _readChanges = <int, ({int revision, bool read, bool pending})>{};
  final _confirmedReads = <int, bool>{};
  final writeErrors = ValueNotifier<Map<int, String>>(const {});
  Set<int> get _pendingReads => {
    for (final entry in _readChanges.entries)
      if (entry.value.pending) entry.key,
  };

  List<rust.UiMessage> _overlayReads(
    List<rust.UiMessage> items,
    int startedRevision,
    Set<int> pendingAtStart,
  ) => [
    for (final item in items)
      _overlayRead(item, startedRevision, pendingAtStart),
  ];

  rust.UiMessage _overlayRead(
    rust.UiMessage item,
    int startedRevision,
    Set<int> pendingAtStart,
  ) {
    final change = _readChanges[item.id];
    if (change == null) return item;
    if (change.pending ||
        change.revision > startedRevision ||
        pendingAtStart.contains(item.id)) {
      return _withRead(item, change.read);
    }
    // A request begun after the ACK is authoritative, including external edits.
    _readChanges.remove(item.id);
    _confirmedReads.remove(item.id);
    return item;
  }

  Future<void> setRead(rust.UiMessage message, bool read) {
    final current = view.value.items
        .where((m) => m.id == message.id)
        .firstOrNull;
    if ((current?.read ?? message.read) == read) return _readEffects;
    return _writeRead(
      {message.id: read},
      () => read
          ? rust.markNotificationRead(id: message.id)
          : rust.markNotificationUnread(id: message.id),
    );
  }

  Future<void> markAllRead() => _writeRead({
    for (final item in view.value.items) item.id: true,
  }, rust.markAllNotificationsRead);

  Future<void> _writeRead(
    Map<int, bool> values,
    Future<void> Function() write,
  ) {
    final owner = ownerEpoch;
    final revision = ++_readRevision;
    for (final entry in values.entries) {
      final before = view.value.items
          .where((m) => m.id == entry.key)
          .firstOrNull;
      _confirmedReads.putIfAbsent(
        entry.key,
        () => before?.read ?? !entry.value,
      );
      _readChanges[entry.key] = (
        revision: revision,
        read: entry.value,
        pending: !fixed,
      );
      _replace(entry.key, entry.value);
    }
    writeErrors.value = Map.of(writeErrors.value)
      ..removeWhere((id, _) => values.containsKey(id));
    if (fixed) return Future.value();
    // Preserve write order (including mark-all) so the server cannot finish an
    // older toggle after a newer one. UI updates remain immediate.
    Future<void> perform() async {
      if (owner != ownerEpoch) return;
      try {
        await write();
        if (owner != ownerEpoch) return;
        for (final entry in values.entries) {
          _confirmedReads[entry.key] = entry.value;
          if (_readChanges[entry.key]?.revision == revision) {
            _readChanges[entry.key] = (
              revision: revision,
              read: entry.value,
              pending: false,
            );
          }
        }
      } catch (error) {
        if (owner != ownerEpoch) return;
        var currentFailure = false;
        for (final id in values.keys) {
          if (_readChanges[id]?.revision != revision) continue;
          currentFailure = true;
          _readChanges.remove(id);
          _replace(id, _confirmedReads.remove(id)!);
          writeErrors.value = {...writeErrors.value, id: '$error'};
        }
        if (currentFailure || values.isEmpty) onError('$error');
      }
    }

    final idle = _pendingReadWrites++ == 0;
    final result = idle ? perform() : _readEffects.then((_) => perform());
    return _readEffects = result.whenComplete(() {
      if (owner == ownerEpoch) _pendingReadWrites--;
    });
  }

  /// 「查看详情」: opens the link in the browser and marks the message read.
  void openLink(rust.UiMessage message) {
    final link = message.deepLink;
    if (link == null) return;
    if (!fixed) unawaited(openUrl(link));
    unawaited(setRead(message, true));
  }

  void _replace(int id, bool read) {
    view.value = view.value.copyWith(
      items: [for (final message in view.value.items) message.id == id ? _withRead(message, read) : message],
    );
  }

  static rust.UiMessage _withRead(rust.UiMessage message, bool read) => rust.UiMessage(
    id: message.id,
    title: message.title,
    content: message.content,
    category: message.category,
    severity: message.severity,
    deepLink: message.deepLink,
    read: read,
    createdAt: message.createdAt,
  );
}
