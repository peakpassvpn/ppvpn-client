using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;
using PPVPN.Windows.Platform;

namespace PPVPN.Windows.Services;

/// <summary>
/// Command-line switches. Besides <c>--background</c> (autostart) and <c>--dev</c>,
/// <c>--fake-profile[=&lt;scenario&gt;]</c> replaces the ppvpn-client crate with the in-process
/// <see cref="FakeClientBackend"/>; the other <c>--fake-*</c> switches tune it, and the
/// <c>--demo-*</c> switches press buttons, so every screen can be reached without clicking
/// (used for the VM screenshots; see README.md).
/// </summary>
public sealed record StartupOptions
{
    public bool Background { get; init; }
    public bool Developer { get; init; }
    public string? Page { get; init; }
    public Appearance? Theme { get; init; }
    /// <summary>Use <see cref="FakeClientBackend"/> instead of the crate.</summary>
    public bool UseFake { get; init; }
    public FakeOptions Fake { get; init; } = new();
    /// <summary>Fake only: <c>--fake-service=installed|approve|deny</c> simulates the privileged service.</summary>
    public FakeService FakeService { get; init; }
    /// <summary>
    /// Fake only: <c>--fake-push-agent</c> runs the push agent wiring (<see cref="PushAgentAutostart"/>)
    /// off the real <c>push-agent.json</c>, which the fake never writes (write it by hand).
    /// </summary>
    public bool FakePushAgent { get; init; }
    public bool DemoSignIn { get; init; }
    public bool DemoConnect { get; init; }
    public bool DemoProbe { get; init; }
    public bool DemoTeams { get; init; }
    /// <summary>Open the message center (and with <see cref="DemoDetail"/> the newest message).</summary>
    public bool DemoMessages { get; init; }
    public bool DemoDetail { get; init; }
    /// <summary>Show a dialog: install, uninstall, signout, error.</summary>
    public string? DemoDialog { get; init; }
    /// <summary>Open the tray menu (for screenshots; it stays open until dismissed).</summary>
    public bool DemoTray { get; init; }
    /// <summary>Press the tray's Quit after this many seconds (smoke tests).</summary>
    public double? DemoQuitAfter { get; init; }
    /// <summary>Set the connection method (enhanced / compatible) before <c>--demo-connect</c>.</summary>
    public string? DemoMode { get; init; }
    /// <summary>Copy the current node's HTTP proxy URL (with credentials) once connected.</summary>
    public bool DemoCopyProxy { get; init; }
    /// <summary>Switch to the other connection method this many seconds after connecting.</summary>
    public double? DemoSwitchAfter { get; init; }
    /// <summary>Turn the connect switch off this many seconds after connecting (after any switch).</summary>
    public double? DemoDisconnectAfter { get; init; }
    /// <summary>Switches that were not understood (logged at startup).</summary>
    public IReadOnlyList<string> Unknown { get; init; } = [];

    public static StartupOptions Parse(IEnumerable<string> args)
    {
        var options = new StartupOptions();
        var fake = new FakeOptions();
        var unknown = new List<string>();
        foreach (var raw in args)
        {
            var (name, value) = raw.Split('=', 2) is [var n, var v] ? (n, v) : (raw, "");
            switch (name)
            {
                case LaunchAtLogin.BackgroundArgument: options = options with { Background = true }; break;
                case "--dev": options = options with { Developer = true }; break;
                case "--page": options = options with { Page = value }; break;
                case "--theme":
                    options = options with { Theme = value switch { "light" => Appearance.Light, "dark" => Appearance.Dark, _ => Appearance.System } };
                    break;
                case "--demo-signin": options = options with { DemoSignIn = true }; break;
                case "--demo-connect" or "--demo-enhanced": options = options with { DemoConnect = true }; break;
                case "--demo-probe": options = options with { DemoProbe = true }; break;
                case "--demo-teams": options = options with { DemoTeams = true }; break;
                case "--demo-messages": options = options with { DemoMessages = true }; break;
                case "--demo-detail": options = options with { DemoMessages = true, DemoDetail = true }; break;
                case "--demo-tray": options = options with { DemoTray = true }; break;
                case "--demo-mode": options = options with { DemoMode = value }; break;
                case "--demo-copy-proxy": options = options with { DemoCopyProxy = true }; break;
                case "--demo-switch-after" when double.TryParse(value, out var switchAfter): options = options with { DemoSwitchAfter = switchAfter }; break;
                case "--demo-disconnect-after" when double.TryParse(value, out var offAfter): options = options with { DemoDisconnectAfter = offAfter }; break;
                case "--demo-quit" when double.TryParse(value, out var quitAfter): options = options with { DemoQuitAfter = quitAfter }; break;
                case "--demo-dialog": options = options with { DemoDialog = value.Length > 0 ? value : "error" }; break;
                case "--fake-profile" when ParseScenario(value) is { } scenario:
                    options = options with { UseFake = true };
                    fake = fake with { PersonalTeam = scenario };
                    break;
                case "--fake-signed-out": fake = fake with { IgnoreSavedCredential = true }; break;
                case "--fake-browser":
                    fake = fake with
                    {
                        Browser = value switch { "pretend" => FakeBrowserMode.Pretend, "fail" => FakeBrowserMode.Fail, _ => FakeBrowserMode.Open },
                    };
                    break;
                case "--fake-login":
                    fake = fake with
                    {
                        LoginOutcome = value switch
                        {
                            "deny" => FakeLoginOutcome.Deny,
                            "expire" => FakeLoginOutcome.Expire,
                            "never" => FakeLoginOutcome.Never,
                            _ => FakeLoginOutcome.Approve,
                        },
                    };
                    break;
                case "--fake-approve-after" when double.TryParse(value, out var seconds):
                    fake = fake with { ApproveAfter = TimeSpan.FromSeconds(seconds) };
                    break;
                case "--fake-connect-fail": fake = fake with { FailFirstConnect = true }; break;
                case "--fake-service":
                    options = options with
                    {
                        FakeService = value switch { "approve" => FakeService.Approve, "deny" => FakeService.Deny, _ => FakeService.Installed },
                    };
                    break;
                case "--fake-push-agent": options = options with { FakePushAgent = true }; break;
                case "--fake-occupied": fake = fake with { OccupiedOnFirstConnect = true }; break;
                case "--fake-proxy-fail": fake = fake with { FailFirstSystemProxy = true }; break;
                case "--fake-method":
                    fake = fake with { Method = value == "compatible" ? ConnectionMode.Compatible : ConnectionMode.Enhanced };
                    break;
                case "--fake-messages" when int.TryParse(value, out var count): fake = fake with { MessageCount = count }; break;
                case "--fake-unread" when int.TryParse(value, out var unread): fake = fake with { UnreadMessages = unread }; break;
                case "--fake-refresh-fail": fake = fake with { FailBackgroundRefresh = true }; break;
                // A launch for a notification click (see AppNotifications); COM adds -Embedding.
                case AppNotifications.ToastActivatedSwitch or "-Embedding" or "/Embedding": break;
                // The name is split at "=", so match the whole argument.
                case not null when raw.StartsWith(AppNotifications.ForwardSwitch, StringComparison.Ordinal): break;
                default: unknown.Add(raw); break;
            }
        }
        return options with { Fake = fake, Unknown = unknown };
    }

    /// <summary>
    /// <c>--fake-profile</c> scenarios for the personal team (the one selected at sign-in):
    /// active (default), no-subscription, expired, team-disabled, invalid. The
    /// <see cref="FakeProfileScenario"/> names work too.
    /// </summary>
    static FakeProfileScenario? ParseScenario(string value) =>
        value.Replace("-", "").Replace("_", "").ToLowerInvariant() switch
        {
            "" or "active" or "ready" => FakeProfileScenario.Active,
            "nosubscription" => FakeProfileScenario.NoSubscription,
            "expired" or "subscriptionexpired" => FakeProfileScenario.SubscriptionExpired,
            "teamdisabled" or "disabled" => FakeProfileScenario.TeamDisabled,
            "invalid" => FakeProfileScenario.Invalid,
            _ => null,
        };
}
