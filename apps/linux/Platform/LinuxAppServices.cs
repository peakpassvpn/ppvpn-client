using System.Diagnostics;
using System.Reflection;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Linux.Platform;

/// <summary>The view models' <see cref="IAppServices"/>; call on the GTK main thread.</summary>
public sealed class LinuxAppServices(SettingsStore settings) : IAppServices
{
    public string AppVersion { get; } =
        typeof(LinuxAppServices).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion
            .Split('+')[0] ?? "0.0.0";

    /// <summary>The release build counter; "dev" for local builds.</summary>
    public string BuildNumber => BuildInfo.Build.Revision > 0 ? BuildInfo.Build.Revision.ToString() : "dev";

    public string DefaultApiBase => BuildInfo.ApiBase;

    public void CopyText(string text) => Gdk.Display.GetDefault()?.GetClipboard().SetText(text);

    public bool OpenUrl(string url) =>
        Uri.TryCreate(url, UriKind.Absolute, out var uri) && uri.Scheme is "https" or "http" && XdgOpen(uri.AbsoluteUri);

    public bool OpenFolder(string path) => Directory.Exists(path) && XdgOpen(path);

    public bool LaunchAtLogin => Autostart.Enabled;

    public void SetLaunchAtLogin(bool enabled) => Autostart.Enabled = enabled;

    // XDG autostart needs no approval; desktops that block it have no settings page to open.
    public bool LaunchAtLoginBlocked => false;

    public void OpenLaunchAtLoginSettings() { }

    public void ApplyAppearance(Appearance appearance) =>
        Adw.StyleManager.GetDefault().SetColorScheme(appearance switch
        {
            Appearance.Light => Adw.ColorScheme.ForceLight,
            Appearance.Dark => Adw.ColorScheme.ForceDark,
            _ => Adw.ColorScheme.Default,
        });

    /// <summary>Set once the update notice exists; the repository does the actual upgrade.</summary>
    public Update.UpdateNotifier? Updates { get; set; }

    public bool CanCheckForUpdates => Updates?.CanCheck == true;

    public void CheckForUpdates() => _ = Updates?.CheckAsync(userInitiated: true);

    public bool AutoCheckUpdates
    {
        get => settings.AutoCheckUpdates;
        set
        {
            settings.AutoCheckUpdates = value;
            settings.Save();
        }
    }

    /// <summary>
    /// Also used by <see cref="LinuxPlatformHooks.OpenUrl"/>, which runs on Rust threads.
    /// xdg-open returns once the handler is launched, and fails (e.g. "no method available")
    /// with a non-zero exit; one still running after two seconds is taken as success (this can
    /// run on the UI thread).
    /// </summary>
    internal static bool XdgOpen(string target)
    {
        try
        {
            using var process = Process.Start(new ProcessStartInfo("xdg-open")
            {
                ArgumentList = { target },
                RedirectStandardOutput = true,
                RedirectStandardError = true,
            });
            if (process is null) return false;
            _ = process.StandardOutput.ReadToEndAsync();
            _ = process.StandardError.ReadToEndAsync();
            return !process.WaitForExit(TimeSpan.FromSeconds(2)) || process.ExitCode == 0;
        }
        catch (Exception)
        {
            return false;
        }
    }
}
