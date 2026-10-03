using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>The message center list: first load (6 skeleton rows), the list, or the empty state.</summary>
public enum InboxState { FirstLoad, Ready, Empty }

public static class MessageTypes
{
    /// <summary>
    /// The <c>t_*</c> label key of a category (the crate derives <see cref="InboxMessage.Category"/>
    /// from the backend type and event key): expiring, expired, bill, order, route, broadcast,
    /// and <c>t_other</c> for anything else.
    /// </summary>
    public static string LabelKey(MessageCategory category) => category switch
    {
        MessageCategory.SubscriptionExpiring => "t_expiring",
        MessageCategory.SubscriptionExpired => "t_expired",
        MessageCategory.Billing => "t_bill",
        MessageCategory.Order => "t_order",
        MessageCategory.Route => "t_route",
        MessageCategory.Announcement => "t_broadcast",
        _ => "t_other",
    };
}

/// <summary>
/// The message center (a separate window): unread badge, paged list with its states, mark read /
/// all read / unread, the detail view with previous / next, deep links, and the entry point for
/// clicks on the push agent's OS notifications (<see cref="ActivateNotification"/>).
/// </summary>
public sealed partial class InboxViewModel : ObservableObject
{
    public const uint PageSize = 20;
    /// <summary>Pages <see cref="OpenMessageAsync"/> loads at most while looking for a message.</summary>
    const int MaxSearchPages = 10;

    readonly MainViewModel _main;
    readonly ILocalizer _strings;
    readonly IAppServices _services;
    readonly TimeProvider _time;
    uint _loadedPages;
    int _generation;
    /// <summary>A refresh was requested before sign-in finished; runs once SignedIn.</summary>
    bool _refreshPending;
    /// <summary>Notifications activated before sign-in finished; marked read once SignedIn.</summary>
    readonly List<ulong> _pendingReads = [];
    /// <summary>
    /// A click on a push about an inbox message before sign-in finished: its read-only detail is
    /// shown meanwhile, and the message is opened (or marked read) once SignedIn.
    /// </summary>
    (PushMessage Push, ulong MessageId)? _pendingActivation;
    /// <summary>A live refresh (<see cref="ScheduleLiveRefresh"/>) is waiting out its debounce.</summary>
    bool _liveRefreshScheduled;

    /// <summary>
    /// How long a rising unread count waits before the first page is re-fetched; increases within
    /// it share one reload (tests shorten it).
    /// </summary>
    internal TimeSpan LiveRefreshDelay { get; set; } = TimeSpan.FromSeconds(1);

    internal InboxViewModel(MainViewModel main, ILocalizer strings, IAppServices services, TimeProvider time)
    {
        _main = main;
        _strings = strings;
        _services = services;
        _time = time;
        headerText = strings.Get("allRead");
        Messages.CollectionChanged += (_, _) => UpdateListState();
    }

    /// <summary>Loaded messages, newest first.</summary>
    public ObservableCollection<InboxItemViewModel> Messages { get; } = [];

    /// <summary>Unread messages of the account (server total, <c>ClientSnapshot.UnreadNotifications</c>).</summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(HasUnread), nameof(BadgeText), nameof(CanMarkAllRead))]
    int unreadCount;

    public bool HasUnread => UnreadCount > 0;
    /// <summary>"3", "99+"; empty at 0 (badge hidden).</summary>
    public string BadgeText => Formatting.Badge(UnreadCount);
    /// <summary>Header: <c>unreadShort</c> {n} / <c>allRead</c>.</summary>
    [ObservableProperty] string headerText;
    public bool CanMarkAllRead => UnreadCount > 0;

    /// <summary>Messages of the account (server total; "1 / 46").</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasMore), nameof(PositionText), nameof(CanNext))] int total;

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsFirstLoad), nameof(IsEmpty), nameof(IsReady))] InboxState state = InboxState.FirstLoad;
    public bool IsFirstLoad => State == InboxState.FirstLoad;
    public bool IsEmpty => State == InboxState.Empty;
    public bool IsReady => State == InboxState.Ready;

    /// <summary>The first page has been loaded (for the current sign-in).</summary>
    [ObservableProperty] bool isLoaded;
    /// <summary>Any page load is running.</summary>
    [ObservableProperty] bool isLoading;
    /// <summary>The footer spinner + <c>loadingMore</c>.</summary>
    [ObservableProperty] bool isLoadingMore;
    /// <summary>Everything is loaded: the footer shows <c>noMore</c>.</summary>
    [ObservableProperty] bool isExhausted;
    /// <summary>The error bar (<c>msgErrT</c> / <c>msgErrD</c> + <see cref="RetryCommand"/>); the cached list stays.</summary>
    [ObservableProperty] bool hasLoadError;

    public bool HasMore => Messages.Count < Total;

    // --- detail --------------------------------------------------------------------

    /// <summary>
    /// The message shown in the detail view; null shows the list. A clicked push without an inbox
    /// message shows as a read-only detail (<see cref="IsDetailReadOnly"/>).
    /// </summary>
    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(IsDetailOpen), nameof(IsDetailReadOnly), nameof(PositionText), nameof(CanPrevious), nameof(CanNext), nameof(DetailReadActionText))]
    InboxItemViewModel? detail;
    public bool IsDetailOpen => Detail is not null;
    /// <summary>
    /// The detail is a push, not an inbox message (<see cref="InboxItemViewModel.IsReadOnly"/>):
    /// title, body, time and severity only; hide the position, previous / next and the read toggle.
    /// </summary>
    public bool IsDetailReadOnly => Detail is { IsReadOnly: true };
    int DetailIndex => Detail is null or { IsReadOnly: true } ? -1 : Messages.IndexOf(Detail);
    /// <summary>"1 / 46" (position in the server total); empty for a read-only detail.</summary>
    public string PositionText => DetailIndex < 0 ? "" : $"{DetailIndex + 1} / {Math.Max(Total, Messages.Count)}";
    public bool CanPrevious => DetailIndex > 0;
    public bool CanNext => DetailIndex >= 0 && DetailIndex < Math.Max(Total, Messages.Count) - 1;
    /// <summary>The detail footer toggle: <c>markRead</c> / <c>markUnread</c>; empty for a read-only detail.</summary>
    public string DetailReadActionText => IsDetailReadOnly ? "" : _strings.Get(Detail is { IsRead: true } ? "markUnread" : "markRead");

    // --- commands --------------------------------------------------------------------

    /// <summary>
    /// Reload the first page (when the window opens; F5). Requested before sign-in has finished
    /// (e.g. while Restoring), it runs automatically once signed in.
    /// </summary>
    [RelayCommand]
    Task RefreshAsync()
    {
        if (_main.Snapshot.Auth is not AuthState.SignedIn)
        {
            _refreshPending = true;
            return Task.CompletedTask;
        }
        return LoadAsync(LoadMode.Reset);
    }

    /// <summary>The error bar's Retry: reload, keeping the cached list until it succeeds.</summary>
    [RelayCommand]
    Task RetryAsync() => RefreshAsync();

    /// <summary>Scrolled within 24 px of the bottom.</summary>
    [RelayCommand]
    Task LoadMoreAsync() => HasMore && !IsLoading ? LoadAsync(LoadMode.More) : Task.CompletedTask;

    [RelayCommand]
    Task MarkAllReadAsync() => _main.RunAsync("mark_all_notifications_read", async () =>
    {
        await _main.Backend.MarkAllNotificationsRead();
        foreach (var item in Messages) item.IsRead = true;
        OnPropertyChanged(nameof(DetailReadActionText));
    });

    [RelayCommand]
    void Back() => Detail = null;

    [RelayCommand]
    void Previous()
    {
        if (CanPrevious) ShowDetail(Messages[DetailIndex - 1]);
    }

    /// <summary>Next; at the end of the loaded list it loads the next page first.</summary>
    [RelayCommand]
    async Task NextAsync()
    {
        if (!CanNext) return;
        var index = DetailIndex;
        if (index + 1 >= Messages.Count && HasMore) await LoadAsync(LoadMode.More);
        if (index + 1 < Messages.Count) ShowDetail(Messages[index + 1]);
    }

    /// <summary>The detail footer: mark read / unread.</summary>
    [RelayCommand]
    Task ToggleDetailReadAsync() => Detail is { } item ? item.ToggleReadCommand.ExecuteAsync(null) : Task.CompletedTask;

    /// <summary>
    /// A click on an OS notification (on the UI thread). <paramref name="pushId"/> is all the
    /// notification carries (launch arguments <c>id=&lt;pushId&gt;</c>, the push queue id); it is
    /// resolved through <see cref="Backend.IClientBackend.ShownPush"/>, and nothing else from the
    /// notification is trusted:
    /// <list type="number">
    /// <item>An absolute http(s) <c>DeepLink</c>: open it; mark <c>MessageId</c> read, if any.</item>
    /// <item>A <c>MessageId</c>: show the message center on that message (marks it read).</item>
    /// <item>Otherwise, or when the message is not found: a read-only detail of the push
    /// (<see cref="IsDetailReadOnly"/>; no inbox calls, nothing marked read).</item>
    /// <item>An unknown push id: just show the message center.</item>
    /// </list>
    /// Before sign-in has finished (the click launched the app), the push's read-only detail shows
    /// right away and the message is opened (or marked read) once signed in; that is dropped if
    /// the app ends up signed out.
    /// </summary>
    public async Task ActivateNotification(ulong pushId)
    {
        var push = LookUpShownPush(pushId);
        if (push is null)
        {
            _main.RequestShow(AppSurface.MessageCenter);
            return;
        }
        if (IsOpenable(push.DeepLink))
        {
            OpenLink(push.DeepLink);
            if (push.MessageId is { } read) await MarkReadWhenSignedInAsync(read);
            return;
        }
        if (push.MessageId is not { } messageId)
        {
            _main.RequestShow(AppSurface.MessageCenter);
            ShowPushDetail(push);
            return;
        }
        _main.RequestShow(AppSurface.MessageCenter, messageId);
        switch (_main.Snapshot.Auth)
        {
            case AuthState.SignedIn:
                if (!await OpenMessageAsync(messageId)) ShowPushDetail(push);
                break;
            case AuthState.Restoring or AuthState.AwaitingBrowser:
                ShowPushDetail(push);
                _pendingActivation = (push, messageId);
                break;
            default:
                ShowPushDetail(push);
                break;
        }
    }

    /// <summary>
    /// Show message <paramref name="id"/> in the detail view, loading pages until it is found
    /// (it may be older than the loaded ones). False when it was not found.
    /// </summary>
    public async Task<bool> OpenMessageAsync(ulong id)
    {
        if (!IsLoaded) await LoadAsync(LoadMode.Reset);
        var pages = 0;
        while (Messages.All(m => m.Id != id) && HasMore && !HasLoadError && pages++ < MaxSearchPages)
            await LoadAsync(LoadMode.More);
        if (Messages.FirstOrDefault(m => m.Id == id) is not { } item) return false;
        ShowDetail(item);
        return true;
    }

    PushMessage? LookUpShownPush(ulong pushId)
    {
        try
        {
            return _main.Backend.ShownPush(pushId);
        }
        catch (Exception error)
        {
            _main.Log.Warn($"inbox: shown push {pushId} unavailable: {ErrorMessages.Describe(error)}");
            return null;
        }
    }

    void ShowPushDetail(PushMessage push) =>
        Detail = new InboxItemViewModel(this, push, _strings, _time.LocalTimeZone, _time.GetUtcNow());

    /// <summary>Signed in after a click on a push about <paramref name="messageId"/> (see <see cref="ActivateNotification"/>).</summary>
    async Task CompleteActivationAsync(PushMessage push, ulong messageId)
    {
        // Still showing the push: replace it with the message when the server has it.
        if (Detail is { IsReadOnly: true } shown && shown.PushId == push.Id)
        {
            await OpenMessageAsync(messageId);
            return;
        }
        await MarkReadAsync(messageId);
    }

    // --- items (internal) ------------------------------------------------------------

    internal void ShowDetail(InboxItemViewModel item)
    {
        Detail = item;
        // Opening marks the message read.
        if (!item.IsRead && !item.IsReadOnly) _ = MarkReadAsync(item.Id);
    }

    internal async Task OpenLinkAsync(InboxItemViewModel item)
    {
        OpenLink(item.DeepLink);
        if (!item.IsRead) await MarkReadAsync(item.Id);
    }

    internal Task MarkReadAsync(ulong id) => SetReadAsync(id, true);

    internal Task SetReadAsync(ulong id, bool read) => _main.RunAsync(read ? "mark_notification_read" : "mark_notification_unread", async () =>
    {
        if (read) await _main.Backend.MarkNotificationRead(id);
        else await _main.Backend.MarkNotificationUnread(id);
        if (Messages.FirstOrDefault(m => m.Id == id) is { } item) item.IsRead = read;
        OnPropertyChanged(nameof(DetailReadActionText));
    });

    async Task MarkReadWhenSignedInAsync(ulong id)
    {
        switch (_main.Snapshot.Auth)
        {
            case AuthState.SignedIn:
                await MarkReadAsync(id);
                break;
            case AuthState.Restoring or AuthState.AwaitingBrowser:
                if (!_pendingReads.Contains(id)) _pendingReads.Add(id);
                break;
        }
    }

    /// <summary>Only absolute http(s) links are opened; anything else from the backend is ignored.</summary>
    internal static bool IsOpenable(string? link) =>
        Uri.TryCreate(link, UriKind.Absolute, out var uri) && (uri.Scheme == Uri.UriSchemeHttps || uri.Scheme == Uri.UriSchemeHttp);

    void OpenLink(string? link)
    {
        if (!IsOpenable(link))
        {
            if (!string.IsNullOrEmpty(link)) _main.Log.Warn($"inbox: ignored deep link {link}");
            return;
        }
        if (!_services.OpenUrl(link!)) _services.CopyText(link!);
    }

    /// <summary>Refresh the relative times (every 60 s from <see cref="MainViewModel"/>'s ticker).</summary>
    public void RefreshTimes()
    {
        var now = _time.GetUtcNow();
        foreach (var item in Messages) item.RefreshTime(now);
        if (Detail is { IsReadOnly: true } push) push.RefreshTime(now);
    }

    enum LoadMode
    {
        /// <summary>Replace the list with page 1 (window opened, F5, retry).</summary>
        Reset,
        /// <summary>Append the next page.</summary>
        More,
        /// <summary>Re-fetch page 1 and prepend what is new, keeping later pages (<see cref="ScheduleLiveRefresh"/>).</summary>
        Merge,
    }

    async Task LoadAsync(LoadMode mode)
    {
        if (_main.Snapshot.Auth is not AuthState.SignedIn || IsLoading) return;
        var generation = _generation;
        var page = mode == LoadMode.More ? _loadedPages + 1 : 1;
        var live = mode == LoadMode.Merge;
        IsLoading = true;
        IsLoadingMore = mode == LoadMode.More;
        try
        {
            var result = await _main.Backend.Notifications(page, PageSize);
            if (generation != _generation) return; // signed out meanwhile
            var now = _time.GetUtcNow();
            var items = result.Items.Select(m => new InboxItemViewModel(this, m, _strings, _time.LocalTimeZone, now)).ToList();
            // More new messages than a page: page 1 no longer joins the loaded list, start over
            // (keeping the message being read).
            if (live && Messages.Count > 0 && items.Count > 0 && !items.Any(i => Messages.Any(m => m.Id == i.Id)))
                mode = LoadMode.Reset;
            switch (mode)
            {
                case LoadMode.Reset:
                    var open = Detail;
                    Messages.Clear();
                    foreach (var item in items) Messages.Add(item);
                    // A read-only (push) detail is not in the list and stays. A live refresh also
                    // keeps a message being read that is no longer on page 1 (until Back).
                    if (open is { IsReadOnly: false })
                        Detail = Messages.FirstOrDefault(m => m.Id == open.Id) ?? (live ? open : null);
                    _loadedPages = page;
                    break;
                case LoadMode.More:
                    var known = Messages.Select(m => m.Id).ToHashSet();
                    foreach (var item in items)
                        if (known.Add(item.Id)) Messages.Add(item);
                    _loadedPages = page;
                    break;
                case LoadMode.Merge:
                    MergeFirstPage(items);
                    break;
            }
            Total = (int)result.Total;
            // A page that added nothing ends paging (the list changed under us).
            if (mode == LoadMode.More && items.Count == 0) Total = Messages.Count;
            HasLoadError = false;
            IsLoaded = true;
        }
        catch (Exception error)
        {
            _main.Log.Warn($"notifications page {page} failed: {ErrorMessages.Describe(error)}");
            // A background refresh failing leaves the list as it was, without the error bar.
            if (generation == _generation && !live) HasLoadError = true;
        }
        finally
        {
            IsLoading = false;
            IsLoadingMore = false;
            UpdateListState();
        }
    }

    /// <summary>
    /// Prepend the messages of a fresh page 1 that are not loaded yet, and take the read state of
    /// the ones that are (read elsewhere). Existing items (and so <see cref="Detail"/>) are kept.
    /// </summary>
    void MergeFirstPage(List<InboxItemViewModel> items)
    {
        var loaded = Messages.ToDictionary(m => m.Id);
        var insertAt = 0;
        foreach (var item in items)
        {
            if (loaded.TryGetValue(item.Id, out var existing)) existing.IsRead = item.IsRead;
            else Messages.Insert(insertAt++, item);
        }
        // Everything loaded is contiguous from the top again; the next page may overlap the end
        // (LoadMore skips known ids) but never skips a message.
        _loadedPages = (uint)(Messages.Count / (int)PageSize);
        OnPropertyChanged(nameof(DetailReadActionText));
    }

    /// <summary>
    /// The unread count rose (a new message, or one marked unread elsewhere): once the debounce
    /// has passed, re-fetch page 1 into a loaded list. Only while signed in with a loaded list.
    /// </summary>
    async void ScheduleLiveRefresh()
    {
        if (_liveRefreshScheduled) return;
        _liveRefreshScheduled = true;
        var generation = _generation;
        do
        {
            await Task.Delay(LiveRefreshDelay, _time);
            if (generation != _generation || _main.Snapshot.Auth is not AuthState.SignedIn || !IsLoaded)
            {
                if (generation == _generation) _liveRefreshScheduled = false;
                return;
            }
        }
        while (IsLoading); // another load is running: wait another round rather than drop this one
        // Increases from now on may not be in this fetch: let them schedule another one.
        _liveRefreshScheduled = false;
        await LoadAsync(LoadMode.Merge);
    }

    void UpdateListState()
    {
        State = !IsLoaded ? InboxState.FirstLoad : Messages.Count == 0 ? InboxState.Empty : InboxState.Ready;
        IsExhausted = IsLoaded && Messages.Count > 0 && !HasMore;
        OnPropertyChanged(nameof(HasMore));
        OnPropertyChanged(nameof(PositionText));
        OnPropertyChanged(nameof(CanPrevious));
        OnPropertyChanged(nameof(CanNext));
    }

    partial void OnIsLoadedChanged(bool value) => UpdateListState();

    partial void OnUnreadCountChanged(int value) =>
        HeaderText = value > 0 ? _strings.Format("unreadShort", ("n", Formatting.Badge(value))) : _strings.Get("allRead");

    // --- from MainViewModel (UI thread) ----------------------------------

    internal void OnSnapshot(ClientSnapshot next)
    {
        if (next.Auth is not AuthState.SignedIn)
        {
            UnreadCount = 0;
            if (next.Auth is AuthState.SignedOut)
            {
                _pendingReads.Clear();
                _pendingActivation = null;
            }
            if (Messages.Count > 0 || IsLoaded)
            {
                _generation++;
                _liveRefreshScheduled = false;
                Detail = null;
                Messages.Clear();
                _loadedPages = 0;
                Total = 0;
                HasLoadError = false;
                IsLoaded = false;
            }
            return;
        }
        var previousUnread = UnreadCount;
        UnreadCount = (int)next.UnreadNotifications;
        // New messages (push or the crate's unread poll) show up in an open list; a decrease is
        // the user reading, already applied locally.
        if (UnreadCount > previousUnread && IsLoaded) ScheduleLiveRefresh();

        if (_refreshPending)
        {
            _refreshPending = false;
            _ = LoadAsync(LoadMode.Reset);
        }
        if (_pendingReads.Count > 0)
        {
            var ids = _pendingReads.ToArray();
            _pendingReads.Clear();
            foreach (var id in ids) _ = MarkReadAsync(id);
        }
        if (_pendingActivation is { } pending)
        {
            _pendingActivation = null;
            _ = CompleteActivationAsync(pending.Push, pending.MessageId);
        }
    }
}

/// <summary>One message row (and the detail view's content).</summary>
public sealed partial class InboxItemViewModel : ObservableObject
{
    readonly InboxViewModel _owner;
    readonly ILocalizer _strings;
    readonly TimeZoneInfo _zone;

    internal InboxItemViewModel(InboxViewModel owner, InboxMessage message, ILocalizer strings, TimeZoneInfo zone, DateTimeOffset now)
    {
        _owner = owner;
        _strings = strings;
        _zone = zone;
        Message = message;
        isRead = message.Read;
        CreatedAt = Formatting.ParseRfc3339(message.CreatedAt);
        AbsoluteTime = CreatedAt is { } at ? Formatting.AbsoluteTime(at, zone, strings.Language) : "";
        RefreshTime(now);
    }

    /// <summary>A read-only detail of a clicked push (<see cref="IsReadOnly"/>).</summary>
    internal InboxItemViewModel(InboxViewModel owner, PushMessage push, ILocalizer strings, TimeZoneInfo zone, DateTimeOffset now)
        : this(owner, new InboxMessage(0, push.Title, push.Body, Kind: "", push.EventKey, push.Category, push.Severity,
            DeepLink: null, Push: true, Read: true, push.CreatedAt), strings, zone, now)
    {
        IsReadOnly = true;
        PushId = push.Id;
    }

    public InboxMessage Message { get; }
    /// <summary>The inbox message id; 0 for a read-only push detail.</summary>
    public ulong Id => Message.Id;
    /// <summary>
    /// A clicked push shown on its own (no inbox message): title, body, time and severity; no
    /// link, no read state, not in <see cref="InboxViewModel.Messages"/>. Its commands do nothing.
    /// </summary>
    public bool IsReadOnly { get; }
    /// <summary>The push queue id of a read-only push detail; null otherwise.</summary>
    public ulong? PushId { get; }
    public string Title => Message.Title;
    public string Content => Message.Content;
    public MessageSeverity Severity => Message.Severity;
    public DateTimeOffset? CreatedAt { get; }

    /// <summary>What the message is about (icon); from the crate.</summary>
    public MessageCategory Category => Message.Category;
    /// <summary><c>t_*</c>: "订阅即将到期".</summary>
    public string TypeLabel => _strings.Get(MessageTypes.LabelKey(Category));
    /// <summary>Critical and important carry a label (and the left bar / icon tint).</summary>
    public bool HasSeverityLabel => Severity is MessageSeverity.Critical or MessageSeverity.Important;
    /// <summary><c>sev_critical</c> / <c>sev_important</c>; empty otherwise.</summary>
    public string SeverityLabel => Severity switch
    {
        MessageSeverity.Critical => _strings.Get("sev_critical"),
        MessageSeverity.Important => _strings.Get("sev_important"),
        _ => "",
    };
    /// <summary>"订阅即将到期 · 紧急" or just the type label.</summary>
    public string MetaText => HasSeverityLabel ? $"{TypeLabel} · {SeverityLabel}" : TypeLabel;
    /// <summary>3px bar / icon tint: critical → Bad, important → Caution, otherwise Neutral (no bar).</summary>
    public StatusTone Tone => Severity switch
    {
        MessageSeverity.Critical => StatusTone.Bad,
        MessageSeverity.Important => StatusTone.Caution,
        _ => StatusTone.Neutral,
    };

    /// <summary>List: "刚刚", "3 分钟前", "今天 14:20", "9月21日"; refreshed every 60 s.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(DetailTimeText))] string relativeTime = "";
    /// <summary>"2026年9月29日 14:29".</summary>
    public string AbsoluteTime { get; }
    /// <summary>Detail header: "2026年9月29日 14:29 · 3 分钟前".</summary>
    public string DetailTimeText => AbsoluteTime.Length == 0 ? RelativeTime : $"{AbsoluteTime} · {RelativeTime}";

    public string? DeepLink => Message.DeepLink;
    public bool HasLink => InboxViewModel.IsOpenable(Message.DeepLink);

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsUnread))] bool isRead;
    public bool IsUnread => !IsRead;

    /// <summary>Row click: the detail view (marks it read).</summary>
    [RelayCommand]
    void ShowDetail() => _owner.ShowDetail(this);

    /// <summary><c>viewDetails</c> ↗: open the link in the browser and mark read.</summary>
    [RelayCommand]
    Task OpenLinkAsync() => _owner.OpenLinkAsync(this);

    /// <summary>Hover / context menu <c>markRead</c>.</summary>
    [RelayCommand]
    Task MarkReadAsync() => IsRead ? Task.CompletedTask : _owner.MarkReadAsync(Id);

    /// <summary>Detail footer: <c>markRead</c> / <c>markUnread</c>.</summary>
    [RelayCommand]
    Task ToggleReadAsync() => IsReadOnly ? Task.CompletedTask : _owner.SetReadAsync(Id, !IsRead);

    internal void RefreshTime(DateTimeOffset now) =>
        RelativeTime = CreatedAt is { } at ? Formatting.RelativeTime(at, now, _zone, _strings) : "";
}
