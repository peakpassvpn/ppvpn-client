using System.Diagnostics;
using System.Globalization;
using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;
using PPVPN.Linux.Platform;
using PPVPN.Linux.Tray;
using PPVPN.Linux.UI;

namespace PPVPN.Linux.App;

/// <summary>
/// Single-instance GTK application: a second launch hands its command line to the running
/// instance, which presents the main window or opens a clicked notification. Owns the view
/// model, the windows and the tray icon.
/// </summary>
public sealed class PPVPNApplication
{
    public const string Id = "com.peakpassvpn.ppvpn.desktop";

    private readonly Adw.Application _app;
    private readonly bool _startInBackground;
    private readonly Func<ClientConfig, Func<ClientListener, IClientBackend>> _backend;
    private MainViewModel? _vm;
    private Gio.NetworkMonitor? _network;
    private Update.UpdateNotifier? _updates;
    private LinuxAppServices? _services;
    private TrayController? _trayController;
    private StatusNotifierItem? _tray;
    private MainWindow? _main;
    private Adw.PreferencesWindow? _settings;
    private Adw.Window? _messages;
    private bool _quitting;

    /// <param name="backend">Defaults to the Rust client; the fake one drives UI previews.</param>
    public PPVPNApplication(bool startInBackground, Func<ClientConfig, Func<ClientListener, IClientBackend>>? backend = null)
    {
        _startInBackground = startInBackground;
        _backend = backend ?? (config => FfiClientBackend.Factory(config, new LinuxPlatformHooks()));
        _app = Adw.Application.New(Id, Gio.ApplicationFlags.HandlesCommandLine);
        _app.OnStartup += (_, _) => Startup();
        _app.OnActivate += (_, _) => Activate();
        _app.OnCommandLine += (_, args) =>
        {
            CommandLine(args.CommandLine.GetArguments(out int _));
            return 0;
        };
        _app.OnShutdown += (_, _) =>
        {
            _tray?.Dispose();
            // Quit normally shuts down through QuitAsync first; this is a no-op then. When the
            // last window closes without a tray, block here (up to ~10 s) instead.
            _vm?.Backend.ShutdownAsync().GetAwaiter().GetResult();
        };
    }

    public int Run(string[] args) => _app.RunWithSynchronizationContext(args);

    private void Startup()
    {
        // Keep running with every window closed while a tray icon can bring it back.
        _app.Hold();
        LoadIcons();
        Theme.Install();

        var settings = new SettingsStore();
        var services = new LinuxAppServices(settings);
        services.ApplyAppearance(settings.Appearance);
        var log = new AppLog(LinuxPaths.LogDir);
        var strings = new JsonLocalizer(log);
        L.Initialize(strings);
        var config = new ClientConfig(
            ApiBase: string.IsNullOrWhiteSpace(settings.ApiBaseOverride) ? services.DefaultApiBase : settings.ApiBaseOverride,
            DataDir: LinuxPaths.DataDir,
            LogDir: LinuxPaths.LogDir,
            CoreBinDir: LinuxPaths.AppDir,
            Platform: "linux",
            AppVersion: services.AppVersion);
        Directory.CreateDirectory(config.DataDir);
        Directory.CreateDirectory(config.LogDir);
        log.Info($"ppvpn {services.AppVersion} starting, api {config.ApiBase}");

        // RunWithSynchronizationContext installed the GLib context the view model captures.
        var prompts = new LinuxUserPrompts(strings, () => MainWindow().Window);
        _vm = new MainViewModel(_backend(config), services, settings, strings, log, prompts);
        _vm.ShowRequested += Show;
        _vm.QuitRequested += () => _ = QuitAsync();

        // The system's network changed (a link, address or route): the view model debounces it
        // and has the client check its data path. GLib also reports one change as the monitor
        // starts, which the debounce absorbs.
        _network = Gio.NetworkMonitorHelper.GetDefault();
        _network.OnNetworkChanged += (_, _) => _vm.OnNetworkChanged();

        _updates = new Update.UpdateNotifier(log, services.AppVersion);
        services.Updates = _updates;
        _updates.CheckFinished += text => MainWindow().Toast(text);
        if (services.AutoCheckUpdates) _updates.StartSchedule();
        _services = services;

        AddAction("preferences", () => _vm.OpenSettingsCommand.Execute(null), "<Control>comma");
        AddAction("quit", () => _vm.QuitCommand.Execute(null), "<Control>q");

        _trayController = new TrayController(_vm, _updates, () => MainWindow().Window.Present());
        _ = StartTrayAsync();
        StartPushAgent(log);
    }

    /// <summary>
    /// The push agent shows notifications whether or not the app runs. Autostart covers later
    /// sessions; starting it here covers the session the package was installed in. It exits
    /// at once when it is already running.
    /// </summary>
    private static void StartPushAgent(AppLog log)
    {
        var path = Path.Combine(LinuxPaths.AppDir, "ppvpn-push-agent");
        if (!File.Exists(path)) return; // development build
        try
        {
            using var process = Process.Start(new ProcessStartInfo(path) { UseShellExecute = false });
        }
        catch (Exception error)
        {
            log.Warn($"push agent not started: {error.Message}");
        }
    }

    /// <summary>
    /// This launch's or a later one's arguments. A notification click arrives as
    /// <c>--open-notification &lt;push id&gt;</c> from the push agent; anything else activates.
    /// </summary>
    private void CommandLine(string[] args)
    {
        var open = Array.IndexOf(args, "--open-notification");
        if (open >= 0 && open + 1 < args.Length
            && ulong.TryParse(args[open + 1], NumberStyles.None, CultureInfo.InvariantCulture, out var id))
        {
            _firstActivation = false;
            _ = _vm!.Inbox.ActivateNotification(id);
            return;
        }
        Activate();
    }

    private void AddAction(string name, Action activate, string accelerator)
    {
        var action = Gio.SimpleAction.New(name, null);
        action.OnActivate += (_, _) => activate();
        _app.AddAction(action);
        _app.SetAccelsForAction($"app.{name}", [accelerator]);
    }

    private void Show(AppSurface surface, ulong? messageId)
    {
        switch (surface)
        {
            case AppSurface.Settings:
                _settings ??= SettingsWindow.Create(_vm!.Settings, MainWindow().Window);
                _settings.Present();
                break;
            case AppSurface.MessageCenter:
                // Opened next to the main window, and only once. A message id comes from a
                // notification click, whose ActivateNotification opens the message itself.
                _messages ??= MessageCenterWindow.Create(_vm!.Inbox, MainWindow().Window);
                // App.Core opens the message itself (ActivateNotification waits for sign-in).
                _messages.Present();
                break;
            default:
                MainWindow().Window.Present();
                break;
        }
    }

    /// <summary>Tray icons, served to the panel as a hicolor theme (StatusNotifierItem.IconThemePath).</summary>
    private static string IconThemePath => Path.Combine(LinuxPaths.AppDir, "Resources", "icons");

    /// <summary>
    /// The packages install the app icon into /usr/share/icons/hicolor; a copy next to the app
    /// covers development builds. It is a flat directory on purpose: adding a hicolor tree
    /// without index.theme to GTK's search path hides icons of the system theme.
    /// </summary>
    private static void LoadIcons()
    {
        Gtk.IconTheme.GetForDisplay(Gdk.Display.GetDefault()!).AddSearchPath(Path.Combine(LinuxPaths.AppDir, "Resources", "app-icon"));
        Gtk.Window.SetDefaultIconName(Id);
    }

    private async Task StartTrayAsync()
    {
        _tray = await StatusNotifierItem.StartAsync(
            _trayController!.Build(), IconThemePath, () => MainWindow().Window.Present(), SynchronizationContext.Current!);
        if (_tray is { } tray)
        {
            var (vm, controller) = (_vm!, _trayController!);
            tray.HostedChanged += ApplyTrayPresence;
            vm.Bind(() => tray.Update(controller.Build()), nameof(vm.Tray));
            _updates!.Bind(() => tray.Update(controller.Build()), TrayController.UpdateProperties);
        }
        ApplyTrayPresence();
    }

    private bool _holding = true;

    /// <summary>
    /// With a visible tray icon the app outlives its window. Without one (e.g. GNOME lacking the
    /// AppIndicator extension) closing the window quits, and a background launch shows the window
    /// after all.
    /// </summary>
    private void ApplyTrayPresence()
    {
        var hosted = _tray?.IsHosted == true;
        _main?.Window.SetHideOnClose(hosted);
        if (hosted && !_holding)
        {
            _app.Hold();
            _holding = true;
        }
        else if (!hosted && _holding)
        {
            _app.Release();
            _holding = false;
            if (_main?.Window.IsVisible() != true) MainWindow().Window.Present();
        }
    }

    private bool _firstActivation = true;

    private void Activate()
    {
        var present = !(_firstActivation && _startInBackground);
        _firstActivation = false;
        if (present) MainWindow().Window.Present();
    }

    private MainWindow MainWindow()
    {
        if (_main is not null) return _main;
        _main = new MainWindow(_app, _vm!, _updates!, _services!);
        // Closing only hides the window while a tray icon can bring it back.
        _main.Window.SetHideOnClose(_tray?.IsHosted == true);
        return _main;
    }

    /// <summary>Stops both cores (up to ~10 s, off the main thread), then quits.</summary>
    private async Task QuitAsync()
    {
        if (_quitting) return;
        _quitting = true;
        _main?.Window.SetVisible(false);
        _settings?.SetVisible(false);
        _messages?.SetVisible(false);
        await _vm!.ShutdownAsync();
        _app.Quit();
    }
}
