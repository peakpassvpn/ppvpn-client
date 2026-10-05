// Platform seams for the shared view models. Nothing in PPVPN.App.Core may
// reference a UI toolkit (WinUI, GTK); each app implements these (Windows in
// Platform/ and Services/, Linux with Gir.Core / GLib equivalents).

namespace PPVPN.App.Core.ViewModels;

public enum Appearance { System, Light, Dark }

/// <summary>Things only the host OS/UI toolkit can do.</summary>
public interface IAppServices
{
    /// <summary>Marketing version, e.g. "1.4.0".</summary>
    string AppVersion { get; }
    /// <summary>Build number, e.g. "2609" (shown as "Version 1.4.0 (2609)").</summary>
    string BuildNumber { get; }
    /// <summary>The API base this build uses when there is no override (the Advanced placeholder).</summary>
    string DefaultApiBase { get; }
    void CopyText(string text);
    bool OpenUrl(string url);
    bool OpenFolder(string path);
    bool LaunchAtLogin { get; }
    /// <exception cref="Exception">The setting could not be changed.</exception>
    void SetLaunchAtLogin(bool enabled);
    /// <summary>
    /// Launch at login is on in the app but the system turned it off (Windows: Settings › Apps ›
    /// Startup). Settings then shows a hint and <see cref="OpenLaunchAtLoginSettings"/>.
    /// </summary>
    bool LaunchAtLoginBlocked { get; }
    void OpenLaunchAtLoginSettings();
    void ApplyAppearance(Appearance appearance);
    /// <summary>False when this build has no update feed; the UI hides the update controls.</summary>
    bool CanCheckForUpdates { get; }
    /// <summary>Starts a user-initiated update check with the updater's own UI.</summary>
    void CheckForUpdates();
    /// <summary>The updater checks automatically (WinSparkle / the Linux updater setting).</summary>
    bool AutoCheckUpdates { get; set; }
}

/// <summary>Persisted app preferences (not client state; that lives in the crate).</summary>
public interface ISettingsStore
{
    Appearance Appearance { get; set; }
    /// <summary>Backend base URL override; null or empty means the default.</summary>
    string? ApiBaseOverride { get; set; }
    /// <summary>The first-close "still running" notice was shown (Windows).</summary>
    bool BackgroundHintShown { get; set; }
    /// <summary>Opaque window placement by window name ("main", "messages"); null when unknown.</summary>
    string? GetWindowPlacement(string window);
    void SetWindowPlacement(string window, string? placement);
    void Save();
}

/// <summary>
/// Localised strings by key (Strings/strings.json: the design's catalog plus <c>Error_*</c>).
/// Placeholders are named: <c>{t}</c>, <c>{n}</c>, <c>{r}</c>, <c>{r0}</c>, <c>{ms}</c>, <c>{d}</c>,
/// <c>{team}</c>, <c>{reason}</c>, <c>{v}</c>, <c>{b}</c>, <c>{m}</c>, <c>{c}</c>.
/// </summary>
public interface ILocalizer
{
    /// <summary>"zh" or "en": the catalog in use; also picks date formats.</summary>
    string Language { get; }
    string Get(string key);
    string Format(string key, params (string Name, object? Value)[] args) => Placeholders.Fill(Get(key), args);
}

public interface IAppLog
{
    void Info(string message);
    void Warn(string message);
    void Error(string message);
}

/// <summary>Confirmation dialogs the view models ask for (texts: <see cref="PromptTexts"/>).</summary>
public enum PromptKind { InstallService, UninstallService, SignOut }

/// <summary>
/// Modal one-off UI: confirmations and one-off failures (ContentDialog / AdwAlertDialog /
/// NSAlert). The design has no transient banner: persistent states render in the content, and
/// failures of user actions come here. Called on the UI thread; the platform shows the main
/// window first when it is hidden (e.g. the action came from the tray) and queues dialogs so
/// only one is open at a time.
/// </summary>
public interface IUserPrompts
{
    /// <summary>True when the user confirmed (primary button), false on cancel / close.</summary>
    Task<bool> ConfirmAsync(PromptKind kind);
    /// <summary>A one-off failure with a single "OK" button (<c>ok</c>).</summary>
    Task ShowErrorAsync(string title, string message);
}

/// <summary>Dialog texts of a <see cref="PromptKind"/>, from the shared catalog.</summary>
public sealed record PromptTexts(string Title, string Message, string Confirm, string Cancel)
{
    public static PromptTexts For(PromptKind kind, ILocalizer strings) => kind switch
    {
        PromptKind.InstallService => new(strings.Get("installT"), strings.Get("installD"), strings.Get("installGo"), strings.Get("cancel")),
        PromptKind.UninstallService => new(strings.Get("uninstallQ"), strings.Get("uninstallD"), strings.Get("uninstallConfirm"), strings.Get("cancel")),
        PromptKind.SignOut => new(strings.Get("signOutQ"), strings.Get("signOutD"), strings.Get("signOut"), strings.Get("cancel")),
        _ => throw new ArgumentOutOfRangeException(nameof(kind)),
    };
}

/// <summary>Windows the app shows (see <see cref="MainViewModel.ShowRequested"/>).</summary>
public enum AppSurface { Main, Settings, MessageCenter }
