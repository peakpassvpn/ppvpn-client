using System.Globalization;
using System.Reflection;
using System.Text.Json;
using System.Text.Json.Serialization;
using PPVPN.App.Core.ViewModels;
using PPVPN.Windows.Platform;
using Windows.ApplicationModel.DataTransfer;

namespace PPVPN.Windows.Services;

/// <summary>
/// Per-user locations: <c>%LOCALAPPDATA%\PPVPN</c> is the crate's data directory (core state,
/// selected node, and the crate's own client-settings.json) and holds the app's app-settings.json; logs
/// go to its <c>logs</c> folder.
/// </summary>
public sealed record AppPaths(string DataDir, string LogDir, string SettingsFile)
{
    public static AppPaths Create()
    {
        var root = Path.Combine(Environment.GetFolderPath(Environment.SpecialFolder.LocalApplicationData), "PPVPN");
        // Not client-settings.json: that is the crate's file (connection mode) in the same folder.
        var paths = new AppPaths(root, Path.Combine(root, "logs"), Path.Combine(root, "app-settings.json"));
        Directory.CreateDirectory(paths.DataDir);
        Directory.CreateDirectory(paths.LogDir);
        return paths;
    }
}

/// <summary>
/// app-settings.json in the per-user app folder: the App.Core <see cref="ISettingsStore"/> values
/// (appearance, API override, first-close notice, window placements) plus the Windows updater's
/// automatic-check switch (<see cref="IAppServices.AutoCheckUpdates"/>).
/// </summary>
public sealed class JsonSettingsStore : ISettingsStore
{
    static readonly JsonSerializerOptions Options = new()
    {
        WriteIndented = true,
        Converters = { new JsonStringEnumConverter() },
    };

    readonly string _path;
    readonly Dictionary<string, string> _placements = [];

    JsonSettingsStore(string path) => _path = path;

    public Appearance Appearance { get; set; }
    public string? ApiBaseOverride { get; set; }
    public bool AutoCheckUpdates { get; set; } = true;
    public bool BackgroundHintShown { get; set; }

    public string? GetWindowPlacement(string window) => _placements.TryGetValue(window, out var value) ? value : null;

    public void SetWindowPlacement(string window, string? placement)
    {
        if (placement is null) _placements.Remove(window);
        else _placements[window] = placement;
    }

    public static JsonSettingsStore Load(string path)
    {
        var store = new JsonSettingsStore(path);
        try
        {
            // Early dev builds kept these values in settings.json; read it once.
            var legacy = Path.Combine(Path.GetDirectoryName(path)!, "settings.json");
            var source = File.Exists(path) ? path : File.Exists(legacy) && File.ReadAllText(legacy).Contains("\"Appearance\"") ? legacy : null;
            if (source is not null && JsonSerializer.Deserialize<Data>(File.ReadAllText(source), Options) is { } data)
            {
                store.Appearance = data.Appearance;
                store.ApiBaseOverride = data.ApiBaseOverride;
                store.AutoCheckUpdates = data.AutoCheckUpdates ?? true;
                store.BackgroundHintShown = data.BackgroundHintShown ?? false;
                foreach (var (window, placement) in data.WindowPlacements ?? []) store._placements[window] = placement;
            }
        }
        catch (Exception error) when (error is IOException or JsonException or UnauthorizedAccessException) { }
        return store;
    }

    public void Save()
    {
        try
        {
            var data = new Data(Appearance, ApiBaseOverride, AutoCheckUpdates, BackgroundHintShown, new(_placements));
            File.WriteAllText(_path, JsonSerializer.Serialize(data, Options));
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException) { }
    }

    sealed record Data(
        Appearance Appearance,
        string? ApiBaseOverride,
        bool? AutoCheckUpdates = null,
        bool? BackgroundHintShown = null,
        Dictionary<string, string>? WindowPlacements = null);
}

/// <summary>The app's own log, next to the client's (the Logs page shows both).</summary>
public sealed class FileAppLog(string directory) : IAppLog
{
    readonly object _gate = new();

    public void Info(string message) => Write("INFO", message);
    public void Warn(string message) => Write("WARN", message);
    public void Error(string message) => Write("ERROR", message);

    void Write(string level, string message)
    {
        var now = DateTimeOffset.Now;
        var line = $"{now.ToString("yyyy-MM-ddTHH:mm:ss.fffzzz", CultureInfo.InvariantCulture)} {level} ppvpn_windows: {message}{Environment.NewLine}";
        lock (_gate)
        {
            // UTC date in the file name, like the crate's tracing-appender files.
            try { File.AppendAllText(Path.Combine(directory, $"ppvpn-windows.{now.UtcDateTime:yyyy-MM-dd}.log"), line); }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }
    }
}

/// <summary>Values baked into the assembly at build time (see PPVPN.Windows.csproj).</summary>
public static class BuildInfo
{
    /// <summary>PPVPN_API_BASE: what the build set (CI: the channel's backend), else www.</summary>
    public static string ApiBase { get; } = Metadata("PPVPN.ApiBase") is { Length: > 0 } value ? value : "https://www.peakpassvpn.com";

    /// <summary>The four-part file version, e.g. 0.3.0.12 (WinSparkle compares it).</summary>
    public static string BuildVersion { get; } = Metadata("PPVPN.BuildVersion");

    /// <summary>The build number: the last part of <see cref="BuildVersion"/>.</summary>
    public static string BuildNumber => BuildVersion.Split('.') is { Length: 4 } parts ? parts[3] : "0";

    /// <summary>The backend to use: the developer override from Settings, else <see cref="ApiBase"/>.</summary>
    public static string EffectiveApiBase(ISettingsStore settings) =>
        string.IsNullOrWhiteSpace(settings.ApiBaseOverride) ? ApiBase : settings.ApiBaseOverride.Trim();

    static string Metadata(string key) =>
        typeof(BuildInfo).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .FirstOrDefault(attribute => attribute.Key == key)?.Value?.Trim() ?? "";
}

public sealed class WindowsAppServices(StartupOptions options, JsonSettingsStore settings) : IAppServices
{
    /// <summary>Set once the main window exists.</summary>
    public Action<Appearance>? AppearanceHandler { get; set; }

    public string AppVersion { get; } =
        typeof(WindowsAppServices).Assembly.GetCustomAttribute<AssemblyInformationalVersionAttribute>()?.InformationalVersion.Split('+')[0]
        ?? "0.0.0";

    public string BuildNumber => BuildInfo.BuildNumber;

    public string DefaultApiBase => BuildInfo.ApiBase;

    public bool DeveloperMode => options.Developer;

    public void CopyText(string text)
    {
        var package = new DataPackage { RequestedOperation = DataPackageOperation.Copy };
        package.SetText(text);
        Clipboard.SetContent(package);
        Clipboard.Flush();
    }

    public bool OpenUrl(string url) => Shell.OpenUrl(url);

    public bool OpenFolder(string path) => Shell.OpenFolder(path);

    public bool LaunchAtLogin => Platform.LaunchAtLogin.IsEnabled;

    public void SetLaunchAtLogin(bool enabled) => Platform.LaunchAtLogin.Set(enabled);

    public bool LaunchAtLoginBlocked => Platform.LaunchAtLogin.IsBlockedBySystem;

    public void OpenLaunchAtLoginSettings() => Platform.LaunchAtLogin.OpenSystemSettings();

    public void ApplyAppearance(Appearance appearance) => AppearanceHandler?.Invoke(appearance);

    public bool CanCheckForUpdates => AppUpdater.IsConfigured;

    public void CheckForUpdates() => AppUpdater.CheckWithUi();

    /// <summary>WinSparkle's scheduled checks; persisted in app-settings.json and applied at start.</summary>
    public bool AutoCheckUpdates
    {
        get => settings.AutoCheckUpdates;
        set
        {
            settings.AutoCheckUpdates = value;
            settings.Save();
            AppUpdater.SetAutomaticChecks(value);
        }
    }
}
