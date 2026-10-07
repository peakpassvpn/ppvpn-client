using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

/// <summary>End-to-end flows over <see cref="FakeClientBackend"/>.</summary>
public sealed class SignInTests
{
    [Fact]
    public async Task SignInFlowGoesThroughAwaitingToSignedIn()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // Gates hold Restoring and Awaiting until the test has looked; a manual clock keeps the countdown at 10:00.
            var restore = new TaskCompletionSource();
            var approve = new TaskCompletionSource();
            using var h = new Harness(new FakeOptions { IgnoreSavedCredential = true, RestoreGate = restore.Task, ApproveGate = approve.Task },
                time: new ManualTime(Scripted.Now));
            var main = h.Main;
            Assert.Equal(AuthStage.Restoring, main.Stage);
            // Restoring is not signed out: no "not signed in" anywhere yet.
            Assert.Equal("", main.StatusLine);
            Assert.DoesNotContain(main.Tray.Items, i => i.Role == TrayItemRole.SignedOut);
            Assert.Equal("PPVPN", main.Tray.ToolTip);
            restore.SetResult();
            await Wait.Until(() => main.Stage == AuthStage.SignedOut, "signed out after restore");
            Assert.True(main.ShowLogin);
            Assert.False(main.HasLoginError);
            Assert.Equal("notSignedIn", main.StatusLine);

            await main.SignInCommand.ExecuteAsync(null);
            await Wait.Until(() => main.Stage == AuthStage.Awaiting, "awaiting browser");
            Assert.Matches("^[A-Z]{4}-[A-Z]{4}$", main.UserCode);
            Assert.Equal("expiresIn(t=10:00)", main.CountdownText);
            main.CopyCodeCommand.Execute(null);
            Assert.Equal(main.UserCode, Assert.Single(h.Services.Copied));

            approve.SetResult(); // the user confirms in the browser
            await Wait.Until(() => main.Stage == AuthStage.SignedIn, "signed in");
            await h.ReadyAsync();
            Assert.Equal("alice@example.com", main.AccountEmail);
            Assert.Equal(11, main.Nodes.Items.Count);
            Assert.Equal("h_idle", main.StatusLine);
            Assert.Equal(0, main.Listener.Stats.Misdelivered);
        });
    }

    [Theory]
    [InlineData(FakeLoginOutcome.Deny, LoginErrorKind.Denied, "errDeniedT", "errDeniedD", "signInAgain")]
    [InlineData(FakeLoginOutcome.Expire, LoginErrorKind.Expired, "errExpiredT", "errExpiredD", "signInAgain")]
    public async Task FailedSignInShowsTheKindOnTheLoginPage(FakeLoginOutcome outcome, LoginErrorKind kind, string title, string message, string retry)
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { IgnoreSavedCredential = true, LoginOutcome = outcome });
            var main = h.Main;
            await Wait.Until(() => main.Stage == AuthStage.SignedOut, "signed out");
            await main.SignInCommand.ExecuteAsync(null);
            await Wait.Until(() => main.HasLoginError, "login error");
            Assert.Equal((kind, title, message, retry), (main.LoginErrorKind, main.LoginErrorTitle, main.LoginErrorMessage, main.LoginRetryText));
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task NetworkErrorOffersTryAgain()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.PushAsync(ClientSnapshots.Initial with { Auth = new AuthState.SignedOut(), LastError = new(ErrorCode.NetworkUnreachable, "dns") });
            Assert.Equal((LoginErrorKind.Network, "errNetT", "errNetD", "tryAgain"),
                (t.Main.LoginErrorKind, t.Main.LoginErrorTitle, t.Main.LoginErrorMessage, t.Main.LoginRetryText));
        });
    }

    [Fact]
    public async Task LockedStoreSaysUnlockAndRetriesInsteadOfSigningIn()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.PushAsync(ClientSnapshots.Initial with
            {
                Auth = new AuthState.SignedOut(),
                LastError = new(ErrorCode.CredentialStoreLocked, "load: LINUX_KEYRING_LOCKED"),
            });
            Assert.Equal((LoginErrorKind.StoreLocked, "errLockedT", "Error_CredentialStoreLocked", "tryAgain"),
                (t.Main.LoginErrorKind, t.Main.LoginErrorTitle, t.Main.LoginErrorMessage, t.Main.LoginRetryText));

            await t.Main.SignInCommand.ExecuteAsync(null);
            Assert.Equal(["retry_credential_restore"], t.Backend.Calls);
            Assert.Equal(LoginErrorKind.StoreLocked, t.Main.LoginErrorKind);
        });
    }

    [Fact]
    public async Task LockedStoreWithoutAPendingRestoreSignsIn()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            await t.PushAsync(ClientSnapshots.Initial with
            {
                Auth = new AuthState.SignedOut(),
                LastError = new(ErrorCode.CredentialStoreLocked, "save: LINUX_KEYRING_LOCKED"),
            });
            t.Backend.CredentialRetryPending = false;
            await t.Main.SignInCommand.ExecuteAsync(null);
            Assert.Equal(["retry_credential_restore", "auth_start"], t.Backend.Calls);
        });
    }

    [Fact]
    public async Task UnlockingTheStoreRestoresTheSavedLogin()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // The background re-read comes every 15 s here; the test only passes through Retry.
            using var h = new Harness(new FakeOptions { TimeScale = 0.5 }, hooks: MemoryHooks.SignedIn(hooks => hooks.Locked = true));
            var main = h.Main;
            await Wait.Until(() => main.LoginErrorKind == LoginErrorKind.StoreLocked, "locked store shown");
            Assert.Equal(AuthStage.SignedOut, main.Stage);
            Assert.Equal("tryAgain", main.LoginRetryText);
            Assert.NotNull(h.Hooks.Credential); // the saved login is kept

            h.Hooks.Locked = false; // the user unlocks the keyring
            await main.SignInCommand.ExecuteAsync(null);
            await Wait.Until(() => main.Stage == AuthStage.SignedIn, "restored after retry");
            Assert.Equal(2, h.Hooks.Loads);
            Assert.Empty(h.Hooks.OpenedUrls); // no new device login
            Assert.False(main.HasLoginError);
        });
    }

    [Fact]
    public async Task TheLocalCountdownExpiresTheCode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var time = new ManualTime(new DateTimeOffset(2026, 9, 29, 14, 30, 0, TimeSpan.FromHours(8)));
            using var h = new Harness(new FakeOptions { IgnoreSavedCredential = true, LoginOutcome = FakeLoginOutcome.Never }, time: time);
            var main = h.Main;
            await Wait.Until(() => main.Stage == AuthStage.SignedOut, "signed out");
            await main.SignInCommand.ExecuteAsync(null);
            await Wait.Until(() => main.Stage == AuthStage.Awaiting, "awaiting");
            time.Advance(TimeSpan.FromSeconds(6));
            main.Tick();
            Assert.Equal("expiresIn(t=9:54)", main.CountdownText);

            time.Advance(TimeSpan.FromMinutes(10));
            main.Tick();
            await Wait.Until(() => main.Stage == AuthStage.SignedOut, "cancelled at 0");
            Assert.Equal(LoginErrorKind.Expired, main.LoginErrorKind);
            Assert.Equal("errExpiredT", main.LoginErrorTitle);

            // Signing in again clears it.
            await main.SignInCommand.ExecuteAsync(null);
            await Wait.Until(() => main.Stage == AuthStage.Awaiting, "awaiting again");
            Assert.False(main.HasLoginError);
        });
    }

    [Fact]
    public async Task SignOutAsksFirst()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await h.ReadyAsync();
            h.Prompts.Answers.Enqueue(false);
            await h.Main.SignOutCommand.ExecuteAsync(null);
            Assert.Equal([PromptKind.SignOut], h.Prompts.Asked);
            Assert.True(h.Main.IsSignedIn);

            await h.Main.SignOutCommand.ExecuteAsync(null);
            await Wait.Until(() => h.Main.Stage == AuthStage.SignedOut, "signed out");
            Assert.Null(h.Hooks.Credential);
            Assert.Empty(h.Main.Teams);
            Assert.Empty(h.Main.Nodes.Items);
        });
    }
}

public sealed class InstallFlowTests
{
    [Fact]
    public async Task CancellingTheExplanationLeavesTheSwitchOff()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // Configured before the harness starts restoring the session.
            using var h = new Harness(hooks: MemoryHooks.SignedIn(hooks => hooks.ServiceInstalled = false));
            await h.ReadyAsync();
            var main = h.Main;
            Assert.True(main.ShowInstallHint);
            Assert.Equal("serviceOff", main.ServiceStatusText);

            h.Prompts.Answers.Enqueue(false);
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal([PromptKind.InstallService], h.Prompts.Asked);
            Assert.Equal(0, h.Hooks.InstallPrompts);
            Assert.Equal(ConnectState.Off, main.ConnectState);
            Assert.Equal(SwitchVisual.Off, main.ConnectSwitch);
            Assert.Empty(main.Notices);
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task AServiceInstalledOutsideTheAppConnectsWithoutAsking()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn(hooks => hooks.ServiceInstalled = false));
            await h.ReadyAsync();
            var main = h.Main;
            Assert.True(main.ShowInstallHint);

            // An admin installs the service (e.g. dpkg) while the app runs.
            h.Hooks.ServiceInstalled = true;
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Empty(h.Prompts.Asked);
            Assert.Equal(0, h.Hooks.InstallPrompts);
            await Wait.Until(() => main.ConnectState == ConnectState.On, "enhanced on");
            Assert.False(main.ShowInstallHint);
        });
    }

    [Fact]
    public async Task DenyingTheOsPromptFailsWithRetry()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // Configured before the harness starts restoring the session.
            using var h = new Harness(hooks: MemoryHooks.SignedIn(hooks => { hooks.ServiceInstalled = false; hooks.Install = PromptOutcome.Cancel; }));
            await h.ReadyAsync();
            var main = h.Main;

            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(1, h.Hooks.InstallPrompts);
            Assert.Equal(ConnectState.Failed, main.ConnectState);
            var notice = Assert.Single(main.Notices);
            Assert.Equal((NoticeKind.Failed, "fr_auth", "retry"), (notice.Kind, notice.Message, notice.ActionText));
            Assert.Empty(h.Prompts.Errors); // shown by the notice, not a dialog
            Assert.True(main.ShowInstallHint);

            // Retry asks again, and this time the OS prompt is allowed: installed → connecting → on.
            h.Hooks.Install = PromptOutcome.Allow;
            await notice.Action!.ExecuteAsync(null);
            Assert.Equal([PromptKind.InstallService, PromptKind.InstallService], h.Prompts.Asked);
            await Wait.Until(() => main.ConnectState == ConnectState.On, "enhanced on");
            Assert.True(main.ServiceInstalled);
            Assert.False(main.ShowInstallHint);
            Assert.Equal("h_on", main.ConnectionTitle);
            Assert.Equal("d_on(r=HKG-A, ms=38 ms)", main.ConnectionDetail);
            Assert.Empty(main.Notices);
        });
    }

    [Fact]
    public async Task StandaloneInstallAndUninstall()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // Configured before the harness starts restoring the session.
            using var h = new Harness(hooks: MemoryHooks.SignedIn(hooks => hooks.ServiceInstalled = false));
            await h.ReadyAsync();
            var main = h.Main;

            // A dismissed OS prompt from the hint changes nothing: no failure, no dialog.
            h.Hooks.Install = PromptOutcome.Cancel;
            await main.InstallServiceCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Off, main.ConnectState);
            Assert.Empty(main.Notices);
            Assert.Empty(h.Prompts.Errors);
            Assert.False(main.ServiceInstalled);

            // A failing installer is a dialog.
            h.Hooks.Install = PromptOutcome.Fail;
            await main.InstallServiceCommand.ExecuteAsync(null);
            Assert.Equal(("installFailT", "Error_ServiceInstallFailed"), h.Prompts.Errors.Single());

            h.Hooks.Install = PromptOutcome.Allow;
            await main.InstallServiceCommand.ExecuteAsync(null);
            await Wait.Until(() => main.ServiceInstalled, "installed");
            Assert.Equal(ConnectState.Off, main.ConnectState); // install only
            Assert.Equal("serviceOn", main.ServiceStatusText);

            // Uninstall: confirm, cancelled OS prompt is silent, then allowed.
            h.Prompts.Answers.Enqueue(false);
            await main.UninstallServiceCommand.ExecuteAsync(null);
            Assert.True(main.ServiceInstalled);
            h.Hooks.Uninstall = PromptOutcome.Cancel;
            await main.UninstallServiceCommand.ExecuteAsync(null);
            Assert.Single(h.Prompts.Errors);
            h.Hooks.Uninstall = PromptOutcome.Allow;
            await main.UninstallServiceCommand.ExecuteAsync(null);
            await Wait.Until(() => !main.ServiceInstalled, "uninstalled");
            Assert.Contains(PromptKind.UninstallService, h.Prompts.Asked);
        });
    }
}

public sealed class ConnectionFlowTests
{
    [Fact]
    public async Task CompatibleModeFailsThenRetries()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { Method = ConnectionMode.Compatible, FailFirstSystemProxy = true }, MemoryHooks.SignedIn());
            await h.ReadyAsync();
            var main = h.Main;
            Assert.Equal(ConnectionMode.Compatible, main.ConnectionMode);

            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Failed, main.ConnectState);
            Assert.Equal(("h_stdFail", "fr_port"), (main.ConnectionTitle, main.ConnectionDetail));
            var notice = Assert.Single(main.Notices);
            Assert.Equal(("stdMode · std_failed", null), (notice.Title, notice.SecondaryActionText));
            Assert.Empty(h.Prompts.Errors);

            await notice.Action!.ExecuteAsync(null);
            Assert.Equal(ConnectState.On, main.ConnectState);
            Assert.Equal(("h_on", "d_stdOn(r=HKG-A, ms=38 ms)"), (main.ConnectionTitle, main.ConnectionDetail));
            Assert.Equal(TrayIconState.On, main.Tray.Icon);
            Assert.Empty(main.Notices);

            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Off, main.ConnectState);
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task DeniedInstallOffersCompatibilityMode()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            // Configured before the harness starts restoring the session.
            using var h = new Harness(hooks: MemoryHooks.SignedIn(hooks => { hooks.ServiceInstalled = false; hooks.Install = PromptOutcome.Cancel; }));
            await h.ReadyAsync();
            var main = h.Main;

            await main.ToggleConnectCommand.ExecuteAsync(null);
            var notice = Assert.Single(main.Notices);
            Assert.Equal(("fr_auth", "useCompatible"), (notice.Message, notice.SecondaryActionText));

            await notice.SecondaryAction!.ExecuteAsync(null);
            Assert.Equal(ConnectionMode.Compatible, main.ConnectionMode);
            Assert.Equal(ConnectState.On, main.ConnectState);
            Assert.Equal("h_on", main.ConnectionTitle);
            Assert.Empty(main.Notices);
            Assert.Equal(1, h.Hooks.InstallPrompts); // compatible needs no install
        });
    }

    [Fact]
    public async Task SwitchingTheMethodWhileConnectedReconnects()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(hooks: MemoryHooks.SignedIn());
            await h.ReadyAsync();
            var main = h.Main;
            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal((ConnectState.On, "d_on(r=HKG-A, ms=38 ms)"), (main.ConnectState, main.ConnectionDetail));

            main.Settings.SelectedConnectionMode = main.ConnectionModes[1];
            await Wait.Until(() => main.ConnectionMode == ConnectionMode.Compatible && main.ConnectState == ConnectState.On, "reconnected compatible");
            Assert.Equal("d_stdOn(r=HKG-A, ms=38 ms)", main.ConnectionDetail);
            Assert.Equal("captionCompatible", main.ConnectionMethodCaption);
        });
    }

    [Fact]
    public async Task OccupiedThenTakenOver()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { OccupiedOnFirstConnect = true }, MemoryHooks.SignedIn());
            await h.ReadyAsync();
            var main = h.Main;

            await main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Occupied, main.ConnectState);
            Assert.Equal(TrayIconState.Error, main.Tray.Icon);
            var notice = Assert.Single(main.Notices);
            Assert.Equal("takeOver", notice.ActionText);

            await notice.Action!.ExecuteAsync(null);
            Assert.Equal(ConnectState.On, main.ConnectState);
            Assert.Empty(main.Notices);

            // Switching nodes while on reconnects through the new node.
            var details = new List<string>();
            main.PropertyChanged += (_, e) => { if (e.PropertyName == nameof(MainViewModel.ConnectionDetail)) details.Add(main.ConnectionDetail); };
            await main.SelectNodeAsync("jp1");
            await Wait.Until(() => main.ConnectState == ConnectState.On, "reconnected");
            Assert.Contains("d_reconnecting(r0=HKG-A, r=NRT-A)", details);
            Assert.Equal("d_on(r=NRT-A, ms=62 ms)", main.ConnectionDetail);
            Assert.Equal("东京 01", main.CurrentNodeName);
            Assert.Equal("u8f2k-jp1", main.CurrentNodeProxy!.Username);

            // Tapping while on stops it.
            await main.ToggleConnectCommand.ExecuteAsync(null);
            await Wait.Until(() => main.ConnectState == ConnectState.Off, "off");
        });
    }

    [Fact]
    public async Task FailedConnectShowsTimeoutReason()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { FailFirstConnect = true }, MemoryHooks.SignedIn());
            await h.ReadyAsync();
            await h.Main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Failed, h.Main.ConnectState);
            Assert.Equal("fr_timeout", Assert.Single(h.Main.Notices).Message);
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task ConflictOnConnectNamesTheOtherApp()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { ConflictOnFirstConnect = true }, MemoryHooks.SignedIn());
            await h.ReadyAsync();
            await h.Main.ToggleConnectCommand.ExecuteAsync(null);
            Assert.Equal(ConnectState.Failed, h.Main.ConnectState);
            Assert.Equal("h_pathContended", h.Main.ConnectionTitle);
            var notice = Assert.Single(h.Main.Notices);
            Assert.Equal((NoticeKind.Conflict, ConnectionTone.Error, "conflictT", "conflictD(app=Surge)", "retry", "useCompatible"),
                (notice.Kind, notice.Tone, notice.Title, notice.Message, notice.ActionText, notice.SecondaryActionText));
            Assert.Empty(h.Prompts.Errors);

            // Retry succeeds (Surge was turned off): the notice goes away.
            await notice.Action!.ExecuteAsync(null);
            await Wait.Until(() => h.Main.ConnectState == ConnectState.On, "connected");
            Assert.Empty(h.Main.Notices);
            Assert.Null(h.Main.Snapshot.Connection.Competitor);
        });
    }

    [Fact]
    public async Task RoutingRulesNoticeClearsOnceTheRuleSetsLoad()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { UnavailableRuleSets = ["cn-site"], RuleSetsLoadAfter = TimeSpan.FromSeconds(10) },
                MemoryHooks.SignedIn());
            await h.ReadyAsync();
            await Wait.Until(() => h.Main.Notices.Any(n => n.Kind == NoticeKind.RulesUnavailable), "rules notice");
            Assert.Equal(["cn-site"], h.Main.Snapshot.RuleSetsUnavailable);
            await Wait.Until(() => h.Main.Notices.Count == 0, "rules loaded");
            Assert.Empty(h.Main.Snapshot.RuleSetsUnavailable);
            Assert.Empty(h.Prompts.Errors);
        });
    }

    [Fact]
    public async Task BackgroundFailuresAreLoggedAndUserActionsGetADialog()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { FailBackgroundRefresh = true }, MemoryHooks.SignedIn());
            await h.ReadyAsync();
            await Wait.Until(() => h.Main.Snapshot.LastError is not null, "background failure");
            Assert.Empty(h.Prompts.Errors);
            Assert.Contains(h.Log.Lines, l => l.Contains("NetworkUnreachable"));

            // A user action that fails: a one-off dialog.
            await h.Main.SelectNodeAsync("nope");
            Assert.Equal(("errorT", "Error_NodeNotFound"), h.Prompts.Errors.Single());
        });
    }

    [Fact]
    public async Task RestrictedRefreshShowsProgressAndNoDialog()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { PersonalTeam = FakeProfileScenario.NoSubscription }, MemoryHooks.SignedIn());
            var main = h.Main;
            await Wait.Until(() => main.Access == AccessState.NoSubscription, "no subscription");
            var refresh = main.RefreshAccessCommand.ExecuteAsync(null);
            Assert.True(main.IsRefreshingAccess);
            await refresh;
            Assert.False(main.IsRefreshingAccess);
            Assert.Empty(h.Prompts.Errors);
            main.OpenPurchaseCommand.Execute(null);
            Assert.Equal("https://api.example.test/dashboard/products", h.Services.Opened.Single());
            // Settings › routing mode: the team's rules on the web.
            main.Settings.OpenRoutingRulesCommand.Execute(null);
            Assert.Equal("https://api.example.test/dashboard/routing-rules", h.Services.Opened.Last());
        });
    }

    [Fact]
    public async Task AboutLinksOpenTheSourceAndTheLicense()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            var t = new Scripted();
            t.Main.Settings.OpenSourceCommand.Execute(null);
            t.Main.Settings.OpenLicenseCommand.Execute(null);
            Assert.Equal(["https://github.com/peakpassvpn/ppvpn-client", "https://www.gnu.org/licenses/gpl-3.0.html"], t.Services.Opened);
            Assert.Empty(t.Services.Copied);

            // Without a browser the link is copied instead.
            t.Services.OpenFails = true;
            t.Main.Settings.OpenSourceCommand.Execute(null);
            Assert.Equal(["https://github.com/peakpassvpn/ppvpn-client"], t.Services.Copied);
            Assert.Equal("© 2026 PeakPass Labs LLC", SettingsViewModel.Copyright);
            await Task.CompletedTask;
        });
    }

    [Fact]
    public async Task SwitchingTeamsFromTheDisabledState()
    {
        using var ui = new UiContext();
        await ui.RunAsync(async () =>
        {
            using var h = new Harness(new FakeOptions { PersonalTeam = FakeProfileScenario.TeamDisabled }, MemoryHooks.SignedIn());
            var main = h.Main;
            await Wait.Until(() => main.Access == AccessState.TeamDisabled && main.Teams.Count == 3, "team disabled");
            Assert.True(main.CanSwitchTeam);
            await main.SwitchTeamCommand.ExecuteAsync(main.Teams.Single(t => t.Id == "team-acme"));
            await Wait.Until(() => main.Access == AccessState.Ok && main.Nodes.Items.Count == 11, "switched");
            Assert.Equal("Acme Studio", main.TeamName);
            Assert.True(main.Teams.Single(t => t.Id == "team-acme").IsCurrent);
        });
    }
}
