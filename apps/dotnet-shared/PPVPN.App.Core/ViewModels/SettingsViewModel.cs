using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// Settings: General (appearance, launch at login, updates), Account (signed in only: user,
/// team, expiry, sign out — bound to <see cref="Main"/>) and Advanced (connection method, API
/// override, system service). Reachable while signed out; then only General and Advanced show.
/// </summary>
public sealed partial class SettingsViewModel : ObservableObject
{
    readonly ISettingsStore _settings;
    readonly IAppServices _services;
    readonly ILocalizer _strings;
    bool _loading = true;

    internal SettingsViewModel(MainViewModel main, ISettingsStore settings, IAppServices services, ILocalizer strings)
    {
        Main = main;
        _settings = settings;
        _services = services;
        _strings = strings;
        appearanceIndex = (int)settings.Appearance;
        apiBaseOverride = settings.ApiBaseOverride ?? "";
        launchAtLogin = services.LaunchAtLogin;
        launchAtLoginNeedsApproval = services.LaunchAtLogin && services.LaunchAtLoginBlocked;
        autoCheckUpdates = services.AutoCheckUpdates;
        main.PropertyChanged += (_, e) =>
        {
            if (e.PropertyName == nameof(MainViewModel.IsSignedIn)) OnPropertyChanged(nameof(ShowAccountSection));
            if (e.PropertyName == nameof(MainViewModel.ConnectionMode)) OnPropertyChanged(nameof(SelectedConnectionMode));
            if (e.PropertyName == nameof(MainViewModel.RoutingMode)) OnPropertyChanged(nameof(SelectedRoutingMode));
        };
        _loading = false;
    }

    /// <summary>Account rows bind to Main: AccountEmail, Teams / SelectedTeam, ExpiresText / IsExpired, SignOutCommand.</summary>
    public MainViewModel Main { get; }

    /// <summary>The Account group shows only while signed in.</summary>
    public bool ShowAccountSection => Main.IsSignedIn;

    /// <summary><c>version</c>: "版本 1.4.0 (2609)".</summary>
    public string VersionText => _strings.Format("version", ("v", _services.AppVersion), ("b", _services.BuildNumber));

    /// <summary>Hidden in builds without an update feed.</summary>
    public bool ShowUpdates => _services.CanCheckForUpdates;

    /// <summary>Advanced › <c>connMethod</c>: Enhanced (recommended) / Compatible, each with a one-line description.</summary>
    public IReadOnlyList<ConnectionModeOption> ConnectionModes => Main.ConnectionModes;

    /// <summary>The current mode (two-way). Changing it while connected reconnects in the new mode.</summary>
    public ConnectionModeOption? SelectedConnectionMode
    {
        get => Main.ConnectionModes.FirstOrDefault(o => o.Mode == Main.ConnectionMode);
        set
        {
            if (value is not null && value.Mode != Main.ConnectionMode) _ = Main.SetConnectionModeAsync(value.Mode);
        }
    }

    /// <summary>Radio buttons: choose <paramref name="option"/>.</summary>
    [RelayCommand]
    Task SetConnectionModeAsync(ConnectionModeOption? option) =>
        option is null ? Task.CompletedTask : Main.SetConnectionModeAsync(option.Mode);

    /// <summary><c>routingMode</c>: Rules (default) / Global, each with a one-line description.</summary>
    public IReadOnlyList<RoutingModeOption> RoutingModes => Main.RoutingModes;

    /// <summary>The current routing mode (two-way). Applied at once, without a reconnect.</summary>
    public RoutingModeOption? SelectedRoutingMode
    {
        get => Main.RoutingModes.FirstOrDefault(o => o.Mode == Main.RoutingMode);
        set
        {
            if (value is not null && value.Mode != Main.RoutingMode) _ = Main.SetRoutingModeAsync(value.Mode);
        }
    }

    /// <summary>Radio buttons: choose <paramref name="option"/>.</summary>
    [RelayCommand]
    Task SetRoutingModeAsync(RoutingModeOption? option) =>
        option is null ? Task.CompletedTask : Main.SetRoutingModeAsync(option.Mode);

    /// <summary>About › <c>aboutSource</c>: where the GPL source is published.</summary>
    public const string SourceUrl = "https://github.com/peakpassvpn/ppvpn-client";

    /// <summary>About › <c>aboutViewLicense</c>: the GNU GPL version 3.</summary>
    public const string LicenseUrl = "https://www.gnu.org/licenses/gpl-3.0.html";

    /// <summary>About: the copyright line, the same in every language.</summary>
    public const string Copyright = "© 2026 PeakPass Labs LLC";

    [RelayCommand]
    void OpenSource() => OpenOrCopy(SourceUrl);

    [RelayCommand]
    void OpenLicense() => OpenOrCopy(LicenseUrl);

    /// <summary>Opens a link in the browser, or copies it when no browser opens.</summary>
    void OpenOrCopy(string url)
    {
        if (!_services.OpenUrl(url)) _services.CopyText(url);
    }

    /// <summary>
    /// <c>editRoutingRules</c>: the team's routing rules on the web (they apply to all its devices);
    /// copied instead when no browser opens.
    /// </summary>
    [RelayCommand]
    void OpenRoutingRules() => OpenOrCopy(Main.RoutingRulesUrl);

    /// <summary>The API override placeholder: the build's effective default base.</summary>
    public string ApiPlaceholder => _services.DefaultApiBase;

    [RelayCommand]
    void CheckForUpdates() => _services.CheckForUpdates();

    /// <summary><c>openWinStartup</c>: the system's startup-apps settings.</summary>
    [RelayCommand]
    void OpenLaunchAtLoginSettings() => _services.OpenLaunchAtLoginSettings();

    /// <summary>0 = follow system, 1 = light, 2 = dark.</summary>
    [ObservableProperty] int appearanceIndex;
    [ObservableProperty] bool launchAtLogin;
    /// <summary>On in the app but turned off by the system: the warning sub-row + <see cref="OpenLaunchAtLoginSettingsCommand"/>.</summary>
    [ObservableProperty] bool launchAtLoginNeedsApproval;
    [ObservableProperty] bool autoCheckUpdates;
    [ObservableProperty] string apiBaseOverride;

    /// <summary>Re-read settings the system may change behind the app's back (call when the page is shown).</summary>
    public void Refresh()
    {
        _loading = true;
        LaunchAtLogin = _services.LaunchAtLogin;
        LaunchAtLoginNeedsApproval = _services.LaunchAtLogin && _services.LaunchAtLoginBlocked;
        AutoCheckUpdates = _services.AutoCheckUpdates;
        _loading = false;
    }

    partial void OnAppearanceIndexChanged(int value)
    {
        if (_loading) return;
        _settings.Appearance = (Appearance)Math.Clamp(value, 0, 2);
        _settings.Save();
        _services.ApplyAppearance(_settings.Appearance);
    }

    partial void OnLaunchAtLoginChanged(bool value)
    {
        if (_loading) return;
        try
        {
            _services.SetLaunchAtLogin(value);
            LaunchAtLoginNeedsApproval = value && _services.LaunchAtLoginBlocked;
        }
        catch (Exception error)
        {
            Main.Log.Error($"launch at login: {error.Message}");
            _loading = true;
            LaunchAtLogin = _services.LaunchAtLogin;
            _loading = false;
            _ = Main.Prompts.ShowErrorAsync(_strings.Get("launchFailT"), error.Message);
        }
    }

    partial void OnAutoCheckUpdatesChanged(bool value)
    {
        if (_loading) return;
        _services.AutoCheckUpdates = value;
    }

    partial void OnApiBaseOverrideChanged(string value)
    {
        if (_loading) return;
        var trimmed = value.Trim();
        _settings.ApiBaseOverride = trimmed.Length == 0 ? null : trimmed;
        _settings.Save();
    }
}
