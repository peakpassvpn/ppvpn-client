using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.App.Core.Backend;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// App-wide state for the main window, the title bar, the account menu, the tray and the
/// dialogs. Everything is derived from the backend snapshot and the listener callbacks; views
/// hold no connection state of their own. Must be created on the UI thread (it captures its
/// SynchronizationContext). The partial files hold the areas: Auth, Connection, Account, Tray.
/// </summary>
public sealed partial class MainViewModel : ObservableObject
{
    readonly IAppServices _services;
    readonly ILocalizer _strings;
    readonly IAppLog _log;
    readonly IUserPrompts _prompts;
    readonly TimeProvider _time;
    readonly SynchronizedListener _listener;
    readonly CancellationTokenSource _life = new();
    ClientErrorInfo? _loggedError;
    ClientErrorInfo? _loggedReason;
    ProfileStatus? _loggedStatus;
    bool _applying;
    int _ticks;

    public MainViewModel(
        Func<ClientListener, IClientBackend> createBackend,
        IAppServices services,
        ISettingsStore settings,
        ILocalizer strings,
        IAppLog log,
        IUserPrompts prompts,
        TimeProvider? time = null)
    {
        _services = services;
        _strings = strings;
        _log = log;
        _prompts = prompts;
        _time = time ?? TimeProvider.System;
        _listener = new SynchronizedListener(Apply, OnProbeResult, OnTraffic, log.Info);
        Backend = createBackend(_listener);
        PurchaseUrl = Backend.PurchaseUrl();
        RoutingRulesUrl = Backend.RoutingRulesUrl();
        Nodes = new NodesViewModel(this, strings, services);
        Nodes.CatalogChanged += OnCatalogChanged;
        Logs = new LogsViewModel(Backend.LogDir(), services, strings, _time);
        Settings = new SettingsViewModel(this, settings, services, strings);
        Inbox = new InboxViewModel(this, strings, services, _time);
        // The crate does not push the initial state; read it.
        Apply(Backend.Snapshot());
        RunTicker();
    }

    public IClientBackend Backend { get; }
    public NodesViewModel Nodes { get; }
    public LogsViewModel Logs { get; }
    public SettingsViewModel Settings { get; }
    public InboxViewModel Inbox { get; }
    public SynchronizedListener Listener => _listener;
    internal ILocalizer Strings => _strings;
    internal IAppLog Log => _log;
    internal IUserPrompts Prompts => _prompts;
    internal TimeProvider Time => _time;
    internal IAppServices Services => _services;

    /// <summary>The latest snapshot (already applied).</summary>
    [ObservableProperty] ClientSnapshot snapshot = ClientSnapshots.Initial;


    // --- surfaces --------------------------------------------------------

    /// <summary>
    /// Show a window: the main window, Settings (a page on Windows, a dialog on Linux) or the
    /// message center (with a message id to open its detail). Raised by the tray items, the
    /// account menu's "Account Settings…", the unread line and notification activation.
    /// </summary>
    public event Action<AppSurface, ulong?>? ShowRequested;

    /// <summary>The tray's "Quit PPVPN": the platform calls <see cref="ShutdownAsync"/> and exits.</summary>
    public event Action? QuitRequested;

    internal void RequestShow(AppSurface surface, ulong? messageId = null) => ShowRequested?.Invoke(surface, messageId);

    [RelayCommand]
    void OpenMain() => RequestShow(AppSurface.Main);

    /// <summary>Works signed out too (General and Advanced only).</summary>
    [RelayCommand]
    void OpenSettings() => RequestShow(AppSurface.Settings);

    [RelayCommand]
    void OpenMessages() => RequestShow(AppSurface.MessageCenter);

    [RelayCommand]
    void Quit() => QuitRequested?.Invoke();

    public bool CanCheckForUpdates => _services.CanCheckForUpdates;

    [RelayCommand]
    void CheckForUpdates() => _services.CheckForUpdates();

    // --- running actions ---------------------------------------------------

    /// <summary>
    /// Run a backend action. Failures never become a banner (the design has none):
    /// <list type="bullet">
    /// <item>Cancelled (a superseded sign-in, a dismissed prompt): logged only.</item>
    /// <item>A code the current profile status already shows (e.g. a refresh while
    /// NoSubscription): logged only, the content shows it.</item>
    /// <item><paramref name="quiet"/> returns true (the state shows it, e.g. enhanced mode ended
    /// in Error): logged only.</item>
    /// <item>Otherwise a one-off dialog: <paramref name="titleKey"/> + the error message.</item>
    /// </list>
    /// Returns whether the action succeeded.
    /// </summary>
    internal async Task<bool> RunAsync(string name, Func<Task> action, string titleKey = "errorT",
        Func<Exception, bool>? quiet = null, Func<Exception, string>? message = null)
    {
        try
        {
            await action();
            return true;
        }
        catch (ClientException.Cancelled)
        {
            _log.Info($"{name}: cancelled");
        }
        catch (Exception error)
        {
            if (IsProfileStateError(error) || quiet?.Invoke(error) == true)
            {
                _log.Info($"{name}: {ErrorMessages.Describe(error)} (shown in the content)");
            }
            else
            {
                _log.Error($"{name} failed: {ErrorMessages.Describe(error)}");
                await _prompts.ShowErrorAsync(_strings.Get(titleKey), message?.Invoke(error) ?? _strings.Message(error));
            }
        }
        finally
        {
            SyncControls();
        }
        return false;
    }

    /// <summary>A background action: failures are logged only.</summary>
    internal async Task RunQuietlyAsync(string name, Func<Task> action)
    {
        try
        {
            await action();
        }
        catch (Exception error)
        {
            _log.Warn($"{name} failed: {ErrorMessages.Describe(error)}");
        }
    }

    /// <summary>The failure is what the current profile status already shows (e.g. a refresh while NoSubscription).</summary>
    bool IsProfileStateError(Exception error) =>
        ErrorMessages.Code(error) is { } code && code == ErrorMessages.Code(Snapshot.ProfileStatus);

    // --- listener (always on the UI thread, see SynchronizedListener) -------

    void Apply(ClientSnapshot next)
    {
        var previous = Snapshot;
        Snapshot = next;
        _applying = true;
        try
        {
            ApplyAuth(previous, next);
            ApplyAccess(next);
            Nodes.OnSnapshot(previous, next);
            ApplyConnection(next);
            ApplyAccount(next);
            Inbox.OnSnapshot(next);
            UnreadNotifications = Inbox.UnreadCount;
            LogSnapshot(next);
        }
        finally
        {
            _applying = false;
        }
        RefreshTray();

        if (next.Auth is AuthState.SignedIn && (previous.Auth is not AuthState.SignedIn || !_teamsLoaded))
        {
            _teamsLoaded = true;
            _ = LoadTeamsAsync();
        }
    }

    void OnProbeResult(ProbeResult result)
    {
        Nodes.OnProbeResult(result);
        if (result.NodeId == Snapshot.SelectedNodeId)
        {
            ApplyConnection(Snapshot);
            RefreshTray();
        }
    }

    void OnTraffic(TrafficSample sample)
    {
        UpRate = Formatting.Rate(sample.UpBps);
        DownRate = Formatting.Rate(sample.DownBps);
    }

    /// <summary>
    /// A new push message. OS notifications come from the push agent now, so the main app only
    /// adds it to a loaded list; <see cref="InboxViewModel.ActivateNotification"/> stays the
    /// entry point for notification clicks forwarded by the agent.
    /// </summary>

    void OnCatalogChanged()
    {
        _applying = true;
        try
        {
            ApplyConnection(Snapshot);
        }
        finally
        {
            _applying = false;
        }
        RefreshTray();
    }

    /// <summary>Re-derive two-way controls after an action (e.g. a switch whose action failed or was cancelled).</summary>
    void SyncControls()
    {
        _applying = true;
        try
        {
            SelectedTeam = Teams.FirstOrDefault(t => t.Id == Snapshot.Team?.Id);
            CurrentNode = Nodes.Items.FirstOrDefault(i => i.Id == Snapshot.SelectedNodeId);
            // Values may be unchanged; views bound to a control the user flipped must resync.
            OnPropertyChanged(nameof(ConnectSwitch));
            OnPropertyChanged(nameof(ConnectionMode));
        }
        finally
        {
            _applying = false;
        }
    }

    void LogSnapshot(ClientSnapshot next)
    {
        if (next.LastError is { } error && error != _loggedError)
            _log.Warn($"last_error: {error.Code} — {error.Detail}");
        _loggedError = next.LastError;
        if (next.Connection.Reason is { } reason && reason != _loggedReason)
            _log.Warn($"connection ({next.ConnectionMode}): {next.Connection.Phase} {reason.Code} — {reason.Detail}");
        _loggedReason = next.Connection.Reason;
        if (next.ProfileStatus is ProfileStatus.Invalid { Error: var invalid } && !Equals(next.ProfileStatus, _loggedStatus))
            _log.Warn($"profile invalid: {invalid.Code} — {invalid.Detail}");
        _loggedStatus = next.ProfileStatus;
    }

    // --- clock -----------------------------------------------------------------

    /// <summary>1 s ticker: the sign-in countdown, and every 60 s the relative message times.</summary>
    async void RunTicker()
    {
        var token = _life.Token;
        try
        {
            while (!token.IsCancellationRequested)
            {
                await Task.Delay(TimeSpan.FromSeconds(1), _time, token);
                Tick();
            }
        }
        catch (OperationCanceledException) { }
    }

    /// <summary>One tick of the ticker (tests call it directly with a fake <see cref="TimeProvider"/>).</summary>
    internal void Tick()
    {
        TickCountdown();
        if (++_ticks % 60 == 0) Inbox.RefreshTimes();
    }

    /// <summary>
    /// Stop the backend before quitting. The native shutdown blocks for up to ~10 s;
    /// <see cref="IClientBackend.ShutdownAsync"/> runs it on the thread pool.
    /// </summary>
    public async Task ShutdownAsync()
    {
        _life.Cancel();
        StopNetworkWatch();
        Logs.Stop();
        await Backend.ShutdownAsync();
        var (received, offThread, misdelivered) = _listener.Stats;
        _log.Info($"listener: {received} callbacks, {offThread} from background threads, {misdelivered} delivered off the UI thread");
    }
}
