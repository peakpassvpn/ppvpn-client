using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

public sealed class InboxTests
{
    [Fact]
    public async Task BadgeHeaderAndMarkAll()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { MessageCount = 12, UnreadMessages = 3 }, MemoryHooks.SignedIn());
            var main = h.Main;
            var inbox = main.Inbox;
            await Wait.Until(() => main.UnreadNotifications == 3, "badge");
            Assert.Equal(("3", true), (main.UnreadBadgeText, main.HasUnreadNotifications));
            Assert.Equal("unreadShort(n=3)", inbox.HeaderText);
            Assert.True(inbox.CanMarkAllRead);
            Assert.Equal(InboxState.FirstLoad, inbox.State);

            await inbox.RefreshCommand.ExecuteAsync(null);
            Assert.Equal(InboxState.Ready, inbox.State);
            Assert.Equal(12, inbox.Messages.Count);
            Assert.True(inbox.IsExhausted);
            var first = inbox.Messages[0];
            Assert.Equal((MessageCategory.SubscriptionExpiring, "t_expiring", "sev_critical", "t_expiring · sev_critical", StatusTone.Bad, true),
                (first.Category, first.TypeLabel, first.SeverityLabel, first.MetaText, first.Tone, first.HasLink));
            Assert.True(first.IsUnread);
            Assert.False(inbox.Messages[3].HasSeverityLabel); // broadcast, no severity
            Assert.Equal(MessageCategory.Announcement, inbox.Messages[3].Category);

            await inbox.MarkAllReadCommand.ExecuteAsync(null);
            Assert.All(inbox.Messages, m => Assert.True(m.IsRead));
            await Wait.Until(() => main.UnreadNotifications == 0, "badge cleared");
            Assert.Equal(("", "allRead", false), (main.UnreadBadgeText, inbox.HeaderText, inbox.CanMarkAllRead));
            Assert.Equal("noUnread", main.Tray.Items.Single(i => i.Role == TrayItemRole.Messages).Text);
        });
    }

    [Fact]
    public async Task DetailNavigationAcrossPages()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { MessageCount = 46, UnreadMessages = 7 }, MemoryHooks.SignedIn());
            var inbox = h.Main.Inbox;
            await Wait.Until(() => h.Main.UnreadNotifications == 7, "badge");
            await inbox.RefreshCommand.ExecuteAsync(null);
            Assert.Equal((20, 46, true), (inbox.Messages.Count, inbox.Total, inbox.HasMore));

            inbox.Messages[0].ShowDetailCommand.Execute(null);
            Assert.True(inbox.IsDetailOpen);
            Assert.Equal("1 / 46", inbox.PositionText);
            Assert.False(inbox.CanPrevious);
            Assert.True(inbox.CanNext);
            // Opening marks it read.
            await Wait.Until(() => inbox.Messages[0].IsRead && h.Main.UnreadNotifications == 6, "opened = read");
            Assert.Equal("markUnread", inbox.DetailReadActionText);

            for (var i = 0; i < 19; i++) await inbox.NextCommand.ExecuteAsync(null);
            Assert.Equal("20 / 46", inbox.PositionText);
            // Next at the end of the loaded page loads the next one.
            await inbox.NextCommand.ExecuteAsync(null);
            Assert.Equal("21 / 46", inbox.PositionText);
            Assert.Equal(40, inbox.Messages.Count);
            inbox.PreviousCommand.Execute(null);
            Assert.Equal("20 / 46", inbox.PositionText);

            // Mark unread from the detail footer.
            await inbox.ToggleDetailReadCommand.ExecuteAsync(null);
            Assert.False(inbox.Detail!.IsRead);
            Assert.Equal("markRead", inbox.DetailReadActionText);

            inbox.BackCommand.Execute(null);
            Assert.False(inbox.IsDetailOpen);

            // Paging to the end.
            await inbox.LoadMoreCommand.ExecuteAsync(null);
            Assert.Equal((46, false, true), (inbox.Messages.Count, inbox.HasMore, inbox.IsExhausted));
            inbox.Messages[45].ShowDetailCommand.Execute(null);
            Assert.Equal(("46 / 46", false), (inbox.PositionText, inbox.CanNext));
        });
    }

    /// <summary>A push as the agent recorded it (push queue id <paramref name="id"/>).</summary>
    static PushMessage Push(ulong id, ulong? messageId = null, string? link = null, string title = "推送标题",
        MessageSeverity severity = MessageSeverity.Important, MessageCategory category = MessageCategory.Announcement) =>
        new(id, messageId, title, "推送正文", severity, category, "broadcast:12", link, "2026-09-29T06:00:00Z");

    [Fact]
    public async Task ActivationWithALinkOpensItAndMarksTheMessageRead()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await Wait.Until(() => h.Main.UnreadNotifications == 3, "badge");
            var shown = new List<(AppSurface, ulong?)>();
            h.Main.ShowRequested += (surface, id) => shown.Add((surface, id));

            // (b) The link comes from the recorded push; its message is marked read.
            var link = $"https://www.peakpassvpn.com/dashboard/messages/{FakeClientBackend.NewestSampleId}";
            h.Backend.RecordShownPush(Push(900, FakeClientBackend.NewestSampleId, link));
            await h.Main.Inbox.ActivateNotification(900);
            Assert.Equal(link, h.Services.Opened.Single());
            Assert.Empty(shown);
            await Wait.Until(() => h.Main.UnreadNotifications == 2, "marked read");

            // A link without a message: opened, nothing marked read.
            h.Backend.RecordShownPush(Push(901, link: "https://www.peakpassvpn.com/promo"));
            await h.Main.Inbox.ActivateNotification(901);
            Assert.Equal("https://www.peakpassvpn.com/promo", h.Services.Opened[1]);
            Assert.Equal(2, h.Main.UnreadNotifications);

            // Only web links are opened: this one falls back to the message (c).
            var message = h.Backend.PushMessage("x", "y", push: false, deepLink: "file:///etc/passwd");
            h.Backend.RecordShownPush(Push(902, message.Id, "file:///etc/passwd"));
            await h.Main.Inbox.ActivateNotification(902);
            Assert.Equal(2, h.Services.Opened.Count);
            Assert.Equal((AppSurface.MessageCenter, message.Id), shown.Single());
            Assert.Equal(message.Id, h.Main.Inbox.Detail?.Id);
            Assert.False(h.Main.Inbox.IsDetailReadOnly);
        });
    }

    [Fact]
    public async Task ActivationOfAMessagePushOpensItsDetail()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { MessageCount = 46, UnreadMessages = 30 }, MemoryHooks.SignedIn());
            await Wait.Until(() => h.Main.UnreadNotifications == 30, "badge");
            var shown = new List<(AppSurface, ulong?)>();
            h.Main.ShowRequested += (surface, id) => shown.Add((surface, id));

            // (c) An older message on page 2 (no link: every 10th sample from index 9).
            var id = FakeClientBackend.NewestSampleId - 29;
            h.Backend.RecordShownPush(Push(77, id));
            await h.Main.Inbox.ActivateNotification(77);
            Assert.Equal((AppSurface.MessageCenter, id), shown.Single());
            Assert.Equal(id, h.Main.Inbox.Detail?.Id);
            Assert.False(h.Main.Inbox.IsDetailReadOnly);
            Assert.Equal("30 / 46", h.Main.Inbox.PositionText);
            await Wait.Until(() => h.Main.UnreadNotifications == 29, "opened = read");
            Assert.Empty(h.Services.Opened);
        });
    }

    [Fact]
    public async Task ActivationOfAMessageTheServerLacksShowsThePush()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            for (ulong i = 0; i < 3; i++)
                t.Backend.Inbox.Add(new InboxMessage(100 - i, $"m{i}", "c", "system", "", MessageCategory.Other, MessageSeverity.Normal, null, false, false, "2026-09-29T06:00:00Z"));
            await t.SignedInAsync();
            var inbox = t.Main.Inbox;

            // (c → d) Message 4711 is not on the server: the push itself shows, read-only.
            t.Backend.ShownPushes[5] = Push(5, 4711, title: "已删除的消息");
            await inbox.ActivateNotification(5);
            Assert.True(inbox.IsLoaded); // the server was asked
            Assert.True(inbox.IsDetailReadOnly);
            Assert.Equal(("已删除的消息", (ulong?)5), (inbox.Detail!.Title, inbox.Detail.PushId));
            Assert.DoesNotContain(t.Backend.Calls, c => c.StartsWith("read", StringComparison.Ordinal));
            Assert.Empty(t.Services.Opened);
        });
    }

    [Fact]
    public async Task ActivationOfAPushWithoutAMessageShowsItReadOnly()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            t.Backend.Inbox.Add(new InboxMessage(100, "m", "c", "system", "", MessageCategory.Other, MessageSeverity.Normal, null, false, false, "2026-09-29T06:00:00Z"));
            await t.SignedInAsync();
            var inbox = t.Main.Inbox;
            var shown = new List<(AppSurface, ulong?)>();
            t.Main.ShowRequested += (surface, id) => shown.Add((surface, id));

            // (d) A broadcast: no message, no link.
            t.Backend.ShownPushes[12] = Push(12, severity: MessageSeverity.Critical);
            await inbox.ActivateNotification(12);
            Assert.Equal((AppSurface.MessageCenter, (ulong?)null), shown.Single());
            Assert.True(inbox.IsDetailOpen);
            Assert.True(inbox.IsDetailReadOnly);
            var detail = inbox.Detail!;
            Assert.Equal(("推送标题", "推送正文", MessageSeverity.Critical, StatusTone.Bad, "t_broadcast · sev_critical"),
                (detail.Title, detail.Content, detail.Severity, detail.Tone, detail.MetaText));
            Assert.Equal((true, (ulong?)12, false), (detail.IsReadOnly, detail.PushId, detail.HasLink));
            // 06:00Z is 14:00 in the test's UTC+8, 30 minutes before "now".
            Assert.Contains("14:00", detail.AbsoluteTime);
            Assert.NotEmpty(detail.RelativeTime);
            // No position, previous / next or read toggle.
            Assert.Equal(("", false, false, ""), (inbox.PositionText, inbox.CanPrevious, inbox.CanNext, inbox.DetailReadActionText));
            await inbox.ToggleDetailReadCommand.ExecuteAsync(null);
            await detail.MarkReadCommand.ExecuteAsync(null);
            await inbox.NextCommand.ExecuteAsync(null);
            Assert.Same(detail, inbox.Detail);
            // No inbox calls at all: nothing loaded, nothing marked.
            Assert.False(inbox.IsLoaded);
            Assert.Empty(t.Backend.Calls);

            // The message center opening (Refresh) keeps it; Back returns to the list.
            await inbox.RefreshCommand.ExecuteAsync(null);
            Assert.Same(detail, inbox.Detail);
            Assert.Single(inbox.Messages);
            inbox.BackCommand.Execute(null);
            Assert.False(inbox.IsDetailOpen);
            Assert.Empty(t.Backend.Calls);
        });
    }

    [Fact]
    public async Task ActivationOfAnUnknownPushOnlyOpensTheMessageCenter()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var shown = new List<(AppSurface, ulong?)>();
            t.Main.ShowRequested += (surface, id) => shown.Add((surface, id));

            // (e) Not recorded (expired, another install, or a forged id).
            await t.Main.Inbox.ActivateNotification(404);
            Assert.Equal((AppSurface.MessageCenter, (ulong?)null), shown.Single());
            Assert.False(t.Main.Inbox.IsDetailOpen);
            Assert.Empty(t.Services.Opened);
            Assert.Empty(t.Backend.Calls);
        });
    }

    [Fact]
    public async Task RefreshAndActivationBeforeSignInWait()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var restore = new TaskCompletionSource();
            using var h = new Harness(new FakeOptions { RestoreGate = restore.Task }, MemoryHooks.SignedIn());
            var inbox = h.Main.Inbox;
            Assert.Equal(AuthStage.Restoring, h.Main.Stage);

            await inbox.RefreshCommand.ExecuteAsync(null); // the window shown at launch
            var link = "https://www.peakpassvpn.com/x";
            h.Backend.RecordShownPush(Push(1, FakeClientBackend.NewestSampleId, link));
            await inbox.ActivateNotification(1);
            Assert.Equal(link, h.Services.Opened.Single()); // opened right away
            Assert.Equal(InboxState.FirstLoad, inbox.State);
            Assert.False(inbox.IsEmpty);

            restore.SetResult(); // the saved session is restored now
            await Wait.Until(() => inbox.IsLoaded, "loaded after sign-in");
            Assert.Equal(FakeClientBackend.NewestSampleId, inbox.Messages[0].Id);
            // Marked read once signed in: 3 → 2.
            await Wait.Until(() => h.Main.UnreadNotifications == 2, "deferred mark-read");
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task AMessagePushClickedBeforeSignInShowsThePushThenTheMessage()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var restore = new TaskCompletionSource();
            using var h = new Harness(new FakeOptions { RestoreGate = restore.Task }, MemoryHooks.SignedIn());
            var inbox = h.Main.Inbox;
            Assert.Equal(AuthStage.Restoring, h.Main.Stage);

            // The newest sample has a link; a push about it without one opens the message.
            h.Backend.RecordShownPush(Push(2, FakeClientBackend.NewestSampleId));
            await inbox.ActivateNotification(2);
            Assert.True(inbox.IsDetailReadOnly); // the push, until signed in
            Assert.Empty(h.Services.Opened);

            restore.SetResult();
            await Wait.Until(() => inbox.Detail is { IsReadOnly: false }, "the message replaces the push");
            Assert.Equal(FakeClientBackend.NewestSampleId, inbox.Detail!.Id);
            await Wait.Until(() => h.Main.UnreadNotifications == 2, "opened = read");
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task EmptyAndErrorStates()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { MessageCount = 0 }, MemoryHooks.SignedIn());
            var inbox = h.Main.Inbox;
            // Let the sign-in settle (profile, standard core, local proxies) before driving the inbox.
            await h.ReadyAsync();
            Assert.Equal(InboxState.FirstLoad, inbox.State);
            var load = inbox.RefreshCommand.ExecuteAsync(null);
            Assert.True(inbox.IsLoading);
            await load;
            Assert.Equal(InboxState.Empty, inbox.State);
            Assert.True(inbox.IsEmpty);
            Assert.False(inbox.IsExhausted);

            h.Backend.PushMessage("新消息", "内容", push: true);
            await Wait.Until(() => h.Main.UnreadNotifications == 1, "badge");
            await inbox.RefreshCommand.ExecuteAsync(null);
            Assert.Single(inbox.Messages);
            Assert.Equal(InboxState.Ready, inbox.State);

            // Signing out clears the list and the badge.
            await h.Main.SignOutCommand.ExecuteAsync(null);
            await Wait.Until(() => h.Main.Stage == AuthStage.SignedOut, "signed out");
            Assert.Empty(inbox.Messages);
            Assert.Equal(InboxState.FirstLoad, inbox.State);
            Assert.Equal(0, h.Main.UnreadNotifications);
        });
    }

    [Fact]
    public async Task LoadFailureKeepsTheCache()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            for (ulong i = 0; i < 3; i++)
                t.Backend.Inbox.Add(new InboxMessage(100 - i, $"m{i}", "c", "system", "", MessageCategory.Other, MessageSeverity.Normal, null, false, true, "2026-09-29T06:00:00Z"));
            await t.SignedInAsync();
            var inbox = t.Main.Inbox;
            await inbox.RefreshCommand.ExecuteAsync(null);
            Assert.Equal(3, inbox.Messages.Count);

            t.Backend.FailNotifications = true;
            await inbox.RetryCommand.ExecuteAsync(null);
            Assert.True(inbox.HasLoadError);
            Assert.Equal(3, inbox.Messages.Count);
            Assert.Equal(InboxState.Ready, inbox.State);
            Assert.Empty(t.Prompts.Errors); // an error bar, not a dialog

            t.Backend.FailNotifications = false;
            await inbox.RetryCommand.ExecuteAsync(null);
            Assert.False(inbox.HasLoadError);
        });
    }

    static InboxMessage Message(ulong id, bool read = true) =>
        new(id, $"m{id}", "c", "system", "", MessageCategory.Other, MessageSeverity.Normal, null, false, read, "2026-09-29T06:00:00Z");

    /// <summary>Signed in with <paramref name="count"/> messages (ids 100 down), the first page loaded, a short debounce.</summary>
    static async Task<Scripted> LoadedAsync(int count, TimeSpan? debounce = null)
    {
        var t = new Scripted();
        for (var i = 0; i < count; i++) t.Backend.Inbox.Add(Message((ulong)(100 - i)));
        await t.SignedInAsync();
        t.Main.Inbox.LiveRefreshDelay = debounce ?? TimeSpan.FromMilliseconds(150);
        await t.Main.Inbox.RefreshCommand.ExecuteAsync(null);
        t.Backend.NotificationPages.Clear();
        return t;
    }

    static Task UnreadAsync(Scripted t, uint unread) => t.PushAsync(t.Main.Snapshot with { UnreadNotifications = unread });

    [Fact]
    public async Task RisingUnreadRefreshesTheLoadedListAndKeepsTheDetail()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = await LoadedAsync(25);
            var inbox = t.Main.Inbox;
            Assert.Equal((20, 25), (inbox.Messages.Count, inbox.Total));
            var reading = inbox.Messages[2];
            reading.ShowDetailCommand.Execute(null);
            Assert.Equal("3 / 25", inbox.PositionText);

            // Two messages arrive; the crate's poll reports them.
            t.Backend.Inbox.Insert(0, Message(200, read: false));
            t.Backend.Inbox.Insert(0, Message(201, read: false));
            await UnreadAsync(t, 2);
            await Wait.Until(() => inbox.Messages.Count == 22, "new messages prepended");
            Assert.Equal(new uint[] { 1 }, t.Backend.NotificationPages);
            Assert.Equal(new ulong[] { 201, 200, 100 }, inbox.Messages.Take(3).Select(m => m.Id));
            Assert.True(inbox.Messages[0].IsUnread);
            Assert.Equal(27, inbox.Total);
            // The message being read stays open; its position follows.
            Assert.Same(reading, inbox.Detail);
            Assert.Equal("5 / 27", inbox.PositionText);

            // Paging on continues without gaps or duplicates.
            await inbox.LoadMoreCommand.ExecuteAsync(null);
            Assert.Equal(t.Backend.Inbox.Select(m => m.Id), inbox.Messages.Select(m => m.Id));
            Assert.True(inbox.IsExhausted);
        });
    }

    [Fact]
    public async Task FallingUnreadDoesNotReload()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = await LoadedAsync(5);
            await UnreadAsync(t, 3);
            await Wait.Until(() => t.Backend.NotificationPages.Count == 1, "reload on the rise");
            await Task.Delay(100);
            t.Backend.NotificationPages.Clear();

            await UnreadAsync(t, 2); // read one
            await UnreadAsync(t, 0); // mark all read
            await Task.Delay(500);
            Assert.Empty(t.Backend.NotificationPages);
        });
    }

    [Fact]
    public async Task ABurstOfIncreasesReloadsOnce()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = await LoadedAsync(5, TimeSpan.FromMilliseconds(400));
            var inbox = t.Main.Inbox;
            for (uint n = 1; n <= 4; n++)
            {
                t.Backend.Inbox.Insert(0, Message(200 + n, read: false));
                await UnreadAsync(t, n);
            }
            await Wait.Until(() => inbox.Messages.Count == 9, "all new messages");
            await Task.Delay(600);
            Assert.Equal(new uint[] { 1 }, t.Backend.NotificationPages);
        });
    }

    [Fact]
    public async Task NoLiveRefreshBeforeTheListIsLoaded()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            t.Backend.Inbox.Add(Message(100, read: false));
            await t.SignedInAsync();
            t.Main.Inbox.LiveRefreshDelay = TimeSpan.FromMilliseconds(50);
            await UnreadAsync(t, 1);
            await Task.Delay(300);
            Assert.Empty(t.Backend.NotificationPages);
            Assert.False(t.Main.Inbox.IsLoaded);
        });
    }

    [Fact]
    public async Task LiveRefreshKeepsAReadOnlyPushDetail()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = await LoadedAsync(3);
            var inbox = t.Main.Inbox;
            t.Backend.ShownPushes[12] = Push(12);
            await inbox.ActivateNotification(12);
            var detail = inbox.Detail!;
            Assert.True(inbox.IsDetailReadOnly);

            t.Backend.Inbox.Insert(0, Message(200, read: false));
            await UnreadAsync(t, 1);
            await Wait.Until(() => inbox.Messages.Count == 4, "new message");
            Assert.Same(detail, inbox.Detail);
        });
    }

    [Fact]
    public async Task MoreNewMessagesThanAPageStartOverButKeepTheDetail()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = await LoadedAsync(5);
            var inbox = t.Main.Inbox;
            var reading = inbox.Messages[4];
            reading.ShowDetailCommand.Execute(null);
            for (ulong i = 0; i < 25; i++) t.Backend.Inbox.Insert(0, Message(300 + i, read: false));
            await UnreadAsync(t, 25);
            await Wait.Until(() => inbox.Messages[0].Id == 324, "page 1 reloaded");
            Assert.Equal((20, 30, true), (inbox.Messages.Count, inbox.Total, inbox.HasMore));
            Assert.Same(reading, inbox.Detail);
            Assert.False(inbox.HasLoadError);
        });
    }

    [Theory]
    [InlineData(MessageCategory.SubscriptionExpiring, "t_expiring")]
    [InlineData(MessageCategory.SubscriptionExpired, "t_expired")]
    [InlineData(MessageCategory.Billing, "t_bill")]
    [InlineData(MessageCategory.Order, "t_order")]
    [InlineData(MessageCategory.Route, "t_route")]
    [InlineData(MessageCategory.Announcement, "t_broadcast")]
    [InlineData(MessageCategory.Other, "t_other")]
    public void CategoryLabels(MessageCategory category, string key) =>
        Assert.Equal(key, MessageTypes.LabelKey(category));
}
