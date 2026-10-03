using System.Globalization;
using System.Security;
using Microsoft.Win32;
using PPVPN.Ffi;

namespace PPVPN.PushAgent;

/// <summary>
/// The crate's <see cref="PushAgentListener"/>: shows each push as a toast of the main app's
/// identity. Must match PPVPN.Windows/Platform/AppNotifications.cs: the AUMID, its registration
/// (the app writes the same values on every start; ppvpn.exe stays the click activator) and the
/// toast's <c>launch</c> arguments, which the app's activator parses (<c>id=&lt;pushId&gt;</c>).
/// </summary>
sealed class PushNotifier(AgentLog log) : PushAgentListener
{
    public const string Aumid = "PeakPass.PPVPN";
    const string DisplayName = "PPVPN";
    const string ActivatorClsid = "FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE";
    const string ToastActivatedSwitch = "--toast-activated";
    const string AppExe = "ppvpn.exe";
    // Windows 11 22H2: toast scenario "urgent" (important notifications), as in the app.
    const int UrgentScenarioBuild = 22546;

    readonly object _gate = new();
    bool? _enabled;

    /// <summary>Set when the crate reports <see cref="PushAgentState.Revoked"/>.</summary>
    public ManualResetEvent Revoked { get; } = new(false);

    public bool OnPush(PushMessage message)
    {
        try
        {
            var setting = Toasts.Setting(Aumid);
            Remember(setting);
            if (setting != Toasts.NotificationSetting.Enabled)
            {
                log.Info($"push {message.Id}: notifications are {setting}; kept pending");
                return false;
            }
            Toasts.Show(Aumid, ToastXml(message), message.Id.ToString(CultureInfo.InvariantCulture), "inbox");
            log.Info($"push {message.Id}: shown ({message.Severity}, {message.Category})");
            return true;
        }
        catch (Exception error)
        {
            log.Warn($"push {message.Id}: not shown: {error.Message}");
            return false;
        }
    }

    public void OnState(PushAgentState state)
    {
        switch (state)
        {
            case PushAgentState.Idle idle:
                log.Info($"state: idle ({idle.Reason}); waiting for push-agent.json");
                break;
            case PushAgentState.Running:
                log.Info("state: running");
                break;
            case PushAgentState.Revoked:
                log.Info("state: revoked");
                Revoked.Set();
                break;
        }
    }

    /// <summary>
    /// Polled by the main thread. True when notifications just became enabled again (the caller
    /// then retries what the crate holds back).
    /// </summary>
    public bool PollSetting()
    {
        try
        {
            return Remember(Toasts.Setting(Aumid));
        }
        catch (Exception error)
        {
            log.Warn($"notification setting unavailable: {error.Message}");
            return false;
        }
    }

    /// <returns>True on a change to Enabled.</returns>
    bool Remember(Toasts.NotificationSetting setting)
    {
        var enabled = setting == Toasts.NotificationSetting.Enabled;
        bool? previous;
        lock (_gate)
        {
            previous = _enabled;
            _enabled = enabled;
        }
        if (previous != enabled) log.Info($"notification setting: {setting}");
        return enabled && previous == false;
    }

    static string ToastXml(PushMessage message)
    {
        var title = message.Title;
        var body = message.Body;
        if (string.IsNullOrWhiteSpace(title))
        {
            title = string.IsNullOrWhiteSpace(body) ? DisplayName : body;
            body = "";
        }
        // Only the push id: the app resolves it through the crate's record of shown pushes
        // and never takes a link from the launch arguments.
        var launch = $"id={message.Id.ToString(CultureInfo.InvariantCulture)}";
        var urgent = message.Severity == MessageSeverity.Critical && Environment.OSVersion.Version.Build >= UrgentScenarioBuild;
        // Messages retried later keep the time they were sent.
        var timestamp = DateTimeOffset.TryParse(message.CreatedAt, CultureInfo.InvariantCulture, DateTimeStyles.AssumeUniversal, out var created)
            ? $" displayTimestamp=\"{created.UtcDateTime.ToString("yyyy-MM-ddTHH:mm:ssZ", CultureInfo.InvariantCulture)}\""
            : "";
        var content = string.IsNullOrWhiteSpace(body) ? "" : $"<text>{SecurityElement.Escape(body)}</text>";
        return $"<toast launch=\"{SecurityElement.Escape(launch)}\"{(urgent ? " scenario=\"urgent\"" : "")}{timestamp}>" +
            $"<visual><binding template=\"ToastGeneric\"><text>{SecurityElement.Escape(title)}</text>{content}</binding></visual>" +
            "</toast>";
    }

    /// <summary>
    /// The same per-user registration the app writes on every start, so toasts show "PPVPN" with its
    /// icon even when the app has not run since an upgrade, and a click starts ppvpn.exe (never
    /// the agent). Only when ppvpn.exe is next to the agent (the install directory).
    /// </summary>
    public void RegisterAumid()
    {
        var dir = AppContext.BaseDirectory;
        var app = Path.Combine(dir, AppExe);
        if (!File.Exists(app))
        {
            log.Warn($"{AppExe} is not in {dir}: leaving the notification registration alone");
            return;
        }
        try
        {
            var changed = false;
            using (var key = Registry.CurrentUser.CreateSubKey($@"Software\Classes\AppUserModelId\{Aumid}"))
            {
                changed |= Ensure(key, "DisplayName", DisplayName);
                // Only an existing file (see AppNotifications.RegisterAumid).
                var icon = Path.Combine(dir, "Assets", "AppLogo.png");
                if (File.Exists(icon)) changed |= Ensure(key, "IconUri", icon);
                changed |= Ensure(key, "CustomActivator", $"{{{ActivatorClsid}}}");
            }
            using (var server = Registry.CurrentUser.CreateSubKey($@"Software\Classes\CLSID\{{{ActivatorClsid}}}\LocalServer32"))
                changed |= Ensure(server, "", $"\"{app}\" {ToastActivatedSwitch}");
            log.Info(changed ? $"notification registration written for {Aumid}" : $"notification registration for {Aumid} is current");
        }
        catch (Exception error)
        {
            log.Warn($"notification registration failed: {error.Message}");
        }
    }

    static bool Ensure(RegistryKey key, string name, string value)
    {
        if (key.GetValue(name) is string current && current == value) return false;
        key.SetValue(name, value, RegistryValueKind.String);
        return true;
    }
}
