using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

/// <summary>Derivations of the connection card, title bar, switch and tray from pushed snapshots (core.js vals()).</summary>
public sealed class ConnectionTests
{
    static Func<ClientSnapshot, ClientSnapshot> Conn(ConnectionPhase phase, ClientErrorInfo? reason = null, string? endpoint = null,
        string? previous = null, uint? latency = null, bool canTakeOver = false, bool suggestCompatible = false,
        ConnectionMode method = ConnectionMode.Enhanced, string? competitor = null, bool proxyWasForeign = false) =>
        Scripted.Conn(phase, reason, endpoint, previous, latency, canTakeOver, suggestCompatible, method, competitor, proxyWasForeign);

    [Fact]
    public async Task IdleShowsNotConnectedWithTheLocalProxyStillAvailable()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            Assert.Equal(ConnectionMode.Enhanced, main.ConnectionMode);
            Assert.Equal(ConnectState.Off, main.ConnectState);
            Assert.Equal(("h_idle", "d_idle", ConnectionTone.Idle), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));
            Assert.Equal("h_idle", main.StatusLine);
            Assert.Equal(ConnectionTone.Idle, main.StatusDotTone);
            Assert.Equal(("香港 01", "hk", "38 ms"), (main.CurrentNodeName, main.CurrentNodeCountryCode, main.CurrentNodeLatencyText));
            Assert.Equal("captionEnhanced", main.ConnectionMethodCaption);
            Assert.Equal((SwitchVisual.Off, true), (main.ConnectSwitch, main.ConnectSwitchEnabled));
            Assert.Empty(main.Notices);
            Assert.True(main.ShowTraffic);
            Assert.Equal("0 KB/s", main.UpRate);

            // Local proxy of the current node: shared port, user name per node, masked password.
            var proxy = main.CurrentNodeProxy!;
            Assert.Equal(("http://127.0.0.1:7890", "socks5://127.0.0.1:7890", "u8f2k-hk1", "••••••"),
                (proxy.HttpDisplay, proxy.SocksDisplay, proxy.Username, proxy.MaskedPassword));
            proxy.CopyHttpCommand.Execute(null);
            Assert.Equal("http://u8f2k-hk1:secret@127.0.0.1:7890", t.Services.Copied.Single());
            proxy.CopyUsernameCommand.Execute(null);
            proxy.CopyPasswordCommand.Execute(null);
            Assert.Equal(["u8f2k-hk1", "secret"], t.Services.Copied.Skip(1));
            // Masked by default; the eye toggles it.
            Assert.Equal((false, "••••••"), (proxy.PasswordRevealed, proxy.PasswordDisplay));
            proxy.TogglePasswordRevealedCommand.Execute(null);
            Assert.Equal((true, "secret"), (proxy.PasswordRevealed, proxy.PasswordDisplay));
            proxy.TogglePasswordRevealedCommand.Execute(null);
            Assert.Equal("••••••", proxy.PasswordDisplay);
            Assert.Null(main.ProxyUnavailableText);
        });
    }

    [Fact]
    public async Task TheProxyCardFollowsTheRulesByDefaultAndCanPinTheNode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            // Default: the routed user (bare prefix), SOCKS URLs resolve at the proxy.
            Assert.Equal(LocalProxyScope.Routed, main.SelectedProxyScope.Scope);
            Assert.Equal(["proxyRouted", "proxyNode"], main.ProxyScopes.Select(o => o.Title));
            var shown = main.ShownProxy!;
            Assert.Same(main.RoutedProxy, shown);
            Assert.Equal(("u8f2k", "socks5h://127.0.0.1:7890", true), (shown.Username, shown.SocksDisplay, shown.IsRouted));
            shown.CopySocksCommand.Execute(null);
            Assert.Equal("socks5h://u8f2k:secret@127.0.0.1:7890", t.Services.Copied.Last());

            // This node only.
            var changed = new List<string?>();
            main.PropertyChanged += (_, e) => changed.Add(e.PropertyName);
            main.SelectedProxyScope = main.ProxyScopes[1];
            Assert.Same(main.CurrentNodeProxy, main.ShownProxy);
            Assert.Contains(nameof(MainViewModel.ShownProxy), changed);
            Assert.Equal("socks5://127.0.0.1:7890", main.ShownProxy!.SocksDisplay);

            // A null written back by a view is ignored.
            changed.Clear();
            main.SelectedProxyScope = null!;
            main.SelectedProxyScope = main.ProxyScopes[1];
            Assert.Equal(LocalProxyScope.Node, main.SelectedProxyScope.Scope);
            // No re-raise: a two-way ComboBox would write back again.
            Assert.DoesNotContain(nameof(MainViewModel.SelectedProxyScope), changed);
            Assert.Equal("proxyNote", main.ProxyNote);
            main.SelectedProxyScope = main.ProxyScopes[0];
            Assert.Equal("proxyNoteRouted", main.ProxyNote);
        });
    }

    [Fact]
    public async Task WithoutTheRoutedUserTheCardShowsTheNode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            t.Backend.RoutedSupported = false;
            await t.SignedInAsync();
            var main = t.Main;
            Assert.False(main.HasRoutedProxy);
            Assert.Same(main.CurrentNodeProxy, main.ShownProxy);
        });
    }

    [Fact]
    public async Task EnhancedTitleRules()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;

            (Func<ClientSnapshot, ClientSnapshot> Pending, string Title, string Detail, ConnectionTone Tone)[] cases =
            [
                (Conn(ConnectionPhase.Preparing), "st_preparing", "d_preparing", ConnectionTone.Busy),
                (Conn(ConnectionPhase.WaitingPermission), "st_authorizing", "d_authorizing", ConnectionTone.Busy),
                (Conn(ConnectionPhase.Connecting, endpoint: "hk1-r0"), "st_connecting", "d_connecting(r=HKG-A)", ConnectionTone.Busy),
                (Conn(ConnectionPhase.On, endpoint: "hk1-r1", latency: 41), "h_on", "d_on(r=HKG-B, ms=41 ms)", ConnectionTone.Ok),
                (Conn(ConnectionPhase.Reconnecting, endpoint: "hk1-r1", previous: "hk1-r0"), "st_reconnecting", "d_reconnecting(r0=HKG-A, r=HKG-B)", ConnectionTone.Busy),
                (Conn(ConnectionPhase.Disconnecting), "st_disconnecting", "d_disconnecting", ConnectionTone.Busy),
                (Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "x")), "h_failed", "", ConnectionTone.Error),
                (Conn(ConnectionPhase.Contended, new(ErrorCode.ServiceBusy, "x"), canTakeOver: true), "h_occupied", "", ConnectionTone.Warn),
                (Conn(ConnectionPhase.Off), "h_idle", "d_idle", ConnectionTone.Idle),
            ];
            foreach (var c in cases)
            {
                await t.PushAsync(s with { }, c.Pending);
                Assert.Equal((c.Title, c.Detail, c.Tone), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));
            }

            // A line without a label: its endpoint key is internal, so the {r} part is left out.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.On, endpoint: "5086", latency: 12));
            Assert.Equal("12 ms", main.ConnectionDetail);
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Connecting, endpoint: "5086"));
            Assert.Equal("", main.ConnectionDetail);
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Reconnecting, endpoint: "hk1-r1", previous: "5086"));
            Assert.Equal("d_connecting(r=HKG-B)", main.ConnectionDetail);
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Reconnecting, endpoint: "5086", previous: "hk1-r0"));
            Assert.Equal("", main.ConnectionDetail);
            await t.PushAsync(s with { }, Conn(ConnectionPhase.On, endpoint: "5086", latency: 11, method: ConnectionMode.Compatible));
            Assert.Equal("11 ms", main.ConnectionDetail);
        });
    }

    [Fact]
    public async Task CompatibleModeUsesTheSystemProxyWording()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;
            const ConnectionMode compatible = ConnectionMode.Compatible;

            await t.PushAsync(s with { }, Conn(ConnectionPhase.Connecting, endpoint: "hk1-r0", method: compatible));
            Assert.Equal(("h_stdStarting", "", ConnectionTone.Busy), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));
            Assert.Equal("captionCompatible", main.ConnectionMethodCaption);
            Assert.False(main.ShowInstallHint);

            await t.PushAsync(s with { }, Conn(ConnectionPhase.On, endpoint: "hk1-r0", latency: 38, method: compatible));
            Assert.Equal(("h_on", "d_stdOn(r=HKG-A, ms=38 ms)", ConnectionTone.Ok), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));
            Assert.Equal("h_on", main.StatusLine);

            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.SystemProxyFailed, "taken"), method: compatible));
            Assert.Equal(("h_stdFail", "fr_port", ConnectionTone.Error), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));
            var notice = Assert.Single(main.Notices);
            Assert.Equal(("stdMode · std_failed", "fr_port", "retry", null), (notice.Title, notice.Message, notice.ActionText, notice.SecondaryActionText));
            await notice.Action!.ExecuteAsync(null);
            Assert.Equal("retry", t.Backend.Calls.Last());

            // A desktop without proxy settings (Linux without gsettings / kwriteconfig): say so.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error,
                new(ErrorCode.SystemProxyUnavailable, "NO_DESKTOP_PROXY_SETTINGS: neither gsettings nor kwriteconfig found"), method: compatible));
            Assert.Equal(("h_stdFail", "sysproxyUnavailable"), (main.ConnectionTitle, main.ConnectionDetail));
            Assert.Equal("sysproxyUnavailable", Assert.Single(main.Notices).Message);
        });
    }

    [Fact]
    public async Task TheSwitchIsTriStateAndStaysClickableWhileConnecting()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;

            (ConnectionPhase Phase, SwitchVisual Visual, bool Enabled)[] cases =
            [
                (ConnectionPhase.Off, SwitchVisual.Off, true),
                (ConnectionPhase.Preparing, SwitchVisual.Indeterminate, false),
                (ConnectionPhase.WaitingPermission, SwitchVisual.Indeterminate, false),
                (ConnectionPhase.Connecting, SwitchVisual.Indeterminate, true),
                (ConnectionPhase.On, SwitchVisual.On, true),
                (ConnectionPhase.Reconnecting, SwitchVisual.Indeterminate, true),
                (ConnectionPhase.Disconnecting, SwitchVisual.Indeterminate, false),
                (ConnectionPhase.Error, SwitchVisual.Off, true),
                (ConnectionPhase.Contended, SwitchVisual.Off, true),
            ];
            foreach (var c in cases)
            {
                var reason = c.Phase is ConnectionPhase.Error or ConnectionPhase.Contended ? new ClientErrorInfo(ErrorCode.ServiceBusy, "x") : null;
                await t.PushAsync(s with { }, Conn(c.Phase, reason));
                Assert.Equal((c.Phase, c.Visual, c.Enabled), (c.Phase, main.ConnectSwitch, main.ConnectSwitchEnabled));
            }

            // Tapping while connecting cancels (disconnect); from failed it retries; from off it connects.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Connecting, endpoint: "hk1-r0"));
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal("disconnect", t.Backend.Calls.Last());
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "x")));
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal("retry", t.Backend.Calls.Last());
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Off));
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal("connect", t.Backend.Calls.Last());
            // Preparing: the tap does nothing.
            var calls = t.Backend.Calls.Count;
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Preparing));
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(calls, t.Backend.Calls.Count);

            // Without a profile (restricted) the switch is disabled.
            await t.PushAsync(s with { Profile = null, ProfileStatus = new ProfileStatus.NoSubscription() }, Conn(ConnectionPhase.Off));
            Assert.False(main.ConnectSwitchEnabled);
        });
    }

    [Fact]
    public async Task NoticesOfferRetryCompatibilityAndTakeOver()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;

            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "HEALTH")));
            var failed = Assert.Single(main.Notices);
            Assert.Equal(("tunMode · st_failed", "fr_timeout", "retry", null), (failed.Title, failed.Message, failed.ActionText, failed.SecondaryActionText));

            // A denied admin prompt: fr_auth, and compatibility mode is offered.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ServiceInstallCancelled, "x"), suggestCompatible: true));
            failed = Assert.Single(main.Notices);
            Assert.Equal(("fr_auth", "useCompatible"), (failed.Message, failed.SecondaryActionText));
            await failed.SecondaryAction!.ExecuteAsync(null);
            Assert.Equal(["mode Compatible", "retry"], t.Backend.Calls.TakeLast(2));

            // Another reason: its error text.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ServiceIncompatible, "x")));
            Assert.Equal("Error_ServiceIncompatible", Assert.Single(main.Notices).Message);

            // Occupied, take-over offered.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.ServiceBusy, "x"), canTakeOver: true));
            var occupied = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Occupied, ConnectionTone.Warn, "st_occupied", "d_occupied", "takeOver"),
                (occupied.Kind, occupied.Tone, occupied.Title, occupied.Message, occupied.ActionText));
            await occupied.Action!.ExecuteAsync(null);
            Assert.Equal("enhanced_take_over", t.Backend.Calls.Last());

            // Another OS user: no take-over.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.ServiceOwnedByAnotherUser, "x")));
            occupied = Assert.Single(main.Notices);
            Assert.Equal(("Error_ServiceOwnedByAnotherUser", null), (occupied.Message, occupied.ActionText));

            // Path contended, nobody named: an ordinary failure (no "taken over"), compatibility mode offered.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.NetworkPathContended, "x"), suggestCompatible: true));
            Assert.Equal(ConnectState.Failed, main.ConnectState);
            failed = Assert.Single(main.Notices);
            Assert.Equal(("fr_timeout", "useCompatible"), (failed.Message, failed.SecondaryActionText));
        });
    }

    [Fact]
    public async Task ConnectionFailuresShowTheNoticeWithoutADialog()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;
            var reason = new ClientErrorInfo(ErrorCode.ConnectHealthCheckFailed, "HEALTH_DIRECT_PATH_FAILED");
            var failed = Conn(ConnectionPhase.Error, reason, suggestCompatible: true)(s);

            // The crate fails the call after publishing Error, before the VM has applied it.
            t.Backend.Stage(failed);
            t.Backend.Fail = new ClientException.Failed(reason.Code, reason.Detail);
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal("connect", t.Backend.Calls.Last());
            Assert.Empty(t.Prompts.Errors);
            await t.PushAsync(failed);
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Failed, "tunMode · st_failed", "Error_ConnectHealthCheckFailed", "retry", "useCompatible"),
                (notice.Kind, notice.Title, notice.Message, notice.ActionText, notice.SecondaryActionText));

            // Retry, and a mode switch, failing the same way: still only the notice.
            t.Backend.Fail = new ClientException.Failed(reason.Code, reason.Detail);
            await notice.Action!.ExecuteAsync(null);
            Assert.Equal("retry", t.Backend.Calls.Last());
            t.Backend.Fail = new ClientException.Failed(reason.Code, reason.Detail);
            await main.SetConnectionModeAsync(ConnectionMode.Compatible);
            Assert.Equal("mode Compatible", t.Backend.Calls.Last());
            Assert.Empty(t.Prompts.Errors);

            // Taken over elsewhere while taking over: the occupied notice shows it.
            var occupied = Conn(ConnectionPhase.Contended, new(ErrorCode.ServiceBusy, "x"), canTakeOver: true)(s);
            await t.PushAsync(occupied);
            t.Backend.Fail = new ClientException.Failed(ErrorCode.ServiceBusy, "CONNECTION_OWNED_BY_ANOTHER_SESSION");
            await Assert.Single(main.Notices).Action!.ExecuteAsync(null);
            Assert.Equal("enhanced_take_over", t.Backend.Calls.Last());
            Assert.Empty(t.Prompts.Errors);

            // A failure no notice shows (the connection is off) is still a dialog.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Off));
            t.Backend.Fail = new ClientException.Failed(ErrorCode.ServiceUnavailable, "connect: refused");
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(("errorT", "Error_ServiceUnavailable"), Assert.Single(t.Prompts.Errors));
        });
    }

    [Fact]
    public async Task ANamedCompetitorTurnsTheFailureIntoAConflictNotice()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;

            // Surge's enhanced mode on macOS: the health check fails and the crate names Surge.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectHealthCheckFailed, "HEALTH_CAPTURE_PATH_FAILED"),
                suggestCompatible: true, competitor: "Surge"));
            Assert.Equal((ConnectState.Failed, "h_pathContended", ConnectionTone.Error), (main.ConnectState, main.ConnectionTitle, main.ConnectionTone));
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Conflict, ConnectionTone.Error, "conflictT", "conflictD(app=Surge)", "retry", "useCompatible"),
                (notice.Kind, notice.Tone, notice.Title, notice.Message, notice.ActionText, notice.SecondaryActionText));
            await notice.SecondaryAction!.ExecuteAsync(null);
            Assert.Contains("mode Compatible", t.Backend.Calls);

            // Contended by another VPN holding the route: the same conflict notice.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.NetworkPathContended, "NETWORK_PATH_CONTENDED"),
                suggestCompatible: true, competitor: "Clash Verge"));
            notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Conflict, "conflictD(app=Clash Verge)", "useCompatible"), (notice.Kind, notice.Message, notice.SecondaryActionText));

            // A health step failing through the node, competitor named, compatibility not suggested: retry only.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "HEALTH_PROXY_PATH_FAILED"), competitor: "Tailscale"));
            notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Conflict, "conflictD(app=Tailscale)", "retry", null), (notice.Kind, notice.Message, notice.ActionText, notice.SecondaryActionText));
            await notice.Action!.ExecuteAsync(null);
            Assert.Equal("retry", t.Backend.Calls.Last());

            // Nobody named: the plain failed notice and headline.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectHealthCheckFailed, "HEALTH_DIRECT_PATH_FAILED"), suggestCompatible: true));
            Assert.Equal("h_failed", main.ConnectionTitle);
            Assert.Equal((NoticeKind.Failed, "Error_ConnectHealthCheckFailed"), (Assert.Single(main.Notices).Kind, main.Notices[0].Message));
            // Path contended with nobody named (e.g. the reconnect after a service restart failed):
            // never "taken over", an ordinary connection failure.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.NetworkPathContended, "NETWORK_PATH_CONTENDED")));
            Assert.Equal((ConnectState.Failed, "h_failed"), (main.ConnectState, main.ConnectionTitle));
            var plain = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Failed, "tunMode · st_failed", "fr_timeout", "retry"), (plain.Kind, plain.Title, plain.Message, plain.ActionText));

            // Compatible mode on after replacing Surge's system proxy: not a failure, no notice.
            await t.PushAsync(s with { }, Conn(ConnectionPhase.On, method: ConnectionMode.Compatible, competitor: "Surge", proxyWasForeign: true));
            Assert.Equal(ConnectState.On, main.ConnectState);
            Assert.Empty(main.Notices);
        });
    }

    [Theory]
    [InlineData("zh-CN", "检测到其他代理软件", "Surge 正在接管本机网络，同时使用可能无法连接。建议先关闭它的增强模式或系统代理。", "网络被其他软件接管")]
    [InlineData("en-US", "Another Proxy App Is Active", "Surge is routing this device’s traffic. Using both may fail; turn off its enhanced mode or system proxy first.", "Network Taken Over")]
    public async Task ConflictNoticeIsLocalized(string culture, string title, string message, string headline)
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(new JsonLocalizer(culture: System.Globalization.CultureInfo.GetCultureInfo(culture)));
            await t.SignedInAsync();
            await t.PushAsync(t.Main.Snapshot, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectHealthCheckFailed, "HEALTH_CAPTURE_PATH_FAILED"),
                suggestCompatible: true, competitor: "Surge"));
            var notice = Assert.Single(t.Main.Notices);
            Assert.Equal((title, message, headline), (notice.Title, notice.Message, t.Main.ConnectionTitle));
        });
    }

    [Theory]
    [InlineData("zh-CN", "连接失败", "所有线路均无法连接（连接超时）。", "此桌面环境不支持设置系统代理。请改用增强模式，或在应用中手动填写本地代理。")]
    [InlineData("en-US", "Connection Failed", "All routes failed to connect (timed out).", "This desktop doesn't expose system proxy settings. Use Enhanced Mode, or enter the local proxy in your apps.")]
    public async Task FailuresWithoutACompetitorOrProxySettingsAreLocalized(string culture, string headline, string contended, string unavailable)
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(new JsonLocalizer(culture: System.Globalization.CultureInfo.GetCultureInfo(culture)));
            await t.SignedInAsync();
            var s = t.Main.Snapshot;
            await t.PushAsync(s with { }, Conn(ConnectionPhase.Contended, new(ErrorCode.NetworkPathContended, "NETWORK_PATH_CONTENDED")));
            Assert.Equal((headline, contended), (t.Main.ConnectionTitle, Assert.Single(t.Main.Notices).Message));

            await t.PushAsync(s with { }, Conn(ConnectionPhase.Error, new(ErrorCode.SystemProxyUnavailable, "NO_DESKTOP_PROXY_SETTINGS"),
                method: ConnectionMode.Compatible));
            Assert.Equal(unavailable, Assert.Single(t.Main.Notices).Message);
        });
    }

    [Theory]
    [InlineData("zh-CN", "连接后无法访问服务，可能是 DNS 或网络被其他软件接管。")]
    [InlineData("en-US", "Connected, but the service couldn’t be reached. Another app may have taken over DNS or the network.")]
    public async Task HealthCheckFailureNamesTheUnreachableService(string culture, string expected)
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted(new JsonLocalizer(culture: System.Globalization.CultureInfo.GetCultureInfo(culture)));
            await t.SignedInAsync();
            var main = t.Main;
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectHealthCheckFailed, "HEALTH_DIRECT_STATUS_FAILED: 404")));
            Assert.Equal(expected, Assert.Single(main.Notices).Message);

            // A route failure keeps the timed-out wording.
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "HEALTH_ENTRANCE_FAILED")));
            Assert.Equal(new JsonLocalizer(culture: System.Globalization.CultureInfo.GetCultureInfo(culture)).Get("fr_timeout"),
                Assert.Single(main.Notices).Message);
        });
    }

    [Fact]
    public async Task APinnedLineStaysPinnedAndSaysWhenItIsDown()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var hk1 = main.Nodes.Items.First(i => i.Id == "hk1");
            Assert.Equal(["lineAuto", "HKG-A", "HKG-B", "SZX-R"], hk1.Lines.Select(l => l.Title));
            // One list until the lines change (a new one resets a view's selection).
            Assert.Same(hk1.Lines, hk1.Lines);
            Assert.Equal(((string?)null, false), (hk1.SelectedLine!.EndpointKey, hk1.IsPinned));

            // The picker pins; automatic unpins.
            hk1.SelectedLine = hk1.Lines[2];
            await Wait.Until(() => t.Backend.Calls.Contains("pin hk1 hk1-r1"), "pinned");

            // Pinned to HKG-B, which the core reports down: pinned still, the overview says so.
            NodeIngresses Reported(bool healthy) => new("hk1", "hk1-r1",
            [
                new("hk1-r0", "primary", "HKG-A", true, false),
                new("hk1-r1", "backup", "HKG-B", healthy, true),
            ]);
            await t.PushAsync(main.Snapshot with
            {
                IngressPins = [new IngressPin("hk1", "hk1-r1")],
                NodeIngresses = [Reported(false)],
            });
            Assert.Equal(("hk1-r1", true, true, "hk1-r1"),
                (hk1.SelectedLine!.EndpointKey, hk1.IsPinned, hk1.PinnedUnavailable, hk1.ActiveEndpointKey));
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.IngressUnavailable, ConnectionTone.Warn, "ingressDownT", "ingressDownD(n=香港 01, r=HKG-B)", "backToAuto"),
                (notice.Kind, notice.Tone, notice.Title, notice.Message, notice.ActionText));
            await notice.Action!.ExecuteAsync(null);
            Assert.Contains("pin hk1 auto", t.Backend.Calls);

            // Healthy again: no notice.
            await t.PushAsync(main.Snapshot with { NodeIngresses = [Reported(true)] });
            Assert.Empty(main.Notices);

            // A refresh dropped the pinned line: back to automatic, said once.
            await t.PushAsync(main.Snapshot with { IngressPins = [], ClearedIngressPins = [new IngressPin("hk1", "gone")] });
            notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.IngressPinCleared, "pinClearedT", "pinClearedD(n=香港 01)", "ok"),
                (notice.Kind, notice.Title, notice.Message, notice.ActionText));
            await notice.Action!.ExecuteAsync(null);
            Assert.Contains("dismiss_cleared_pins", t.Backend.Calls);
            Assert.Equal(((string?)null, false), (hk1.SelectedLine!.EndpointKey, hk1.IsPinned));
        });
    }

    [Fact]
    public async Task AnAutomaticSwitchToAnotherLineIsMentionedLightly()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.On, endpoint: "hk1-r1", previous: "hk1-r0", latency: 41));
            Assert.Equal("d_switched(r=HKG-B, ms=41 ms)", main.ConnectionDetail);
            Assert.Empty(main.Notices);
            // Pinned nodes never switch on their own: the plain line.
            await t.PushAsync(main.Snapshot with { IngressPins = [new IngressPin("hk1", "hk1-r1")] });
            Assert.Equal("d_on(r=HKG-B, ms=41 ms)", main.ConnectionDetail);
        });
    }

    [Fact]
    public async Task TheLineShownFollowsThePinTheCoreAndCompatibleMode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;

            // Pinned to SZX-R: that line, whatever the core last reported.
            await t.PushAsync(main.Snapshot with { IngressPins = [new IngressPin("hk1", "hk1-r2")] },
                Conn(ConnectionPhase.Connecting, endpoint: "hk1-r1"));
            Assert.Equal("d_connecting(r=SZX-R)", main.ConnectionDetail);

            // No report from the core: the line it marks active, not a guess at the primary.
            await t.PushAsync(main.Snapshot with
            {
                IngressPins = [],
                NodeIngresses = [new NodeIngresses("hk1", null,
                [
                    new("hk1-r0", "primary", "HKG-A", false, false),
                    new("hk1-r1", "backup", "HKG-B", true, true),
                ])],
            }, Conn(ConnectionPhase.On, latency: 41));
            Assert.Equal("d_on(r=HKG-B, ms=41 ms)", main.ConnectionDetail);
            // Nothing known about a multi-line node: no line at all.
            await t.PushAsync(main.Snapshot with { NodeIngresses = [] }, Conn(ConnectionPhase.On, latency: 41));
            Assert.Equal("41 ms", main.ConnectionDetail);

            // Compatible mode mentions an automatic switch as enhanced mode does.
            await t.PushAsync(main.Snapshot,
                Conn(ConnectionPhase.On, endpoint: "hk1-r1", previous: "hk1-r0", latency: 41, method: ConnectionMode.Compatible));
            Assert.Equal("d_switched(r=HKG-B, ms=41 ms)", main.ConnectionDetail);
        });
    }

    [Fact]
    public async Task UnavailableRoutingRulesShowALowKeyNoticeUntilTheyLoad()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;

            await t.PushAsync(main.Snapshot with { RuleSetsUnavailable = ["cn-site"] });
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.RulesUnavailable, ConnectionTone.Warn, "rulesT", "rulesUnavailableD", null, null, null, null),
                (notice.Kind, notice.Tone, notice.Title, notice.Message, notice.ActionText, notice.Action, notice.SecondaryActionText, notice.SecondaryAction));
            // Informational only: the connection itself is unaffected.
            Assert.Equal(("h_idle", "d_idle", ConnectionTone.Idle), (main.ConnectionTitle, main.ConnectionDetail, main.ConnectionTone));

            // Connected: still shown, after the connection's own notices.
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.On, endpoint: "11", latency: 38));
            Assert.Equal(NoticeKind.RulesUnavailable, Assert.Single(main.Notices).Kind);
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.Error, new(ErrorCode.ConnectFailed, "x")));
            Assert.Equal([NoticeKind.Failed, NoticeKind.RulesUnavailable], main.Notices.Select(n => n.Kind));
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.On, endpoint: "11", latency: 38));

            // Loaded: it clears by itself.
            await t.PushAsync(main.Snapshot with { RuleSetsUnavailable = [] });
            Assert.Empty(main.Notices);

            // Not while restricted.
            await t.PushAsync(main.Snapshot with
            {
                RuleSetsUnavailable = ["cn-ip", "cn-site"],
                Profile = null,
                ProfileStatus = new ProfileStatus.NoSubscription(),
            }, Conn(ConnectionPhase.Off));
            Assert.Empty(main.Notices);
        });
    }

    [Fact]
    public async Task FailedStandardCoreShowsTheLocalProxyUnavailable()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var failed = new StandardState.Failed(new(ErrorCode.StandardCoreFailed, "profile rejected"));

            // Off in either mode: not "local proxy still available".
            await t.PushAsync(main.Snapshot with { Standard = failed });
            Assert.Equal(("h_idle", "d_idleProxyFailed"), (main.ConnectionTitle, main.ConnectionDetail));
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.ProxyFailed, ConnectionTone.Error, "proxyFailedT", "Error_StandardCoreFailed", "retry", null),
                (notice.Kind, notice.Tone, notice.Title, notice.Message, notice.ActionText, notice.SecondaryActionText));
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.Off, method: ConnectionMode.Compatible));
            Assert.Equal(("h_idle", "d_idleProxyFailed"), (main.ConnectionTitle, main.ConnectionDetail));
            Assert.Equal(NoticeKind.ProxyFailed, Assert.Single(main.Notices).Kind);

            // Retry refetches the profile.
            await Assert.Single(main.Notices).Action!.ExecuteAsync(null);
            Assert.Equal("refresh", t.Backend.Calls.Last());

            // Next to a connection failure.
            await t.PushAsync(main.Snapshot, Conn(ConnectionPhase.Error, new(ErrorCode.SystemProxyFailed, "x"), method: ConnectionMode.Compatible));
            Assert.Equal([NoticeKind.Failed, NoticeKind.ProxyFailed], main.Notices.Select(n => n.Kind));

            // Running again: back to normal.
            await t.PushAsync(main.Snapshot with { Standard = new StandardState.Ready("rev-1") }, Conn(ConnectionPhase.Off));
            await Wait.Until(() => main.HasCurrentNodeProxy, "proxies reloaded");
            Assert.Equal(("h_idle", "d_idle"), (main.ConnectionTitle, main.ConnectionDetail));
            Assert.Empty(main.Notices);
            Assert.Null(main.ProxyUnavailableText);
        });
    }

    [Fact]
    public async Task RestrictedAccessReplacesTheCardAndDimsTheTitleBar()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            var s = main.Snapshot;

            await t.PushAsync(s with { Profile = null, Standard = new StandardState.Stopped(), ProfileStatus = new ProfileStatus.SubscriptionExpired("2026-09-01T12:00:00Z") });
            Assert.Equal(AccessState.Expired, main.Access);
            Assert.True(main.IsRestricted);
            Assert.Equal(("expiredT", "expiredD(d=2026年9月1日)"), (main.RestrictedTitle, main.RestrictedMessage));
            Assert.True(main.CanBuy);
            Assert.False(main.CanSwitchTeam);
            Assert.Equal("expiredT", main.StatusLine);
            Assert.Equal(ConnectionTone.Idle, main.StatusDotTone);
            Assert.False(main.ShowTraffic);
            Assert.Equal(NodesViewState.Restricted, main.Nodes.ViewState);
            Assert.Equal("expiredOn(d=2026年9月1日)", main.ExpiresText);
            Assert.True(main.IsExpired);
            Assert.Equal("expiredT", main.AccountSubtitle);

            await t.PushAsync(s with { Profile = null, Team = new Team("team-lumen", "Lumen Labs", false, false), ProfileStatus = new ProfileStatus.TeamDisabled() });
            Assert.Equal(("teamOffT", "teamOffD(team=Lumen Labs)"), (main.RestrictedTitle, main.RestrictedMessage));
            Assert.True(main.CanSwitchTeam);
            Assert.False(main.CanBuy);

            await t.PushAsync(s with { Profile = null, ProfileStatus = new ProfileStatus.NoSubscription() });
            Assert.Equal(("noSubT", "noSubD"), (main.RestrictedTitle, main.RestrictedMessage));
            Assert.Equal("—", main.ExpiresText);

            // Invalid with a previous profile: the warning only. Without: the nodes empty state.
            await t.PushAsync(s with { ProfileStatus = new ProfileStatus.Invalid(new(ErrorCode.ProfileInvalid, "x")) });
            Assert.False(main.IsRestricted);
            Assert.True(main.ConfigInvalid);
            Assert.True(main.Nodes.ConfigInvalid);
            Assert.Equal(NodesViewState.Data, main.Nodes.ViewState);
            await t.PushAsync(s with { Profile = null, ProfileStatus = new ProfileStatus.Invalid(new(ErrorCode.ProfileInvalid, "x")) });
            Assert.True(main.InvalidNoHistory);
            Assert.Equal(NodesViewState.InvalidNoHistory, main.Nodes.ViewState);
            await t.PushAsync(s with { Profile = null, ProfileStatus = new ProfileStatus.Loading() });
            Assert.True(main.ProfileLoading);
            Assert.Equal(NodesViewState.Loading, main.Nodes.ViewState);
        });
    }

    [Fact]
    public async Task AccountMenuHeaderAndTeams()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var main = t.Main;
            await Wait.Until(() => main.Teams.Count == 3, "teams");
            Assert.Equal("alice@example.com", main.AccountEmail);
            Assert.Equal("A", main.AvatarInitial);
            Assert.Equal("personal", main.TeamName);
            Assert.Equal("2026年12月31日", main.ExpiresText);
            Assert.Equal("serviceExpiresOn(d=2026年12月31日)", main.AccountSubtitle);

            var personal = main.Teams[0];
            Assert.Equal(("personal", true, true, ""), (personal.Title, personal.IsCurrent, personal.IsSelectable, personal.DisabledTag));
            var lumen = main.Teams[2];
            Assert.Equal(("Lumen Labs", false, false, "disabled"), (lumen.Title, lumen.IsCurrent, lumen.IsSelectable, lumen.DisabledTag));

            // Disabled teams are ignored; a refused switch is a dialog.
            await main.SwitchTeamCommand.ExecuteAsync(lumen);
            Assert.DoesNotContain(t.Backend.Calls, c => c.StartsWith("switch"));
            t.Backend.Fail = new ClientException.Failed(ErrorCode.TeamDisabled, "403012");
            await main.SwitchTeamCommand.ExecuteAsync(main.Teams[1]);
            Assert.Equal(("switchFailT", "switchFailD(reason=Error_TeamDisabled)"), t.Prompts.Errors.Single());
            Assert.Equal(personal.Id, main.SelectedTeam?.Id);
        });
    }

    [Fact]
    public async Task TrayMenuFollowsTheState()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            var main = t.Main;
            // Signed out.
            await t.PushAsync(ClientSnapshots.Initial with { Auth = new AuthState.SignedOut() });
            Assert.Equal(TrayIconState.Off, main.Tray.Icon);
            Assert.Equal(
                [TrayItemRole.SignedOut, TrayItemRole.None, TrayItemRole.OpenMain, TrayItemRole.Settings, TrayItemRole.CheckUpdates, TrayItemRole.None, TrayItemRole.Quit],
                main.Tray.Items.Select(i => i.Role));
            Assert.Equal("notSignedIn", main.Tray.Items[0].Text);
            Assert.False(main.Tray.Items[0].IsEnabled);

            // Signed in and connected, 3 unread.
            await t.SignedInAsync(s => s with { UnreadNotifications = 3 }, Conn(ConnectionPhase.On, endpoint: "hk1-r0", latency: 38));
            var tray = main.Tray;
            Assert.Equal(TrayIconState.On, tray.Icon);
            Assert.Equal(
                [TrayItemRole.Status, TrayItemRole.None, TrayItemRole.Connect, TrayItemRole.CurrentNode, TrayItemRole.None,
                 TrayItemRole.Messages, TrayItemRole.None, TrayItemRole.OpenMain, TrayItemRole.Settings, TrayItemRole.CheckUpdates, TrayItemRole.None, TrayItemRole.Quit],
                tray.Items.Select(i => i.Role));
            var header = tray.Items[0];
            Assert.Equal(("h_on", "香港 01 · 38 ms", ConnectionTone.Ok), (header.Text, header.Secondary, header.Dot));
            var connect = tray.Items[2];
            Assert.Equal(("connect", "st_on", true, true), (connect.Text, connect.Secondary, connect.IsChecked, connect.IsEnabled));
            var nodes = tray.Items[3];
            Assert.Equal("currentNodeIs(n=香港 01)", nodes.Text);
            Assert.Equal(11, nodes.Children!.Count);
            Assert.True(nodes.Children[0].IsChecked);
            var messages = tray.Items[5];
            Assert.Equal(("unreadN(n=3)", true), (messages.Text, messages.BadgeDot));
            Assert.Equal("settingsMenuWin", tray.Items[8].Text);

            // The Connect item disconnects; picking a node from the submenu selects it.
            connect.Command!.Execute(null);
            await Wait.Until(() => t.Backend.Calls.Contains("disconnect"), "disconnect from the tray");
            nodes.Children[2].Command!.Execute(nodes.Children[2].CommandParameter);
            await Wait.Until(() => t.Backend.Calls.Contains("select tw1"), "node selected from the tray");

            // Compatible wording; busy and error icons; unread over 99.
            await t.PushAsync(ScriptedBackend.SignedIn() with { UnreadNotifications = 120 },
                Conn(ConnectionPhase.Connecting, endpoint: "hk1-r0", method: ConnectionMode.Compatible));
            Assert.Equal(TrayIconState.Busy, main.Tray.Icon);
            Assert.Equal("std_starting", main.Tray.Items.Single(i => i.Role == TrayItemRole.Connect).Secondary);
            Assert.Equal("unreadN(n=99+)", main.Tray.Items.Single(i => i.Role == TrayItemRole.Messages).Text);
            await t.PushAsync(ScriptedBackend.SignedIn(), Conn(ConnectionPhase.Contended, new(ErrorCode.ServiceBusy, "x"), canTakeOver: true));
            Assert.Equal(TrayIconState.Error, main.Tray.Icon);
            Assert.Equal("noUnread", main.Tray.Items.Single(i => i.Role == TrayItemRole.Messages).Text);

            // Restricted: the restricted title (disabled) and the messages line.
            await t.PushAsync(ScriptedBackend.SignedIn(s => s with { Profile = null, ProfileStatus = new ProfileStatus.NoSubscription() }), Conn(ConnectionPhase.Off));
            Assert.Equal(TrayIconState.Off, main.Tray.Icon);
            Assert.Equal([TrayItemRole.Restricted, TrayItemRole.None, TrayItemRole.Messages], main.Tray.Items.Take(3).Select(i => i.Role));
            Assert.Equal("noSubT", main.Tray.Items[0].Text);
        });
    }

    [Fact]
    public async Task SurfacesAreRequestedFromTheTrayAndMenus()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            var requests = new List<(AppSurface, ulong?)>();
            var quit = 0;
            t.Main.ShowRequested += (surface, id) => requests.Add((surface, id));
            t.Main.QuitRequested += () => quit++;
            await t.PushAsync(ClientSnapshots.Initial with { Auth = new AuthState.SignedOut() });

            // Settings works signed out (General and Advanced only).
            t.Main.OpenSettingsCommand.Execute(null);
            Assert.False(t.Main.Settings.ShowAccountSection);
            t.Main.OpenMainCommand.Execute(null);
            t.Main.OpenMessagesCommand.Execute(null);
            t.Main.OpenAccountSettingsCommand.Execute(null);
            t.Main.QuitCommand.Execute(null);
            Assert.Equal([(AppSurface.Settings, null), (AppSurface.Main, null), (AppSurface.MessageCenter, null), (AppSurface.Settings, (ulong?)null)], requests);
            Assert.Equal(1, quit);
            await t.SignedInAsync();
            Assert.True(t.Main.Settings.ShowAccountSection);
        });
    }

    [Fact]
    public async Task SettingsChooseTheConnectionMethod()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var settings = t.Main.Settings;
            Assert.Equal(
                [(ConnectionMode.Enhanced, "methodEnhanced", "methodEnhancedD"), (ConnectionMode.Compatible, "methodCompatible", "methodCompatibleD")],
                settings.ConnectionModes.Select(o => (o.Mode, o.Title, o.Description)));
            Assert.Equal(ConnectionMode.Enhanced, settings.SelectedConnectionMode?.Mode);

            settings.SelectedConnectionMode = settings.ConnectionModes[1];
            await Wait.Until(() => t.Backend.Calls.Contains("mode Compatible"), "mode set");
            await t.PushAsync(t.Main.Snapshot with { }, Conn(ConnectionPhase.Off, method: ConnectionMode.Compatible));
            Assert.Equal(ConnectionMode.Compatible, settings.SelectedConnectionMode?.Mode);
            Assert.False(t.Main.ShowInstallHint);
            Assert.Equal("1.4.0", t.Services.AppVersion);
            Assert.Equal("version(v=1.4.0, b=2609)", settings.VersionText);
            Assert.Equal("https://api.example.test", settings.ApiPlaceholder);
        });
    }

    [Fact]
    public async Task SettingsChooseTheRoutingMode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.SignedInAsync();
            var settings = t.Main.Settings;
            Assert.Equal(
                [(RoutingMode.Rules, "routingRules", "routingRulesD"), (RoutingMode.Global, "routingGlobal", "routingGlobalD")],
                settings.RoutingModes.Select(o => (o.Mode, o.Title, o.Description)));
            Assert.Equal(RoutingMode.Rules, settings.SelectedRoutingMode?.Mode);

            // The same mode again: nothing to do.
            await t.Main.SetRoutingModeAsync(RoutingMode.Rules);
            Assert.DoesNotContain("routing Rules", t.Backend.Calls);

            settings.SelectedRoutingMode = settings.RoutingModes[1];
            await Wait.Until(() => t.Backend.Calls.Contains("routing Global"), "routing mode set");
            await t.PushAsync(t.Main.Snapshot with { RoutingMode = RoutingMode.Global });
            Assert.Equal(RoutingMode.Global, t.Main.RoutingMode);
            Assert.Equal(RoutingMode.Global, settings.SelectedRoutingMode?.Mode);
            Assert.DoesNotContain("disconnect", t.Backend.Calls);
        });
    }

    [Fact]
    public async Task NetworkChangesReachTheClientOnceTheySettle()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var time = new SteppedTime(Scripted.Now);
            var t = new Scripted(time: time);
            int Told() => t.Backend.Calls.Count(c => c == "network_changed");

            // A burst (a Wi-Fi switch, our TUN coming up): told once, a second after the last event.
            t.Main.OnNetworkChanged();
            time.Advance(TimeSpan.FromMilliseconds(600));
            t.Main.OnNetworkChanged();
            time.Advance(TimeSpan.FromMilliseconds(600));
            t.Main.OnNetworkChanged();
            time.Advance(TimeSpan.FromMilliseconds(900));
            Assert.Equal(0, Told());
            time.Advance(TimeSpan.FromMilliseconds(100));
            Assert.Equal(1, Told());
            time.Advance(TimeSpan.FromSeconds(5));
            Assert.Equal(1, Told());

            // Events arrive on background threads.
            await Task.Run(t.Main.OnNetworkChanged);
            time.Advance(MainViewModel.NetworkChangeDebounce);
            Assert.Equal(2, Told());

            // Quitting drops a pending change and ignores later ones.
            t.Main.OnNetworkChanged();
            await t.Main.ShutdownAsync();
            t.Main.OnNetworkChanged();
            time.Advance(TimeSpan.FromSeconds(3));
            Assert.Equal(2, Told());
        });
    }
}
