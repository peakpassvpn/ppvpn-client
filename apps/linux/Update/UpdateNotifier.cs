using System.Net.Http.Headers;
using System.Net.Http.Json;
using System.Text.Json;
using System.Text.Json.Serialization;
using CommunityToolkit.Mvvm.ComponentModel;
using PPVPN.App.Core.ViewModels;
using PPVPN.Linux.Platform;
using static PPVPN.Linux.UI.L;

namespace PPVPN.Linux.Update;

/// <summary>
/// Tells the user a newer release is in the apt/dnf repository; the system's package manager
/// does the upgrade. Polls the repository's latest.json at launch and every 24 hours and shows
/// the command for this install's package format. Downloads and installs nothing.
/// Use on the GTK main thread.
/// </summary>
public sealed partial class UpdateNotifier : ObservableObject
{
    private static readonly TimeSpan FirstCheckDelay = TimeSpan.FromSeconds(15);
    private static readonly TimeSpan CheckInterval = TimeSpan.FromHours(24);

    private readonly Uri? _feed;
    private readonly Version _build;
    private readonly string? _format;
    private readonly IAppLog _log;
    private readonly HttpClient _http;
    private Version? _latest;
    private Version? _dismissed;

    public UpdateNotifier(IAppLog log, string version)
        : this(BuildInfo.UpdateFeed, BuildInfo.Build, BuildInfo.PackageFormat, log, version)
    {
    }

    internal UpdateNotifier(Uri? feed, Version build, string? format, IAppLog log, string version)
    {
        _feed = feed;
        _build = build;
        _format = format;
        _log = log;
        _http = new HttpClient { Timeout = TimeSpan.FromSeconds(30) };
        _http.DefaultRequestHeaders.UserAgent.Add(new ProductInfoHeaderValue("PPVPN", version));
        _http.DefaultRequestHeaders.UserAgent.Add(new ProductInfoHeaderValue($"(linux; {format ?? "dev"})"));
    }

    /// <summary>Only release packages (a feed, a package format and a build number) check.</summary>
    public bool CanCheck => _feed is not null && _format is not null && _build > new Version(0, 0, 0, 0);

    [ObservableProperty] bool isAvailable;
    /// <summary>
    /// The newer release as shown (banner, tray item, check toast): x.y.z, with the build when only
    /// the build differs from this install ("0.2.90 (3)"), or "PPVPN 0.2.90 is available" would
    /// read as the version already installed.
    /// </summary>
    [ObservableProperty] string availableVersion = "";
    [ObservableProperty] bool isBannerVisible;

    /// <summary>The upgrade command for this install's package format.</summary>
    public string Command => _format == "rpm"
        ? "sudo dnf upgrade ppvpn"
        : "sudo apt update && sudo apt install --only-upgrade ppvpn";

    /// <summary>Result of a check the user started from Settings, for a toast.</summary>
    public event Action<string>? CheckFinished;

    /// <summary>Checks shortly after launch, then every 24 hours.</summary>
    public async void StartSchedule()
    {
        if (!CanCheck) return;
        await Task.Delay(FirstCheckDelay);
        while (true)
        {
            await CheckAsync(userInitiated: false);
            await Task.Delay(CheckInterval);
        }
    }

    public async Task CheckAsync(bool userInitiated)
    {
        if (!CanCheck) return;
        try
        {
            var latest = await _http.GetFromJsonAsync<Latest>(_feed);
            if (latest is null || !Version.TryParse(latest.Build, out var build) || string.IsNullOrWhiteSpace(latest.Version))
                throw new FormatException("latest.json has no version or build");
            if (build <= _build)
            {
                _log.Info($"update: up to date ({_build}, repository {build})");
                if (userInitiated) CheckFinished?.Invoke(T("upToDate"));
                return;
            }
            _log.Info($"update: {latest.Version} ({build}) in the repository");
            AvailableVersion = DisplayVersion(latest.Version.Trim(), build, _build);
            IsAvailable = true;
            // "Later" hides the banner until a newer build appears or the user asks.
            IsBannerVisible = userInitiated || _dismissed is null || build > _dismissed;
            _latest = build;
            if (userInitiated) CheckFinished?.Invoke(T("updAvailT", ("v", AvailableVersion)));
        }
        catch (Exception error) when (error is HttpRequestException or TaskCanceledException or JsonException or FormatException)
        {
            _log.Warn($"update check failed: {error.Message}");
            if (userInitiated) CheckFinished?.Invoke(T("updCheckFailT"));
        }
    }

    public void Later()
    {
        _dismissed = _latest;
        IsBannerVisible = false;
    }

    /// <summary>Brings the banner back (tray item) while an update is available.</summary>
    public void ShowBanner()
    {
        if (IsAvailable) IsBannerVisible = true;
    }

    /// <summary>
    /// <paramref name="version"/>, plus the build number when <paramref name="build"/> has the
    /// same x.y.z as <paramref name="installed"/> (builds are x.y.z.n).
    /// </summary>
    internal static string DisplayVersion(string version, Version build, Version installed) =>
        build.Major == installed.Major && build.Minor == installed.Minor && build.Build == installed.Build
            ? $"{version} ({build.Revision})"
            : version;

    /// <summary>{"version":"0.3.0","build":"0.3.0.12","published_at":"…"}, written by the release workflow.</summary>
    private sealed record Latest(
        [property: JsonPropertyName("version")] string? Version,
        [property: JsonPropertyName("build")] string? Build);
}
