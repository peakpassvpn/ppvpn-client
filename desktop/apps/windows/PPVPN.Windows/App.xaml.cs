using Microsoft.UI.Dispatching;
using Microsoft.UI.Xaml;
using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;
using PPVPN.Windows.Platform;
using PPVPN.Windows.Services;
using PPVPN.Windows.Strings;

namespace PPVPN.Windows;

public partial class App : Application
{
    MainWindow? _window;
    MessageCenterWindow? _messages;
    TrayIconHost? _tray;
    bool _quitting;
    DispatcherQueueTimer? _updateQuitTimer;
    PushAgentAutostart? _pushAgent;

    public App()
    {
        InitializeComponent();
        UnhandledException += (_, e) => Log?.Error($"unhandled: {e.Exception}");
    }

    public static MainViewModel ViewModel { get; private set; } = null!;

    public static JsonSettingsStore Settings { get; private set; } = null!;

    public static Prompts Prompts { get; private set; } = null!;

    public static IAppServices Services { get; private set; } = null!;

    public static IAppLog? Log { get; private set; }

    public static StartupOptions Options { get; private set; } = new();

    public static new App Current => (App)Application.Current;

    public MainWindow? MainWindow => _window;

    public MessageCenterWindow? MessageCenter => _messages;

    internal TrayIconHost? Tray => _tray;

    protected override void OnLaunched(LaunchActivatedEventArgs args)
    {
        Options = StartupOptions.Parse(Environment.GetCommandLineArgs().Skip(1));
        var paths = AppPaths.Create();
        var log = new FileAppLog(paths.LogDir);
        Log = log;
        var settings = JsonSettingsStore.Load(paths.SettingsFile);
        Settings = settings;
        var services = new WindowsAppServices(Options, settings);
        Services = services;
        var strings = new JsonLocalizer(log);
        Loc.Strings = strings;
        LaunchAtLogin.Refresh();

        var apiBase = BuildInfo.EffectiveApiBase(settings);
        // CoreBinDir: ppvpn-core.exe is installed next to ppvpn.exe.
        var config = new ClientConfig(apiBase, paths.DataDir, paths.LogDir, AppContext.BaseDirectory, "windows", services.AppVersion);
        log.Info($"app {services.AppVersion} ({services.BuildNumber}) starting, pid {Environment.ProcessId}, UI thread {Environment.CurrentManagedThreadId}, args [{string.Join(' ', Environment.GetCommandLineArgs().Skip(1))}]");
        log.Info($"backend: {(Options.UseFake ? $"fake ({Options.Fake.PersonalTeam})" : "ppvpn-client")}, api {apiBase}{(apiBase == BuildInfo.ApiBase ? "" : $" (override; build default {BuildInfo.ApiBase})")}, strings {strings.Language}");
        if (Options.Unknown.Count > 0) log.Warn($"ignored command-line switches: {string.Join(' ', Options.Unknown)}");

        Prompts = new Prompts(() => _window?.Content?.XamlRoot, ShowMainWindow);
        try
        {
            ViewModel = new MainViewModel(
                Options.UseFake
                    ? FakeClientBackend.Factory(config, new WindowsPlatformHooks(CredentialStore.FakeTarget, Options.FakeService), Options.Fake)
                    : FfiClientBackend.Factory(config, new WindowsPlatformHooks()),
                services, settings, strings, log, Prompts);
        }
        catch (Exception error)
        {
            // E.g. ppvpn_client.dll missing or not matching the bindings: a broken installation.
            log.Error($"client start failed: {error}");
            throw;
        }
        if (!Options.UseFake) log.Info($"ppvpn_client loaded from {NativeLoader.LoadedFrom ?? "(default probing)"}");
        // Interfaces, addresses and connectivity (our own TUN adapter included): the view model debounces.
        System.Net.NetworkInformation.NetworkChange.NetworkAddressChanged += (_, _) => ViewModel.OnNetworkChanged();
        System.Net.NetworkInformation.NetworkChange.NetworkAvailabilityChanged += (_, _) => ViewModel.OnNetworkChanged();
        log.Info(AppNotifications.IsRegistered
            ? $"notifications registered as {AppNotifications.Aumid}"
            : $"notification clicks unavailable: {AppNotifications.RegistrationError}");

        _window = new MainWindow(ViewModel, settings);
        services.AppearanceHandler = ApplyAppearance;
        ApplyAppearance(Options.Theme ?? settings.Appearance);

        _tray = new TrayIconHost(ViewModel);
        ViewModel.ShowRequested += OnShowRequested;
        ViewModel.QuitRequested += Quit;

        var dispatcher = DispatcherQueue.GetForCurrentThread();
        Program.Reactivated += () => dispatcher.TryEnqueue(ShowMainWindow);
        // The installer sets this event to make us quit cleanly before it replaces files.
        QuitSignal.Listen(() => dispatcher.TryEnqueue(() =>
        {
            Log?.Info("quit requested by the installer");
            Quit();
        }));

        var fromNotification = false;
        AppNotifications.Attach(activation =>
        {
            fromNotification = true;
            dispatcher.TryEnqueue(() => OnNotificationActivated(activation));
        });

        if (!Options.Background && !fromNotification) _window.ShowAndFocus();
        // The push agent shows notifications while the app is closed; the fake never registers a
        // device, so fake runs leave the real agent alone unless asked to (--fake-push-agent).
        if (!Options.UseFake || Options.FakePushAgent) _pushAgent = PushAgentAutostart.Attach(ViewModel, paths.DataDir, log);
        DemoDriver.Attach(ViewModel, _window, Options);
        AppUpdater.Start(services.AppVersion, settings.AutoCheckUpdates, log, () => dispatcher.TryEnqueue(QuitForUpdate));
    }

    void ApplyAppearance(Appearance appearance)
    {
        _window?.ApplyAppearance(appearance);
        _messages?.ApplyAppearance(appearance);
    }

    public void ShowMainWindow() => _window?.ShowAndFocus();

    /// <summary>App.Core asks for a window: the tray, "Account Settings…", the unread line, a notification.</summary>
    void OnShowRequested(AppSurface surface, ulong? messageId)
    {
        switch (surface)
        {
            case AppSurface.Main:
                ShowMainWindow();
                break;
            case AppSurface.Settings:
                ShowMainWindow();
                _window?.NavigateTo("settings");
                break;
            case AppSurface.MessageCenter:
                ShowMessageCenter(messageId);
                break;
        }
    }

    /// <summary>
    /// The message center window: created once, afterwards only brought to the front. With a
    /// <paramref name="messageId"/> (a notification click) App.Core opens the message itself,
    /// once signed in, so the list is not refreshed here.
    /// </summary>
    public void ShowMessageCenter(ulong? messageId = null)
    {
        if (_messages is null)
        {
            _messages = new MessageCenterWindow(ViewModel, Settings, _window?.AppWindow);
            _messages.ApplyAppearance(Options.Theme ?? Settings.Appearance);
            _messages.Closed += (_, _) => _messages = null;
        }
        _messages.ShowAndFocus(refresh: messageId is null);
    }

    /// <summary>
    /// A notification was clicked (UI thread). App.Core resolves the push id: it opens the push's
    /// link, or asks for the message center (<see cref="OnShowRequested"/>) on its message or on
    /// a read-only detail of the push.
    /// </summary>
    void OnNotificationActivated(NotificationActivation activation)
    {
        Log?.Info($"notification {activation.Id} clicked");
        _ = ViewModel.Inbox.ActivateNotification(activation.Id);
    }

    /// <summary>
    /// WinSparkle started the installer. Disappear now, but keep running until
    /// the installer signals <see cref="QuitSignal"/>: that way it sees the app
    /// running, stops it through the normal quit path and relaunches it after
    /// the upgrade. If no signal arrives, quit on our own.
    /// </summary>
    void QuitForUpdate()
    {
        if (_quitting) return;
        _tray?.Dispose();
        _tray = null;
        _window?.AppWindow.Hide();
        _messages?.AppWindow.Hide();
        var timer = DispatcherQueue.GetForCurrentThread().CreateTimer();
        timer.Interval = TimeSpan.FromSeconds(30);
        timer.IsRepeating = false;
        timer.Tick += (_, _) =>
        {
            Log?.Warn("update: installer did not ask us to quit, quitting");
            Quit();
        };
        timer.Start();
        _updateQuitTimer = timer;
    }

    /// <summary>
    /// Hide everything first, then stop the backend on the thread pool
    /// (Shutdown blocks for up to ~10 s), then exit.
    /// </summary>
    public async void Quit()
    {
        if (_quitting) return;
        _quitting = true;
        Log?.Info("quit requested");
        _tray?.Dispose();
        if (_window?.IsShown == true) _window.SavePlacement();
        _messages?.SavePlacement();
        _window?.AppWindow.Hide();
        _messages?.AppWindow.Hide();
        try
        {
            await ViewModel.ShutdownAsync();
        }
        catch (Exception error)
        {
            Log?.Error($"shutdown failed: {error}");
        }
        _updateQuitTimer?.Stop();
        _pushAgent?.Dispose();
        AppUpdater.Cleanup();
        Log?.Info("quit");
        _messages?.CloseForReal();
        _window?.CloseForReal();
        Exit();
    }
}
