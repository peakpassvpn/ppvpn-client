using System.Collections.Concurrent;
using System.Globalization;
using PPVPN.App.Core.Backend;
using PPVPN.App.Core.ViewModels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Tests;

/// <summary>
/// A single-threaded "UI thread": a dedicated thread running posted callbacks in FIFO order, like
/// WinUI's DispatcherQueue or the GLib main loop. Test bodies run on it via <see cref="Run"/>.
/// </summary>
sealed class UiContext : SynchronizationContext, IDisposable
{
    readonly BlockingCollection<(SendOrPostCallback Callback, object? State)> _queue = new();
    readonly Thread _thread;
    readonly ConcurrentQueue<Exception> _errors = new();

    public UiContext()
    {
        using var started = new ManualResetEventSlim();
        _thread = new Thread(() =>
        {
            SetSynchronizationContext(this);
            CultureInfo.CurrentCulture = CultureInfo.InvariantCulture;
            CultureInfo.CurrentUICulture = CultureInfo.InvariantCulture;
            started.Set();
            foreach (var (callback, state) in _queue.GetConsumingEnumerable())
            {
                try { callback(state); }
                catch (Exception error) { _errors.Enqueue(error); }
            }
        })
        { IsBackground = true, Name = "test-ui" };
        _thread.Start();
        started.Wait();
    }

    public int ThreadId => _thread.ManagedThreadId;

    /// <summary>Exceptions that escaped a posted callback (e.g. an async void handler).</summary>
    public IReadOnlyCollection<Exception> Errors => _errors;

    public override void Post(SendOrPostCallback d, object? state)
    {
        if (!_queue.IsAddingCompleted)
        {
            try { _queue.Add((d, state)); } catch (InvalidOperationException) { }
        }
    }

    public override void Send(SendOrPostCallback d, object? state) => throw new NotSupportedException();

    public override SynchronizationContext CreateCopy() => this;

    /// <summary>
    /// Run <paramref name="body"/> on the UI thread and await it (with a timeout). Awaited, not
    /// waited on: blocking a test thread would take a thread-pool thread the fake backend's
    /// timers and pump need, and starve them on machines with few cores.
    /// </summary>
    public async Task RunAsync(Func<Task> body, int timeoutSeconds = 20)
    {
        var done = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        Post(async _ =>
        {
            try
            {
                await body();
                done.SetResult();
            }
            catch (Exception error)
            {
                done.SetException(error);
            }
        }, null);
        if (await Task.WhenAny(done.Task, Task.Delay(TimeSpan.FromSeconds(timeoutSeconds))) != done.Task)
            throw new TimeoutException("test body did not finish");
        await done.Task;
        Assert.Empty(Errors);
    }

    public void Dispose() => _queue.CompleteAdding();
}

static class Wait
{
    /// <summary>Poll <paramref name="condition"/> on the current (UI) context.</summary>
    public static async Task Until(Func<bool> condition, string what, int timeoutMs = 5000)
    {
        var deadline = Environment.TickCount64 + timeoutMs;
        while (!condition())
        {
            if (Environment.TickCount64 > deadline) throw new TimeoutException($"timed out waiting for: {what}");
            await Task.Delay(5);
        }
    }
}

public enum PromptOutcome { Allow, Cancel, Fail }

sealed class MemoryHooks : PlatformHooks
{
    public byte[]? Credential { get; set; }
    public bool ServiceInstalled { get; set; } = true;
    /// <summary>What the OS admin prompt does when installing the service.</summary>
    public PromptOutcome Install { get; set; } = PromptOutcome.Allow;
    public PromptOutcome Uninstall { get; set; } = PromptOutcome.Allow;
    public int InstallPrompts;
    public List<string> OpenedUrls { get; } = [];

    /// <summary>A saved session; <paramref name="configure"/> runs before anything reads the hooks.</summary>
    public static MemoryHooks SignedIn(Action<MemoryHooks>? configure = null)
    {
        var hooks = new MemoryHooks { Credential = "{\"access_token\":\"saved\"}"u8.ToArray() };
        configure?.Invoke(hooks);
        return hooks;
    }

    /// <summary>The keyring is locked: loading fails with <see cref="PlatformException.Locked"/>.</summary>
    public volatile bool Locked;
    public int Loads;

    public byte[]? CredentialLoad()
    {
        Interlocked.Increment(ref Loads);
        return Locked ? throw new PlatformException.Locked("LINUX_KEYRING_LOCKED: test") : Credential;
    }
    public void CredentialSave(byte[] blob) => Credential = blob;
    public void CredentialDelete() => Credential = null;
    public bool OpenUrl(string url)
    {
        lock (OpenedUrls) OpenedUrls.Add(url);
        return true;
    }
    public bool PrivilegedServiceInstalled() => ServiceInstalled;

    public void InstallPrivilegedService()
    {
        Interlocked.Increment(ref InstallPrompts);
        switch (Install)
        {
            case PromptOutcome.Cancel: throw new PlatformException.Cancelled();
            case PromptOutcome.Fail: throw new PlatformException.Failed("installer exited with 1603");
            default: ServiceInstalled = true; break;
        }
    }

    public void UninstallPrivilegedService()
    {
        switch (Uninstall)
        {
            case PromptOutcome.Cancel: throw new PlatformException.Cancelled();
            case PromptOutcome.Fail: throw new PlatformException.Failed("uninstaller exited with 1");
            default: ServiceInstalled = false; break;
        }
    }
}

/// <summary>Returns the key itself (with named arguments), so tests assert which string was chosen.</summary>
sealed class KeyLocalizer(string language = "zh") : ILocalizer
{
    public string Language { get; } = language;
    public string Get(string key) => key;
    public string Format(string key, params (string Name, object? Value)[] args) =>
        $"{key}({string.Join(", ", args.Select(a => $"{a.Name}={a.Value}"))})";
}

sealed class TestServices : IAppServices
{
    public string AppVersion => "1.4.0";
    public string BuildNumber => "2609";
    public string DefaultApiBase => "https://api.example.test";
    public List<string> Copied { get; } = [];
    public List<string> Opened { get; } = [];
    public void CopyText(string text) => Copied.Add(text);
    public bool OpenUrl(string url)
    {
        Opened.Add(url);
        return true;
    }
    public bool OpenFolder(string path) => true;
    public bool LaunchAtLogin { get; set; }
    public bool LaunchAtLoginBlocked { get; set; }
    public bool FailLaunchAtLogin { get; set; }
    public void SetLaunchAtLogin(bool enabled)
    {
        if (FailLaunchAtLogin) throw new InvalidOperationException("access denied");
        LaunchAtLogin = enabled;
    }
    public int OpenedLaunchSettings;
    public void OpenLaunchAtLoginSettings() => OpenedLaunchSettings++;
    public void ApplyAppearance(Appearance appearance) { }
    public bool CanCheckForUpdates { get; set; } = true;
    public int UpdateChecks;
    public void CheckForUpdates() => UpdateChecks++;
    public bool AutoCheckUpdates { get; set; } = true;
}

/// <summary>Answers confirmations from a queue (default: <see cref="DefaultAnswer"/>) and records errors.</summary>
sealed class TestPrompts : IUserPrompts
{
    public Queue<bool> Answers { get; } = new();
    public bool DefaultAnswer { get; set; } = true;
    public List<PromptKind> Asked { get; } = [];
    public List<(string Title, string Message)> Errors { get; } = [];

    public Task<bool> ConfirmAsync(PromptKind kind)
    {
        Asked.Add(kind);
        return Task.FromResult(Answers.Count > 0 ? Answers.Dequeue() : DefaultAnswer);
    }

    public Task ShowErrorAsync(string title, string message)
    {
        Errors.Add((title, message));
        return Task.CompletedTask;
    }
}

sealed class TestSettings : ISettingsStore
{
    readonly Dictionary<string, string?> _placements = [];
    public Appearance Appearance { get; set; }
    public string? ApiBaseOverride { get; set; }
    public bool BackgroundHintShown { get; set; }
    public string? GetWindowPlacement(string window) => _placements.GetValueOrDefault(window);
    public void SetWindowPlacement(string window, string? placement) => _placements[window] = placement;
    public int Saves;
    public void Save() => Saves++;
}

sealed class TestLog : IAppLog
{
    public ConcurrentQueue<string> Lines { get; } = new();
    public void Info(string message) => Lines.Enqueue("INFO " + message);
    public void Warn(string message) => Lines.Enqueue("WARN " + message);
    public void Error(string message) => Lines.Enqueue("ERROR " + message);
}

/// <summary>A clock the test moves by hand (timers stay real); local time zone UTC+8.</summary>
sealed class ManualTime(DateTimeOffset now) : TimeProvider
{
    public static readonly TimeZoneInfo Beijing = TimeZoneInfo.CreateCustomTimeZone("test+8", TimeSpan.FromHours(8), "UTC+8", "UTC+8");

    public DateTimeOffset Now { get; set; } = now;
    public override DateTimeOffset GetUtcNow() => Now.ToUniversalTime();
    public override TimeZoneInfo LocalTimeZone => Beijing;
    public void Advance(TimeSpan by) => Now += by;
}

/// <summary>A clock whose timers fire only in <see cref="Advance"/>, in due order (debounces).</summary>
sealed class SteppedTime(DateTimeOffset now) : TimeProvider
{
    readonly List<SteppedTimer> _timers = [];

    public DateTimeOffset Now { get; private set; } = now;
    public override DateTimeOffset GetUtcNow() => Now.ToUniversalTime();
    public override TimeZoneInfo LocalTimeZone => ManualTime.Beijing;

    public override ITimer CreateTimer(TimerCallback callback, object? state, TimeSpan dueTime, TimeSpan period)
    {
        var timer = new SteppedTimer(this, callback, state);
        lock (_timers) _timers.Add(timer);
        timer.Change(dueTime, period);
        return timer;
    }

    public void Advance(TimeSpan by)
    {
        var end = Now + by;
        while (true)
        {
            SteppedTimer? next;
            lock (_timers) next = _timers.Where(t => t.Due is { } due && due <= end).MinBy(t => t.Due);
            if (next is null) break;
            Now = next.Due!.Value;
            next.Fire();
        }
        Now = end;
    }

    sealed class SteppedTimer(SteppedTime clock, TimerCallback callback, object? state) : ITimer
    {
        TimeSpan _period;
        public DateTimeOffset? Due { get; private set; }

        public bool Change(TimeSpan dueTime, TimeSpan period)
        {
            lock (clock._timers)
            {
                Due = dueTime == Timeout.InfiniteTimeSpan ? null : clock.Now + dueTime;
                _period = period;
            }
            return true;
        }

        public void Fire()
        {
            lock (clock._timers) Due = _period == Timeout.InfiniteTimeSpan || _period == TimeSpan.Zero ? null : Due + _period;
            callback(state);
        }

        public void Dispose()
        {
            lock (clock._timers)
            {
                Due = null;
                clock._timers.Remove(this);
            }
        }

        public ValueTask DisposeAsync()
        {
            Dispose();
            return ValueTask.CompletedTask;
        }
    }
}

/// <summary>A MainViewModel over the fake backend; create it on the UI thread.</summary>
sealed class Harness : IDisposable
{
    public const string ApiBase = "https://api.example.test/v1";

    readonly string _root = Path.Combine(Path.GetTempPath(), "ppvpn-app-core-tests", Guid.NewGuid().ToString("N"));

    public Harness(FakeOptions? options = null, MemoryHooks? hooks = null, TimeProvider? time = null, ILocalizer? strings = null)
    {
        Hooks = hooks ?? new MemoryHooks();
        Config = new ClientConfig(ApiBase, Path.Combine(_root, "data"), Path.Combine(_root, "logs"), Path.Combine(_root, "bin"), "linux", "0.0.0-test");
        var opts = (options ?? new FakeOptions()) with
        {
            Browser = options?.Browser ?? FakeBrowserMode.Pretend,
            TimeScale = options is { TimeScale: not 1.0 } ? options.TimeScale : 0.02,
        };
        Main = new MainViewModel(
            listener => Backend = new FakeClientBackend(Config, Hooks, listener, opts),
            Services, Settings, strings ?? new KeyLocalizer(), Log, Prompts, time);
    }

    public MemoryHooks Hooks { get; }
    public ClientConfig Config { get; }
    public TestServices Services { get; } = new();
    public TestSettings Settings { get; } = new();
    public TestPrompts Prompts { get; } = new();
    public TestLog Log { get; } = new();
    public FakeClientBackend Backend { get; private set; } = null!;
    public MainViewModel Main { get; }

    /// <summary>Signed in with the profile, the standard core and the local proxies ready.</summary>
    public async Task ReadyAsync()
    {
        await Wait.Until(() => Main.IsSignedIn && Main.Nodes.CanProbe && Main.HasCurrentNodeProxy, "signed in and ready");
    }

    public void Dispose()
    {
        Backend.Dispose();
        try { Directory.Delete(_root, recursive: true); } catch (IOException) { }
    }
}

/// <summary>
/// A backend whose snapshots the test pushes by hand (through the listener, from a background
/// thread like the crate), for deterministic derivation tests. Actions are recorded.
/// </summary>
sealed class ScriptedBackend : IClientBackend
{
    readonly ClientListener _listener;
    ClientSnapshot _snapshot = ClientSnapshots.Initial;

    public ScriptedBackend(ClientListener listener) => _listener = listener;

    public List<string> Calls { get; } = [];
    public Node[] NodeList { get; set; } = FakeClientBackend.SampleNodes();
    public Team[] TeamList { get; set; } =
    [
        new(FakeClientBackend.PersonalTeamId, "个人", true, true),
        new("team-acme", "Acme Studio", false, true),
        new("team-lumen", "Lumen Labs", false, false),
    ];
    /// <summary>Thrown by the next action, if set.</summary>
    public Exception? Fail { get; set; }

    public static ClientSnapshot SignedIn(Func<ClientSnapshot, ClientSnapshot>? change = null)
    {
        var s = ClientSnapshots.Initial with
        {
            Auth = new AuthState.SignedIn(),
            Account = new Account("acct", "Alice", null, "alice@example.com"),
            Team = new Team(FakeClientBackend.PersonalTeamId, "个人", true, true),
            Profile = new ProfileSummary("rev-1", "2026-12-31T12:00:00Z", 11),
            ProfileStatus = new ProfileStatus.Ready(),
            Standard = new StandardState.Ready("rev-1"),
            ServiceInstalled = true,
            SelectedNodeId = "hk1",
            Connection = ConnectionStates.Off with { Detail = new ConnectionDetail("hk1-r0", null, null, 38) },
        };
        return change is null ? s : change(s);
    }

    /// <summary>Push a snapshot from a background thread (like the crate) and wait until the VM applied it.</summary>
    public async Task PushAsync(MainViewModel main, ClientSnapshot snapshot)
    {
        _snapshot = snapshot;
        await Task.Run(() => _listener.OnSnapshot(snapshot));
        await Wait.Until(() => Equals(main.Snapshot, snapshot), "snapshot applied");
    }

    /// <summary>
    /// Change what <see cref="Snapshot"/> returns without notifying the listener yet: the crate's
    /// state after it failed a call, before the snapshot callback reached the UI thread.
    /// </summary>
    public void Stage(ClientSnapshot snapshot) => _snapshot = snapshot;

    public ClientSnapshot Snapshot() => _snapshot;
    public string LogDir() => Path.Combine(Path.GetTempPath(), "ppvpn-app-core-tests", "scripted-logs");
    public string PurchaseUrl() => "https://www.example.test/dashboard/products";
    public string RoutingRulesUrl() => "https://www.example.test/dashboard/routing-rules";
    public void NetworkChanged() => Calls.Add("network_changed");

    Task Record(string call)
    {
        Calls.Add(call);
        if (Fail is { } error)
        {
            Fail = null;
            return Task.FromException(error);
        }
        return Task.CompletedTask;
    }

    /// <summary>The inbox, newest first.</summary>
    public List<InboxMessage> Inbox { get; } = [];
    public bool FailNotifications { get; set; }

    /// <summary>The pages <see cref="Notifications"/> was asked for, in order.</summary>
    public List<uint> NotificationPages { get; } = [];

    public Task<InboxPage> Notifications(uint page, uint pageSize)
    {
        NotificationPages.Add(page);
        return FailNotifications
            ? Task.FromException<InboxPage>(new ClientException.Failed(ErrorCode.NetworkUnreachable, "dns"))
            : Task.FromResult(new InboxPage(Inbox.Skip((int)((page - 1) * pageSize)).Take((int)pageSize).ToArray(), (uint)Inbox.Count));
    }
    public Task MarkNotificationRead(ulong id) => Record($"read {id}");
    public Task MarkNotificationUnread(ulong id) => Record($"unread {id}");
    public Task MarkAllNotificationsRead() => Record("read all");
    /// <summary>Pushes the agent "showed", by push id (<see cref="ShownPush"/>).</summary>
    public Dictionary<ulong, PushMessage> ShownPushes { get; } = [];
    public PushMessage? ShownPush(ulong pushId) => ShownPushes.GetValueOrDefault(pushId);
    public Task<DeviceCode> AuthStart() => Record("auth_start").ContinueWith(_ => new DeviceCode("ABCD-EFGH", "https://x.test/d", 600, true));
    public void AuthCancel() => Calls.Add("auth_cancel");
    /// <summary>What <see cref="RetryCredentialRestore"/> answers (a background retry is pending).</summary>
    public bool CredentialRetryPending { get; set; } = true;
    public bool RetryCredentialRestore()
    {
        Calls.Add("retry_credential_restore");
        return CredentialRetryPending;
    }
    public Task Logout() => Record("logout");
    public Task<Team[]> Teams() => Task.FromResult(TeamList);
    public Task SwitchTeam(string teamId) => Record($"switch {teamId}");
    public Task RefreshProfile() => Record("refresh");
    public Node[] Nodes() => _snapshot.Profile is null ? [] : NodeList;
    public Task SelectNode(string nodeId) => Record($"select {nodeId}");
    public Task Probe(ProbeMethod method, string[] nodeIds) => Record($"probe {method}");
    /// <summary>When set, <see cref="LocalProxies"/> answers only once this completes.</summary>
    public TaskCompletionSource? ProxiesGate { get; set; }
    public async Task<LocalProxy[]> LocalProxies()
    {
        if (ProxiesGate is { } gate) await gate.Task;
        return NodeList.Select(n => new LocalProxy(n.Id, "127.0.0.1", 7890, $"u8f2k-{n.Id}", "secret")).ToArray();
    }
    /// <summary>What <see cref="RoutedLocalProxy"/> answers (null: a core before 0.5.12).</summary>
    public bool RoutedSupported { get; set; } = true;
    public async Task<LocalProxy?> RoutedLocalProxy()
    {
        if (ProxiesGate is { } gate) await gate.Task;
        return RoutedSupported ? new LocalProxy("", "127.0.0.1", 7890, "u8f2k", "secret") : null;
    }
    public Task SetConnectionMode(ConnectionMode mode) => Record($"mode {mode}");
    public Task SetRoutingMode(RoutingMode mode) => Record($"routing {mode}");
    public Task PinIngress(string nodeId, string? endpointKey) => Record($"pin {nodeId} {endpointKey ?? "auto"}");
    public void DismissClearedIngressPins() => Calls.Add("dismiss_cleared_pins");
    public Task Connect() => Record("connect");
    public Task Disconnect() => Record("disconnect");
    public Task Retry() => Record("retry");
    public Task EnhancedTakeOver() => Record("enhanced_take_over");
    public Task ServiceInstall() => Record("service_install");
    public Task ServiceUninstall() => Record("service_uninstall");
    /// <summary>The system's answer to the pre-prompt check (null: what the snapshot says).</summary>
    public bool? InstalledOnSystem { get; set; }
    public Task<bool> RefreshServiceInstalled() => Task.FromResult(InstalledOnSystem ?? _snapshot.ServiceInstalled);
    public Task ShutdownAsync() => Task.CompletedTask;
    public void Dispose() { }
}

/// <summary>A MainViewModel over <see cref="ScriptedBackend"/>; create it on the UI thread.</summary>
sealed class Scripted
{
    /// <summary>Fixed "now" for scripted tests: 2026-09-29 14:30 in UTC+8.</summary>
    public static readonly DateTimeOffset Now = new(2026, 9, 29, 14, 30, 0, TimeSpan.FromHours(8));

    /// <param name="time">Defaults to a <see cref="ManualTime"/> at <see cref="Now"/> in UTC+8, so
    /// dates render the same on any machine's time zone.</param>
    public Scripted(ILocalizer? strings = null, TimeProvider? time = null)
    {
        Main = new MainViewModel(listener => Backend = new ScriptedBackend(listener),
            Services, new TestSettings(), strings ?? new KeyLocalizer(), Log, Prompts, time ?? new ManualTime(Now));
    }

    public ScriptedBackend Backend { get; private set; } = null!;
    public MainViewModel Main { get; }
    public TestServices Services { get; } = new();
    public TestPrompts Prompts { get; } = new();
    public TestLog Log { get; } = new();

    public Task PushAsync(ClientSnapshot snapshot) => Backend.PushAsync(Main, snapshot);

    /// <summary>Push <paramref name="snapshot"/> with <paramref name="change"/> applied (e.g. <see cref="Conn"/>).</summary>
    public Task PushAsync(ClientSnapshot snapshot, Func<ClientSnapshot, ClientSnapshot> change) => Backend.PushAsync(Main, change(snapshot));

    /// <summary>Signed in and ready (nodes and local proxies loaded).</summary>
    public async Task SignedInAsync(Func<ClientSnapshot, ClientSnapshot>? change = null, Func<ClientSnapshot, ClientSnapshot>? connection = null)
    {
        var snapshot = ScriptedBackend.SignedIn(change);
        await PushAsync(connection is null ? snapshot : connection(snapshot));
        await Wait.Until(() => Main.Nodes.Items.Count > 0 && (Main.Snapshot.Standard is not StandardState.Ready || Main.HasCurrentNodeProxy), "nodes loaded");
    }

    /// <summary>A snapshot change: the connection in <paramref name="phase"/>, in <paramref name="method"/>.</summary>
    public static Func<ClientSnapshot, ClientSnapshot> Conn(ConnectionPhase phase, ClientErrorInfo? reason = null, string? endpoint = null,
        string? previous = null, uint? latency = null, bool canTakeOver = false, bool suggestCompatible = false,
        ConnectionMode method = ConnectionMode.Enhanced, string? competitor = null, bool proxyWasForeign = false) =>
        s => s with
        {
            ConnectionMode = method,
            Connection = new ConnectionState(phase, reason, reason is not null, canTakeOver, suggestCompatible, competitor, proxyWasForeign,
                phase == ConnectionPhase.Off ? s.Connection.Detail : new ConnectionDetail(endpoint, null, previous, latency)),
        };
}
