using System.Globalization;
using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

public sealed class FormattingTests
{
    static readonly TimeZoneInfo Zone = ManualTime.Beijing;
    // 2026-09-29 14:32 in UTC+8, like the prototype's clock.
    static readonly DateTimeOffset Now = new(2026, 9, 29, 14, 32, 0, TimeSpan.FromHours(8));

    [Theory]
    [InlineData(0, "zh", "刚刚")]
    [InlineData(0.5, "en", "Just now")]
    [InlineData(3, "zh", "3 分钟前")]
    [InlineData(59, "en", "59 min ago")]
    [InlineData(131, "zh", "今天 12:21")]
    [InlineData(14 * 60 + 31, "en", "Today 00:01")]
    [InlineData(1450, "zh", "昨天 14:22")]
    [InlineData(1690, "en", "Yesterday 10:22")]
    [InlineData(8 * 24 * 60, "zh", "9月21日")]
    [InlineData(8 * 24 * 60, "en", "Sep 21")]
    [InlineData(300 * 24 * 60, "zh", "2025年12月3日")]
    [InlineData(300 * 24 * 60, "en", "Dec 3, 2025")]
    public void RelativeTimeFollowsTheDesign(double minutesAgo, string language, string expected)
    {
        var strings = new JsonLocalizer(culture: CultureInfo.GetCultureInfo(language == "zh" ? "zh-CN" : "en-US"));
        Assert.Equal(expected, Formatting.RelativeTime(Now.AddMinutes(-minutesAgo), Now, Zone, strings));
    }

    [Fact]
    public void AbsoluteTimeAndLongDate()
    {
        var when = new DateTimeOffset(2026, 9, 29, 6, 29, 0, TimeSpan.Zero);
        Assert.Equal("2026年9月29日 14:29", Formatting.AbsoluteTime(when, Zone, "zh"));
        Assert.Equal("Sep 29, 2026 14:29", Formatting.AbsoluteTime(when, Zone, "en"));
        Assert.Equal("Dec 31, 2026", Formatting.LongDate(new DateTimeOffset(2026, 12, 31, 0, 0, 0, TimeSpan.Zero), "en"));
    }

    [Fact]
    public async Task RelativeTimesRefreshOnTheMinuteTick()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var time = new ManualTime(Now);
            var t = new Scripted(new JsonLocalizer(culture: CultureInfo.GetCultureInfo("zh-CN")), time);
            t.Backend.Inbox.Add(new InboxMessage(1, "a", "b", "system", "", MessageCategory.Other, MessageSeverity.Normal, null, false, false,
                Now.AddMinutes(-3).ToString("O", CultureInfo.InvariantCulture)));
            await t.SignedInAsync();
            await t.Main.Inbox.RefreshCommand.ExecuteAsync(null);
            var item = t.Main.Inbox.Messages.Single();
            Assert.Equal("3 分钟前", item.RelativeTime);
            Assert.Equal("2026年9月29日 14:29 · 3 分钟前", item.DetailTimeText);

            time.Advance(TimeSpan.FromHours(2));
            for (var i = 0; i < 60; i++) t.Main.Tick(); // the 60 s refresh
            Assert.Equal("今天 14:29", item.RelativeTime);
        });
    }

    [Theory]
    [InlineData(0UL, "0 KB/s")]
    [InlineData(512UL, "1 KB/s")]
    [InlineData(1536UL, "2 KB/s")]
    [InlineData(307_200UL, "300 KB/s")]
    [InlineData(1_048_576UL, "1.0 MB/s")]
    [InlineData(9_017_753UL, "8.6 MB/s")]
    public void RatesAreBytesPerSecond(ulong bytesPerSecond, string expected) =>
        Assert.Equal(expected, Formatting.Rate(bytesPerSecond));

    [Fact]
    public async Task TrafficShowsWhileSignedIn()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await h.ReadyAsync();
            Assert.True(h.Main.ShowTraffic);
            await Wait.Until(() => h.Main.DownRate != "0 KB/s", "traffic (standard core)");
            Assert.Matches(@"^\d+(\.\d)? (KB|MB)/s$", h.Main.DownRate);
        });
    }

    [Fact]
    public void NamedPlaceholdersAndBadges()
    {
        Assert.Equal("正在尝试 HKG-A", Placeholders.Fill("正在尝试 {r}", ("r", "HKG-A")));
        Assert.Equal("{r0} 中断，切换到 HKG-B", Placeholders.Fill("{r0} 中断，切换到 {r}", ("r", "HKG-B")));
        Assert.Equal("HKG-A · 38 ms", Placeholders.Fill("{r} · {ms}", ("r", "HKG-A"), ("ms", "38 ms")));
        Assert.Equal(new HashSet<string> { "r0", "r" }, Placeholders.Names("{r0} 中断，切换到 {r}"));
        Assert.Equal(("", "1", "99", "99+"), (Formatting.Badge(0), Formatting.Badge(1), Formatting.Badge(99), Formatting.Badge(100)));
        Assert.Equal("9:54", Formatting.Countdown(TimeSpan.FromSeconds(594)));
        Assert.Equal("0:00", Formatting.Countdown(TimeSpan.FromSeconds(-3)));
    }

    [Fact]
    public void PromptTextsComeFromTheCatalog()
    {
        var strings = new JsonLocalizer(culture: CultureInfo.GetCultureInfo("zh-CN"));
        Assert.Equal(new PromptTexts("安装 PPVPN 系统服务", strings.Get("installD"), "继续", "取消"), PromptTexts.For(PromptKind.InstallService, strings));
        Assert.Equal("卸载", PromptTexts.For(PromptKind.UninstallService, strings).Confirm);
        Assert.Equal("退出登录？", PromptTexts.For(PromptKind.SignOut, strings).Title);
    }

    [Fact]
    public void RoutesShowLabelsNeverEndpointKeys()
    {
        Assert.Null(Formatting.RouteLabel(null, "5086"));
        var replica = new Replica("5086", 0, "vless", null);
        Assert.Null(Formatting.RouteLabel(replica));
        Assert.Equal("HKG-A", Formatting.RouteLabel(replica with { Label = "HKG-A" }));
        var node = new Node("n1", "中国-203.0.113.49", "standard", null, null, "cn", true, [replica]);
        Assert.Null(Formatting.RouteLabel(node, "5086"));

        // Unlabelled lines are numbered in failover order; a single one shows nothing.
        // Listed backwards: the order comes from ReplicaOrdinal.
        static Node With(params string?[] labels) => new("n1", "N", "standard", null, null, "cn", true,
            labels.Select((label, i) => new Replica($"508{i}", (uint)i, "vless", label)).Reverse().ToArray());
        var keys = new KeyLocalizer();
        Assert.Equal("", Formatting.Routes(With((string?)null), keys));
        Assert.Equal("HKG-A", Formatting.Routes(With("HKG-A"), keys));
        Assert.Equal("HKG-A → HKG-B", Formatting.Routes(With("HKG-A", "HKG-B"), keys));
        Assert.Equal("routeN(n=1) → routeN(n=2)", Formatting.Routes(With(null, null), keys));
        Assert.Equal("HKG-A → routeN(n=2)", Formatting.Routes(With("HKG-A", null), keys));
        var zh = new JsonLocalizer(culture: CultureInfo.GetCultureInfo("zh-CN"));
        var en = new JsonLocalizer(culture: CultureInfo.GetCultureInfo("en-US"));
        Assert.Equal("线路 1 → 线路 2", Formatting.Routes(With(null, null), zh));
        Assert.Equal("Route 1 → Route 2", Formatting.Routes(With(null, null), en));
    }
}
