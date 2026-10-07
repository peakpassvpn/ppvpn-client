using System.Globalization;
using System.Reflection;
using System.Runtime.InteropServices;
using PPVPN.App.Core.ViewModels;

namespace PPVPN.Windows.Platform;

/// <summary>
/// Auto-update through WinSparkle (native WinSparkle.dll next to ppvpn.exe).
/// The appcast URL and the EdDSA public key are build properties
/// (PPVPN_UPDATE_FEED_URL, PPVPN_UPDATE_PUBLIC_KEY) embedded as assembly
/// metadata; without both, updates are disabled and the UI hides
/// "Check for updates".
///
/// Flow: WinSparkle checks the Sparkle-format appcast (automatically on its
/// own schedule, or on demand), downloads the installer, verifies its
/// sparkle:edSignature against the embedded key, runs it with
/// sparkle:installerArguments (/S) and asks the app to shut down. The app
/// then hides immediately and waits for the installer to signal
/// <see cref="QuitSignal"/> (the installer records that the app was running
/// and relaunches it afterwards), falling back to quitting on its own.
/// </summary>
public static class AppUpdater
{
    const string Dll = "WinSparkle.dll";

    /// <summary>HKCU key for WinSparkle's own settings (last check, skipped version).</summary>
    const string RegistryPath = @"Software\PPVPN\WinSparkle";

    static readonly string FeedUrl = Metadata("PPVPN.UpdateFeedUrl");
    static readonly string PublicKey = Metadata("PPVPN.UpdatePublicKey");

    // Delegates handed to native code must stay alive for the process lifetime.
    static CanShutdownCallback? _canShutdown;
    static VoidCallback? _shutdownRequest;
    static VoidCallback? _error;
    static VoidCallback? _foundUpdate;
    static VoidCallback? _noUpdate;
    static bool _started;
    static IAppLog? _log;

    /// <summary>True when this build has a feed URL, a well-formed key and WinSparkle.dll.</summary>
    public static bool IsConfigured { get; } =
        FeedUrl.Length > 0 && IsEd25519PublicKey(PublicKey) && File.Exists(Path.Combine(AppContext.BaseDirectory, Dll));

    /// <summary>
    /// Configures WinSparkle and starts it (which runs the scheduled
    /// background check). Call on the UI thread once the main window exists.
    /// </summary>
    /// <param name="shutdownRequested">Called on a WinSparkle thread after the installer started.</param>
    public static void Start(string displayVersion, bool automaticChecks, IAppLog log, Action shutdownRequested)
    {
        _log = log;
        if (!IsConfigured)
        {
            log.Info("updates disabled: no feed URL or no valid public key in this build");
            return;
        }
        try
        {
            var buildVersion = Metadata("PPVPN.BuildVersion");
            // Same rule as the app's resources: English for English UIs, else Chinese.
            var uiLanguage = CultureInfo.CurrentUICulture.Name;
            var updaterLanguage = uiLanguage.StartsWith("en", StringComparison.OrdinalIgnoreCase) ? "en" : "zh_CN";
            win_sparkle_set_lang(updaterLanguage);
            win_sparkle_set_appcast_url(FeedUrl);
            if (win_sparkle_set_eddsa_public_key(PublicKey) != 1)
            {
                log.Error("updates disabled: invalid EdDSA public key");
                return;
            }
            win_sparkle_set_app_details("PeakPass Labs LLC", "PPVPN", displayVersion);
            // sparkle:version in the appcast is the monotonic FileVersion (e.g. 0.3.0.12).
            if (buildVersion.Length > 0) win_sparkle_set_app_build_version(buildVersion);
            win_sparkle_set_registry_path(RegistryPath);
            // Settings → General → "Automatically check for updates" (settings.json).
            win_sparkle_set_automatic_check_for_updates(automaticChecks ? 1 : 0);

            _canShutdown = () => 1;
            _shutdownRequest = () =>
            {
                _log?.Info("update: installer launched, shutting down");
                shutdownRequested();
            };
            _error = () => _log?.Warn("update: WinSparkle reported an error (feed, download or signature)");
            _foundUpdate = () => _log?.Info("update: update available");
            _noUpdate = () => _log?.Info("update: no update available");
            win_sparkle_set_can_shutdown_callback(_canShutdown);
            win_sparkle_set_shutdown_request_callback(_shutdownRequest);
            win_sparkle_set_error_callback(_error);
            win_sparkle_set_did_find_update_callback(_foundUpdate);
            win_sparkle_set_did_not_find_update_callback(_noUpdate);

            win_sparkle_init();
            _started = true;
            log.Info($"updates enabled: feed {FeedUrl}, version {displayVersion} (build {buildVersion}), language {updaterLanguage} (UI {uiLanguage})");
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException or BadImageFormatException)
        {
            log.Error($"updates disabled: {error.Message}");
        }
    }

    /// <summary>Turns WinSparkle's scheduled background checks on or off (takes effect immediately).</summary>
    public static void SetAutomaticChecks(bool enabled)
    {
        if (!IsConfigured) return;
        try
        {
            win_sparkle_set_automatic_check_for_updates(enabled ? 1 : 0);
            _log?.Info($"update: automatic checks {(enabled ? "on" : "off")}");
        }
        catch (Exception error) when (error is DllNotFoundException or EntryPointNotFoundException)
        {
            _log?.Warn($"update: {error.Message}");
        }
    }

    /// <summary>User-initiated check with WinSparkle's progress and result UI.</summary>
    public static void CheckWithUi()
    {
        if (!_started) return;
        _log?.Info("update: manual check");
        win_sparkle_check_update_with_ui();
    }

    /// <summary>Cancels pending work and stops WinSparkle's threads. Call on quit.</summary>
    public static void Cleanup()
    {
        if (!_started) return;
        _started = false;
        win_sparkle_cleanup();
    }

    static bool IsEd25519PublicKey(string value)
    {
        var bytes = new byte[64];
        return Convert.TryFromBase64String(value, bytes, out var length) && length == 32;
    }

    static string Metadata(string key) =>
        typeof(AppUpdater).Assembly.GetCustomAttributes<AssemblyMetadataAttribute>()
            .FirstOrDefault(attribute => attribute.Key == key)?.Value?.Trim() ?? "";

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    delegate int CanShutdownCallback();

    [UnmanagedFunctionPointer(CallingConvention.Cdecl)]
    delegate void VoidCallback();

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_init();

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_cleanup();

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_lang([MarshalAs(UnmanagedType.LPStr)] string lang);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_appcast_url([MarshalAs(UnmanagedType.LPStr)] string url);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern int win_sparkle_set_eddsa_public_key([MarshalAs(UnmanagedType.LPStr)] string key);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Unicode)]
    static extern void win_sparkle_set_app_details(string companyName, string appName, string appVersion);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl, CharSet = CharSet.Unicode)]
    static extern void win_sparkle_set_app_build_version(string build);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_registry_path([MarshalAs(UnmanagedType.LPStr)] string path);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_automatic_check_for_updates(int state);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_can_shutdown_callback(CanShutdownCallback callback);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_shutdown_request_callback(VoidCallback callback);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_error_callback(VoidCallback callback);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_did_find_update_callback(VoidCallback callback);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_set_did_not_find_update_callback(VoidCallback callback);

    [DllImport(Dll, CallingConvention = CallingConvention.Cdecl)]
    static extern void win_sparkle_check_update_with_ui();
}

/// <summary>
/// Named event the installer sets to ask a running ppvpn.exe in its session
/// to quit through the normal path (backend Shutdown included) before files
/// are replaced. The name is shared with installer/ppvpn.nsi.
/// </summary>
public static class QuitSignal
{
    public const string EventName = @"Local\PPVPN.Desktop.Quit";

    static EventWaitHandle? _event;

    /// <param name="quit">Called on a thread-pool thread when the event is set.</param>
    public static void Listen(Action quit)
    {
        try
        {
            _event = new EventWaitHandle(false, EventResetMode.AutoReset, EventName);
            _ = ThreadPool.RegisterWaitForSingleObject(_event, (_, _) => quit(), null, Timeout.Infinite, executeOnlyOnce: true);
        }
        catch (Exception error) when (error is IOException or UnauthorizedAccessException or WaitHandleCannotBeOpenedException)
        {
            // Without the event the installer falls back to terminating the process.
        }
    }
}
