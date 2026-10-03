using System.Globalization;
using System.Text.Json;
using System.Threading.Channels;
using PPVPN.Ffi;

namespace PPVPN.App.Core.Backend;

public enum FakeLoginOutcome { Approve, Deny, Expire, Never }

public enum FakeBrowserMode
{
    /// <summary>Open the verification page through <see cref="PlatformHooks.OpenUrl"/>.</summary>
    Open,
    /// <summary>Report the browser as opened without opening anything (screenshots, tests).</summary>
    Pretend,
    /// <summary>Report that the browser could not be opened.</summary>
    Fail,
}

/// <summary>What the backend answers for a team's proxy profile (see <see cref="ProfileStatus"/>).</summary>
public enum FakeProfileScenario
{
    /// <summary>A valid profile: <see cref="ProfileStatus.Ready"/>.</summary>
    Active,
    NoSubscription,
    SubscriptionExpired,
    TeamDisabled,
    /// <summary>The profile loads, then the core rejects it: <see cref="ProfileStatus.Invalid"/> (profile kept).</summary>
    Invalid,
    /// <summary>The profile is rejected and there is no previous one: Invalid without a profile.</summary>
    InvalidNoHistory,
}

public sealed record FakeOptions
{
    public bool IgnoreSavedCredential { get; init; }
    public FakeBrowserMode Browser { get; init; } = FakeBrowserMode.Open;
    public FakeLoginOutcome LoginOutcome { get; init; } = FakeLoginOutcome.Approve;
    public TimeSpan ApproveAfter { get; init; } = TimeSpan.FromSeconds(4);
    /// <summary>The first enhanced-mode connect fails with ConnectFailed (all routes timed out).</summary>
    public bool FailFirstConnect { get; init; }
    /// <summary>
    /// The first enhanced-mode connect fails its health check with another app named, like Surge's
    /// enhanced mode on macOS: ConnectHealthCheckFailed, <c>Competitor</c> "Surge", compatibility mode offered.
    /// </summary>
    public bool ConflictOnFirstConnect { get; init; }
    /// <summary>The first enhanced-mode connect finds the connection owned by another session (Contended, take-over offered).</summary>
    public bool OccupiedOnFirstConnect { get; init; }
    /// <summary>The first compatible-mode connect fails with SystemProxyFailed.</summary>
    public bool FailFirstSystemProxy { get; init; }
    /// <summary>The connection method at start (the crate remembers it).</summary>
    public ConnectionMode Method { get; init; } = ConnectionMode.Enhanced;
    /// <summary>The background profile refresh after sign-in fails (NetworkUnreachable, a transient last_error).</summary>
    public bool FailBackgroundRefresh { get; init; }
    /// <summary>Profile answer for the personal team (the one selected at sign-in). Other teams are Active.</summary>
    public FakeProfileScenario PersonalTeam { get; init; } = FakeProfileScenario.Active;
    /// <summary>Multiplies every simulated delay; tests use a small value.</summary>
    public double TimeScale { get; init; } = 1.0;
    /// <summary>Path <see cref="FakeClientBackend.PurchaseUrl"/> appends to the site root of the API base.</summary>
    public string PurchasePath { get; init; } = "/dashboard/products";
    /// <summary>Messages in the inbox after sign-in (the design's samples, cycled, newest first).</summary>
    public int MessageCount { get; init; } = 12;
    /// <summary>How many of the newest messages are unread. The newest (critical) one is <c>push</c>.</summary>
    public int UnreadMessages { get; init; } = 3;
    /// <summary>Delay after the profile loads before the first inbox poll (the crate polls every 60 s).</summary>
    public TimeSpan FirstPollAfter { get; init; } = TimeSpan.FromSeconds(2);
    /// <summary>Emit traffic samples once per second while signed in.</summary>
    public bool Traffic { get; init; } = true;
    /// <summary>
    /// Routing rule sets the core reports unavailable once the profile loads
    /// (<c>RuleSetsUnavailable</c>, the rules notice); they load after <see cref="RuleSetsLoadAfter"/>.
    /// </summary>
    public IReadOnlyList<string> UnavailableRuleSets { get; init; } = [];
    public TimeSpan RuleSetsLoadAfter { get; init; } = TimeSpan.FromSeconds(20);
    /// <summary>When set, restoring the saved session waits for it (tests hold the app in Restoring).</summary>
    public Task? RestoreGate { get; init; }
    /// <summary>When set, the browser's answer to a device login waits for it as well as <see cref="ApproveAfter"/>.</summary>
    public Task? ApproveGate { get; init; }
}

/// <summary>
/// In-process stand-in for ppvpn-client with realistic data and timing, including the system
/// connection in both modes (<see cref="Connect"/>, <see cref="SetConnectionMode"/>). Like the Rust crate, every listener callback is delivered on a
/// background thread (a single ordered pump), never on the caller's thread, and every callback
/// an action causes is delivered before the action completes. Platform hooks are real: the
/// saved "session" goes through the app's credential store, the service install/uninstall
/// through its prompts.
/// </summary>
public sealed class FakeClientBackend : IClientBackend
{
    public const string PersonalTeamId = "team-personal";
    public const string AccountEmail = "alice@example.com";
    /// <summary>Per-device user-name prefix of the local proxy (<c>u8f2k-&lt;nodeId&gt;</c>).</summary>
    public const string ProxyUserPrefix = "u8f2k";
    public const string ProxyPassword = "Xk29aQ7mTe4w";
    public const ushort ProxyPort = 7890;
    static readonly TimeSpan ProbeDeadline = TimeSpan.FromSeconds(3);
    /// <summary>Credential-store re-read after a failed restore (the crate: 30 s, later 5 min).</summary>
    static readonly TimeSpan CredentialRetryInterval = TimeSpan.FromSeconds(30);

    readonly ClientConfig _config;
    readonly PlatformHooks _platform;
    readonly ClientListener _listener;
    readonly FakeOptions _options;
    readonly object _gate = new();
    readonly Channel<Action> _events = Channel.CreateUnbounded<Action>(new UnboundedChannelOptions { SingleReader = true });
    readonly CancellationTokenSource _life = new();
    readonly FakeLog _log;
    readonly Node[] _nodes = SampleNodes();
    Team[] _teams =
    [
        new(PersonalTeamId, "个人", true, true),
        new("team-acme", "Acme Studio", false, true),
        // Disabled or dissolved: listed, but switching to it is refused (TeamDisabled).
        new("team-lumen", "Lumen Labs", false, false),
    ];
    readonly Dictionary<string, FakeProfileScenario> _scenarios = [];
    /// <summary>Inbox, newest first.</summary>
    readonly List<InboxMessage> _inbox = [];
    /// <summary>Pushes "the push agent" showed, by push id.</summary>
    readonly Dictionary<ulong, PushMessage> _shownPushes = [];
    ulong _nextMessageId = 5000;

    ClientSnapshot _snapshot;
    CancellationTokenSource? _login;
    CancellationTokenSource? _connect;
    CancellationTokenSource? _session;
    CancellationTokenSource? _traffic;
    int _revision = 41;
    int _shutdown;
    bool _connectFailed, _occupied, _proxyFailed, _conflicted;
    readonly SemaphoreSlim _credentialWake = new(0);
    volatile bool _credentialRetryPending;

    public FakeClientBackend(ClientConfig config, PlatformHooks platform, ClientListener listener, FakeOptions? options = null)
    {
        _config = config;
        _platform = platform;
        _listener = listener;
        _options = options ?? new FakeOptions();
        _scenarios[PersonalTeamId] = _options.PersonalTeam;
        _log = new FakeLog(config.LogDir);
        _snapshot = ClientSnapshots.Initial with { SelectedNodeId = LoadSelection() };
        _snapshot = _snapshot with { ConnectionMode = _options.Method };
        _ = Task.Run(PumpEventsAsync);
        _ = Task.Run(RestoreAsync);
        _log.Info($"client started (platform={config.Platform}, version={config.AppVersion}, api={config.ApiBase})");
    }

    /// <summary>A factory for <c>MainViewModel</c>'s <c>createBackend</c> argument.</summary>
    public static Func<ClientListener, IClientBackend> Factory(ClientConfig config, PlatformHooks platform, FakeOptions? options = null) =>
        listener => new FakeClientBackend(config, platform, listener, options);

    /// <summary>Disable or re-enable a team (as its owner or the operator would).</summary>
    public void SetTeamActive(string teamId, bool active)
    {
        lock (_gate) _teams = _teams.Select(t => t.Id == teamId ? t with { Active = active } : t).ToArray();
    }

    /// <summary>Change what the backend answers for a team; applies on the next profile fetch.</summary>
    public void SetScenario(string teamId, FakeProfileScenario scenario)
    {
        lock (_gate) _scenarios[teamId] = scenario;
    }

    /// <summary>Another session takes the enhanced-mode connection (phase Contended, take-over offered).</summary>
    public void SimulateOccupied()
    {
        StopConnectTimers();
        UpdateConnection(c => c with
        {
            Phase = ConnectionPhase.Contended,
            Reason = new ClientErrorInfo(ErrorCode.ServiceBusy, "CONNECTION_OWNED_BY_ANOTHER_SESSION"),
            Retryable = true,
            CanTakeOver = true,
            Detail = StandardPath(),
        });
    }

    public ClientSnapshot Snapshot()
    {
        lock (_gate) return _snapshot;
    }


    public string LogDir() => _config.LogDir;

    public string PurchaseUrl() => SiteUrl(_options.PurchasePath);

    public string RoutingRulesUrl() => SiteUrl("/dashboard/routing-rules");

    public void NetworkChanged() { }

    string SiteUrl(string path) =>
        Uri.TryCreate(_config.ApiBase.Trim(), UriKind.Absolute, out var api)
            ? new UriBuilder(api) { Path = path, Query = "", Fragment = "" }.Uri.ToString()
            : _config.ApiBase.Trim().TrimEnd('/') + path;

    // --- public actions ----------------------------------------------------
    // Like the crate, every snapshot/probe callback an action causes has been handed to the
    // listener before the action completes or throws (the crate calls the listener
    // synchronously); the fake's callbacks go through a pump, so each action flushes it.

    public Task<DeviceCode> AuthStart() => Delivered(AuthStartCore);
    public Task Logout() => Delivered(LogoutCore);
    public Task<Team[]> Teams() => Delivered(TeamsCore);
    public Task SwitchTeam(string teamId) => Delivered(() => SwitchTeamCore(teamId));
    public Task RefreshProfile() => Delivered(RefreshProfileCore);
    public Task SelectNode(string nodeId) => Delivered(() => SelectNodeCore(nodeId));
    public Task Probe(ProbeMethod method, string[] nodeIds) => Delivered(() => ProbeCore(method, nodeIds));
    public Task<LocalProxy[]> LocalProxies() => Delivered(LocalProxiesCore);
    public Task<LocalProxy?> RoutedLocalProxy() => Delivered(RoutedLocalProxyCore);
    public Task SetConnectionMode(ConnectionMode method) => Delivered(() => SetConnectionModeCore(method));
    public Task SetRoutingMode(RoutingMode mode) => Delivered(() =>
    {
        _log.Info($"routing mode: {mode}");
        Update(s => s with { RoutingMode = mode });
        return Task.CompletedTask;
    });
    public Task PinIngress(string nodeId, string? endpointKey) => Delivered(() =>
    {
        _log.Info($"pin ingress: {nodeId} -> {endpointKey ?? "auto"}");
        Update(s => s with
        {
            IngressPins = endpointKey is null
                ? s.IngressPins.Where(p => p.NodeId != nodeId).ToArray()
                : s.IngressPins.Where(p => p.NodeId != nodeId).Append(new IngressPin(nodeId, endpointKey)).ToArray(),
        });
        return Task.CompletedTask;
    });
    public void DismissClearedIngressPins() => Update(s => s with { ClearedIngressPins = [] });
    public Task Connect() => Delivered(() => ConnectCore(takeOver: false, retry: false));
    public Task Retry() => Delivered(() => ConnectCore(takeOver: false, retry: true));
    public Task EnhancedTakeOver() => Delivered(() => ConnectCore(takeOver: true, retry: true));
    public Task Disconnect() => Delivered(DisconnectCore);
    public Task ServiceInstall() => Delivered(ServiceInstallCore);
    public Task ServiceUninstall() => Delivered(ServiceUninstallCore);
    public Task<bool> RefreshServiceInstalled()
    {
        var installed = _platform.PrivilegedServiceInstalled();
        Update(s => s with { ServiceInstalled = installed });
        return Task.FromResult(installed);
    }
    public Task<InboxPage> Notifications(uint page, uint pageSize) => Delivered(() => NotificationsCore(page, pageSize));
    public Task MarkNotificationRead(ulong id) => Delivered(() => SetReadCore(id, true));
    public Task MarkNotificationUnread(ulong id) => Delivered(() => SetReadCore(id, false));
    public Task MarkAllNotificationsRead() => Delivered(MarkAllNotificationsReadCore);

    public PushMessage? ShownPush(ulong pushId)
    {
        lock (_gate) return _shownPushes.GetValueOrDefault(pushId);
    }

    /// <summary>
    /// What the push agent does after the OS showed <paramref name="push"/>: record it, so a click
    /// on its notification (<c>id=&lt;push.Id&gt;</c>) resolves through <see cref="ShownPush"/>.
    /// </summary>
    public void RecordShownPush(PushMessage push)
    {
        lock (_gate) _shownPushes[push.Id] = push;
    }

    async Task<T> Delivered<T>(Func<Task<T>> action)
    {
        try { return await action().ConfigureAwait(false); }
        finally { await Flush().ConfigureAwait(false); }
    }

    async Task Delivered(Func<Task> action)
    {
        try { await action().ConfigureAwait(false); }
        finally { await Flush().ConfigureAwait(false); }
    }

    /// <summary>Completes once every callback queued so far has been delivered.</summary>
    Task Flush() => EmitAsync(() => { });

    // --- notifications ---------------------------------------------------

    /// <summary>
    /// A new backend message arrives. The unread count updates as the crate's next badge
    /// poll would see it; OS notifications are the push agent's job, not the app's.
    /// </summary>
    public InboxMessage PushMessage(string title, string content, bool push = true,
        MessageSeverity severity = MessageSeverity.Normal, string? deepLink = null,
        string kind = "system", string eventKey = "", MessageCategory category = MessageCategory.Announcement)
    {
        InboxMessage message;
        lock (_gate)
        {
            message = new InboxMessage(++_nextMessageId, title, content, kind, eventKey, category, severity, deepLink, push, false,
                DateTimeOffset.UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ", CultureInfo.InvariantCulture));
            _inbox.Insert(0, message);
        }
        Poll();
        return message;
    }

    async Task<InboxPage> NotificationsCore(uint page, uint pageSize)
    {
        RequireSignedIn();
        await Delay(150).ConfigureAwait(false);
        var size = (int)Math.Clamp(pageSize, 1u, 100u);
        var skip = (int)Math.Max(0, (long)Math.Max(page, 1u) - 1) * size;
        InboxPage result;
        lock (_gate) result = new InboxPage(_inbox.Skip(skip).Take(size).ToArray(), (uint)_inbox.Count);
        PublishUnread();
        return result;
    }

    async Task SetReadCore(ulong id, bool read)
    {
        RequireSignedIn();
        await Delay(80).ConfigureAwait(false);
        lock (_gate)
        {
            var index = _inbox.FindIndex(m => m.Id == id);
            if (index >= 0) _inbox[index] = _inbox[index] with { Read = read };
        }
        _log.Info($"message {id} marked {(read ? "read" : "unread")}");
        PublishUnread();
    }

    async Task MarkAllNotificationsReadCore()
    {
        RequireSignedIn();
        await Delay(120).ConfigureAwait(false);
        lock (_gate)
            for (var i = 0; i < _inbox.Count; i++) _inbox[i] = _inbox[i] with { Read = true };
        _log.Info("all messages marked read");
        PublishUnread();
    }

    /// <summary>What one crate badge poll does: refresh the unread count.</summary>
    void Poll()
    {
        if (Snapshot().Auth is not AuthState.SignedIn) return;
        PublishUnread();
    }

    /// <summary>
    /// Counts inside <see cref="Update"/>'s lock: a count taken before it could land after a
    /// newer one (the first badge poll runs on the thread pool) and bring back a read message.
    /// </summary>
    void PublishUnread() =>
        Update(s => s.Auth is AuthState.SignedIn ? s with { UnreadNotifications = UnreadCount() } : s);

    /// <summary>Unread messages; call with <c>_gate</c> held.</summary>
    uint UnreadCount() => (uint)_inbox.Count(m => !m.Read);

    /// <summary>The design's sample messages (kind / event key as the backend sends them).</summary>
    static readonly (string Kind, string Event, MessageCategory Category, MessageSeverity Severity, bool Link, string Title, string Body)[] MessageTemplates =
    [
        ("subscription", "subscription.expire_reminder_3d", MessageCategory.SubscriptionExpiring, MessageSeverity.Critical, true, "订阅将在 3 天后到期", "你的 Pro 年付套餐将于 2026年10月2日 到期，到期后将无法连接任何节点。续费后立即生效，剩余时长自动顺延，系统代理与增强模式的设置会保留。"),
        ("proxy", "proxy.chain_unhealthy", MessageCategory.Route, MessageSeverity.Important, false, "香港 01 线路 HKG-A 不稳定", "HKG-A 在 14:05–14:20 出现间歇性丢包，客户端已自动切换到 HKG-B。如仍有问题，可在节点页重新测速或切换节点。"),
        ("invoice", "invoice.generated", MessageCategory.Billing, MessageSeverity.Normal, true, "9 月账单已生成", "本期账单金额 ¥128.00，将于 10月1日 从钱包余额自动扣除。"),
        ("broadcast", "", MessageCategory.Announcement, MessageSeverity.Unspecified, true, "东京节点维护通知", "为提升线路质量，东京 01 将于 10月1日 02:00–04:00（UTC+8）进行维护，期间该节点不可用。建议提前切换到大阪 01 或首尔 01，维护完成后无需任何操作。"),
        ("order", "order.paid", MessageCategory.Order, MessageSeverity.Normal, true, "订单 #20260928-0417 已支付", "Pro 年付套餐 · ¥1,188.00。发票可在控制台「账单」中申请。"),
        ("proxy", "proxy.node_unavailable", MessageCategory.Route, MessageSeverity.Important, false, "法兰克福 01 暂时不可用", "上游运营商故障，预计 2 小时内恢复。恢复前测速会显示超时。"),
        ("subscription", "subscription.expired", MessageCategory.SubscriptionExpired, MessageSeverity.Critical, true, "团队订阅已过期", "Lumen Labs 的团队订阅已于 9月1日 到期，团队成员将无法连接节点。请联系团队管理员续费。"),
        ("broadcast", "broadcast:release-1.4.0", MessageCategory.Announcement, MessageSeverity.Unspecified, true, "客户端 1.4.0 已发布", "新增消息中心与实时流量显示，修复了若干已知问题。"),
        ("billing", "wallet.low_balance", MessageCategory.Billing, MessageSeverity.Important, true, "钱包余额不足", "当前余额 ¥12.50，不足以支付下个周期的费用。请在到期前充值，以免服务中断。"),
        ("order", "order.refunded", MessageCategory.Order, MessageSeverity.Unspecified, false, "退款已完成", "订单 #20260903-1122 的退款 ¥49.00 已原路退回。"),
    ];

    /// <summary>Sample message ids: the newest is <see cref="NewestSampleId"/>, then counting down.</summary>
    public const ulong NewestSampleId = 1000;

    void SeedMessages()
    {
        lock (_gate)
        {
            if (_inbox.Count > 0) return;
            var now = DateTimeOffset.UtcNow;
            var age = 0.0;
            int[] fixedAges = [3, 18, 52, 131, 262, 1450, 1690];
            for (var i = 0; i < _options.MessageCount; i++)
            {
                age = i < fixedAges.Length ? fixedAges[i] : age + 700 + (i * 337 % 1500);
                var t = MessageTemplates[i % MessageTemplates.Length];
                var id = NewestSampleId - (ulong)i;
                var link = t.Link ? $"https://www.peakpassvpn.com/dashboard/messages/{id}" : null;
                _inbox.Add(new InboxMessage(id, t.Title, t.Body, t.Kind, t.Event, t.Category, t.Severity, link,
                    Push: i == 0, Read: i >= _options.UnreadMessages,
                    now.AddMinutes(-age).ToString("yyyy-MM-ddTHH:mm:ssZ", CultureInfo.InvariantCulture)));
            }
        }
    }

    // --- auth ------------------------------------------------------------

    async Task<DeviceCode> AuthStartCore()
    {
        await Delay(350).ConfigureAwait(false);
        var previous = Interlocked.Exchange(ref _login, new CancellationTokenSource());
        previous?.Cancel();
        var token = _login!.Token;

        var userCode = $"{RandomLetters(4)}-{RandomLetters(4)}";
        var url = $"https://www.peakpassvpn.com/device?user_code={userCode}";
        var opened = _options.Browser switch
        {
            FakeBrowserMode.Open => _platform.OpenUrl(url),
            FakeBrowserMode.Pretend => true,
            _ => false,
        };
        var code = new DeviceCode(userCode, url, 600, opened);
        _log.Info($"device login started, browser_opened={opened}");
        Update(s => s with { Auth = new AuthState.AwaitingBrowser(code), LastError = null });

        _ = Task.Run(async () =>
        {
            try
            {
                await Delay(_options.ApproveAfter, token).ConfigureAwait(false);
                if (_options.ApproveGate is { } gate) await gate.WaitAsync(token).ConfigureAwait(false);
                switch (_options.LoginOutcome)
                {
                    case FakeLoginOutcome.Approve:
                        await SignInAsync(restored: false, token).ConfigureAwait(false);
                        break;
                    case FakeLoginOutcome.Deny:
                        _log.Warn("device login denied in the browser");
                        Update(s => s with { Auth = new AuthState.SignedOut(), LastError = new(ErrorCode.AuthDenied, "access_denied") });
                        break;
                    case FakeLoginOutcome.Expire:
                        _log.Warn("device code expired");
                        Update(s => s with { Auth = new AuthState.SignedOut(), LastError = new(ErrorCode.AuthExpired, "expired_token") });
                        break;
                }
            }
            catch (OperationCanceledException) { }
        }, CancellationToken.None);

        return code;
    }

    public void AuthCancel()
    {
        Interlocked.Exchange(ref _login, null)?.Cancel();
        _log.Info("device login cancelled");
        Update(s => s.Auth is AuthState.AwaitingBrowser ? s with { Auth = new AuthState.SignedOut() } : s);
    }

    async Task LogoutCore()
    {
        Interlocked.Exchange(ref _session, null)?.Cancel();
        StopConnectTimers();
        StopTraffic();
        await Delay(250).ConfigureAwait(false);
        Update(s => ClientSnapshots.Initial with
        {
            Auth = new AuthState.SignedOut(),
            ConnectionMode = s.ConnectionMode,
            ServiceInstalled = s.ServiceInstalled,
        });
        SaveSelection(null);
        _log.Info("signed out");
        try
        {
            _platform.CredentialDelete();
        }
        catch (PlatformException error)
        {
            throw new ClientException.Failed(StoreErrorCode(error), error.Message);
        }
    }

    // --- account ---------------------------------------------------------

    async Task<Team[]> TeamsCore()
    {
        RequireSignedIn();
        await Delay(200).ConfigureAwait(false);
        lock (_gate) return [.. _teams];
    }

    async Task SwitchTeamCore(string teamId)
    {
        RequireSignedIn();
        Team? team;
        lock (_gate) team = _teams.FirstOrDefault(t => t.Id == teamId);
        if (team is null)
            throw Report(new ClientException.Failed(ErrorCode.RequestRejected, $"POST /api/v1/me/team -> HTTP 404 (unknown team {teamId})"));
        if (!team.Active)
        {
            // Like the crate: a refused switch is a one-off error; the current team stays.
            await Delay(300).ConfigureAwait(false);
            _log.Warn($"switch to {team.Name} refused: team disabled");
            throw Report(new ClientException.Failed(ErrorCode.TeamDisabled, "POST /api/v1/me/team -> HTTP 403 403012"));
        }
        _log.Info($"switching team to {team.Name}");
        StopConnectTimers();
        Update(s => s with
        {
            Team = team,
            Profile = null,
            ProfileStatus = new ProfileStatus.Loading(),
            Standard = new StandardState.Stopped(),
            Connection = ConnectionStates.Off,
            LastError = null,
        });
        await Delay(600).ConfigureAwait(false);
        await FetchProfileAsync(userAction: true, CancellationToken.None).ConfigureAwait(false);
    }

    // --- profile & nodes -------------------------------------------------

    async Task RefreshProfileCore()
    {
        RequireSignedIn();
        await Delay(400).ConfigureAwait(false);
        await FetchProfileAsync(userAction: true, CancellationToken.None).ConfigureAwait(false);
    }

    public Node[] Nodes()
    {
        lock (_gate) return _snapshot.Profile is null ? [] : [.. _nodes];
    }

    async Task SelectNodeCore(string nodeId)
    {
        RequireSignedIn();
        var node = _nodes.FirstOrDefault(n => n.Id == nodeId)
            ?? throw Report(new ClientException.Failed(ErrorCode.NodeNotFound, nodeId));
        await Delay(50).ConfigureAwait(false);
        SaveSelection(nodeId);
        _log.Info($"selected node {Describe(node)}");
        var wasOn = Snapshot().Connection.Phase == ConnectionPhase.On;
        // The standard core follows the selection (snapshot.connection).
        Update(s => s with
        {
            SelectedNodeId = nodeId,
            LastError = null,
            // Off: the detail is the standard core's path to the current node.
            Connection = s.Connection.Phase == ConnectionPhase.Off && s.Standard is StandardState.Ready
                ? s.Connection with { Detail = ConnectionTo(node) }
                : s.Connection,
        });
        if (!wasOn) return;

        // Applied live: the connection moves to the new node.
        var endpoint = node.Replicas[0].EndpointKey;
        UpdateConnection(c => c with
        {
            Phase = ConnectionPhase.Reconnecting,
            Detail = new ConnectionDetail(endpoint, Label(endpoint), c.Detail.EndpointKey, null),
        });
        await Delay(1100).ConfigureAwait(false);
        UpdateConnection(c => c.Phase == ConnectionPhase.Reconnecting
            ? c with { Phase = ConnectionPhase.On, Detail = c.Detail with { PreviousEndpointKey = null, LatencyMs = RealLatency(node.Id) } }
            : c);
    }

    async Task ProbeCore(ProbeMethod method, string[] nodeIds)
    {
        RequireSignedIn();
        if (Snapshot().Standard is not StandardState.Ready)
            throw new ClientException.StandardNotReady();

        var ids = nodeIds.Length == 0 ? _nodes.Select(n => n.Id).ToArray() : nodeIds.Distinct().ToArray();
        _log.Info($"speed test ({method}) for {ids.Length} node(s)");
        var tasks = ids.Select((id, k) => Task.Run(async () =>
        {
            var node = _nodes.FirstOrDefault(n => n.Id == id);
            var sample = SampleLatency(id);
            ProbeResult result;
            if (node is null)
            {
                result = new(id, method, false, null, null, new(ErrorCode.NodeNotFound, "unknown node"));
            }
            else if (sample == TimeoutLatency)
            {
                await Delay(ProbeDeadline).ConfigureAwait(false);
                result = new(id, method, false, null, null, new(ErrorCode.Timeout, $"no answer within {ProbeDeadline.TotalSeconds:0}s"));
            }
            else if (sample == FailedLatency)
            {
                await Delay(300 + k * 40).ConfigureAwait(false);
                result = new(id, method, false, null, null, new(ErrorCode.Unreachable, "connection refused"));
            }
            else
            {
                var factor = method switch { ProbeMethod.Icmp => 0.85, ProbeMethod.Connect => 1.7, _ => 1.0 };
                var latency = (uint)Math.Max(1, Math.Round(sample!.Value * factor + Random.Shared.Next(-5, 6)));
                await Delay(TimeSpan.FromMilliseconds(350 + k * 60 + Random.Shared.Next(0, 300))).ConfigureAwait(false);
                result = new(id, method, true, latency, method == ProbeMethod.Connect ? null : node.Replicas[0].EndpointKey, null);
            }
            _log.Info(result.Success
                ? $"speed test {Describe(node)}: {result.LatencyMs} ms"
                : $"speed test {Describe(node)}: {result.Error?.Code}");
            // Like the crate, every result has been handed to the listener before Probe returns.
            await EmitAsync(() => _listener.OnProbeResult(result)).ConfigureAwait(false);
        })).ToArray();
        await Task.WhenAll(tasks).ConfigureAwait(false);
    }

    async Task<LocalProxy?> RoutedLocalProxyCore()
    {
        RequireSignedIn();
        if (Snapshot().Standard is not StandardState.Ready)
            throw new ClientException.StandardNotReady();
        await Delay(40).ConfigureAwait(false);
        // The bare prefix: Profile rules, then the selected node.
        return new LocalProxy("", "127.0.0.1", ProxyPort, ProxyUserPrefix, ProxyPassword);
    }

    async Task<LocalProxy[]> LocalProxiesCore()
    {
        RequireSignedIn();
        if (Snapshot().Standard is not StandardState.Ready)
            throw new ClientException.StandardNotReady();
        await Delay(80).ConfigureAwait(false);
        // One shared port; the user name picks the node, the password is per device.
        return _nodes.Select(n => new LocalProxy(n.Id, "127.0.0.1", ProxyPort, $"{ProxyUserPrefix}-{n.Id}", ProxyPassword)).ToArray();
    }

    // --- connection -----------------------------------------------------------

    async Task SetConnectionModeCore(ConnectionMode method)
    {
        var (current, phase) = (Snapshot().ConnectionMode, Snapshot().Connection.Phase);
        if (current == method) return;
        _log.Info($"connection method: {method}");
        var connected = phase is ConnectionPhase.On or ConnectionPhase.Connecting or ConnectionPhase.Reconnecting or ConnectionPhase.Preparing;
        if (connected) await DisconnectCore().ConfigureAwait(false);
        Update(s => s with { ConnectionMode = method, Connection = ConnectionStates.Off with { Detail = StandardPath() } });
        // Switching while connected reconnects in the new method.
        if (connected) await ConnectCore(takeOver: false, retry: false).ConfigureAwait(false);
    }

    async Task ConnectCore(bool takeOver, bool retry)
    {
        RequireSignedIn();
        if (Snapshot().Profile is null)
            throw Report(new ClientException.Failed(ErrorCode.NodeNotFound, "no profile"));
        var node = CurrentNode()
            ?? throw Report(new ClientException.Failed(ErrorCode.NodeNotFound, "no node selected"));
        var (method, connection) = (Snapshot().ConnectionMode, Snapshot().Connection);
        if (!takeOver && !retry && connection.Phase is ConnectionPhase.On or ConnectionPhase.Connecting or ConnectionPhase.Preparing or ConnectionPhase.WaitingPermission) return;

        var cts = new CancellationTokenSource();
        Interlocked.Exchange(ref _connect, cts)?.Cancel();
        var token = cts.Token;
        var endpoint = node.Replicas[0].EndpointKey;
        try
        {
            if (method == ConnectionMode.Compatible)
            {
                _log.Info($"compatible: setting the system proxy for {Describe(node)}");
                UpdateConnection(_ => ConnectionStates.Off with { Phase = ConnectionPhase.Connecting, Detail = new ConnectionDetail(endpoint, Label(endpoint), null, null) });
                await Delay(700, token).ConfigureAwait(false);
                if (_options.FailFirstSystemProxy && !_proxyFailed)
                {
                    _proxyFailed = true;
                    var error = new ClientErrorInfo(ErrorCode.SystemProxyFailed, "proxy settings are managed by another program");
                    _log.Error($"system proxy failed: {error.Detail}");
                    UpdateConnection(c => c with { Phase = ConnectionPhase.Error, Reason = error, Retryable = true, Detail = StandardPath() });
                    throw Report(new ClientException.Failed(error.Code, error.Detail));
                }
                _log.Info("compatible: on");
                UpdateConnection(c => c with { Phase = ConnectionPhase.On, Detail = c.Detail with { LatencyMs = RealLatency(node.Id) } });
                return;
            }

            // Like the crate, connecting re-checks the service, so the snapshot follows the system.
            var serviceInstalled = _platform.PrivilegedServiceInstalled();
            Update(s => s with { ServiceInstalled = serviceInstalled });
            if (!serviceInstalled)
            {
                _log.Info("enhanced: installing the privileged service");
                UpdateConnection(_ => ConnectionStates.Off with { Phase = ConnectionPhase.WaitingPermission, Detail = StandardPath() });
                ClientErrorInfo? denied = null;
                try
                {
                    await Task.Run(_platform.InstallPrivilegedService, token).ConfigureAwait(false);
                }
                catch (PlatformException.Cancelled)
                {
                    denied = new ClientErrorInfo(ErrorCode.ServiceInstallCancelled, "admin prompt dismissed");
                }
                catch (PlatformException error)
                {
                    denied = new ClientErrorInfo(ErrorCode.ServiceInstallFailed, error.Message);
                }
                if (denied is not null)
                {
                    // Like the crate: a denied prompt is a retryable Error, and compatibility mode would work.
                    UpdateConnection(c => c with { Phase = ConnectionPhase.Error, Reason = denied, Retryable = true, SuggestCompatible = true });
                    throw Report(new ClientException.Failed(denied.Code, denied.Detail));
                }
                Update(s => s with { ServiceInstalled = true });
            }
            _log.Info("enhanced: preparing");
            UpdateConnection(_ => ConnectionStates.Off with { Phase = ConnectionPhase.Preparing, Detail = StandardPath() });
            await Delay(700, token).ConfigureAwait(false);

            _log.Info($"enhanced: connecting via {Describe(node)} ({endpoint})");
            UpdateConnection(c => c with { Phase = ConnectionPhase.Connecting, Detail = new ConnectionDetail(endpoint, Label(endpoint), null, null) });
            await Delay(1300, token).ConfigureAwait(false);

            if (!takeOver && _options.OccupiedOnFirstConnect && !_occupied)
            {
                _occupied = true;
                _log.Warn("enhanced: connection owned by another session");
                SimulateOccupied();
                return;
            }
            if (_options.ConflictOnFirstConnect && !_conflicted)
            {
                _conflicted = true;
                var reason = new ClientErrorInfo(ErrorCode.ConnectHealthCheckFailed, "HEALTH_CAPTURE_PATH_FAILED");
                _log.Error("enhanced: health check failed (HEALTH_CAPTURE_PATH_FAILED)");
                _log.Info("conflict detection: competitors [\"Surge\"], fake-ip [\"utun5 (198.18.0.1)\"], route Some(\"utun5\")");
                UpdateConnection(c => c with
                {
                    Phase = ConnectionPhase.Error, Reason = reason, Retryable = true, SuggestCompatible = true,
                    Competitor = "Surge", Detail = StandardPath(),
                });
                return;
            }
            if (_options.FailFirstConnect && !_connectFailed)
            {
                _connectFailed = true;
                var reason = new ClientErrorInfo(ErrorCode.ConnectFailed, "HEALTH_ENTRANCE_FAILED: all routes timed out");
                _log.Error("enhanced: health check through the node failed (HEALTH_ENTRANCE_FAILED)");
                UpdateConnection(c => c with { Phase = ConnectionPhase.Error, Reason = reason, Retryable = true, Detail = StandardPath() });
                return;
            }

            _log.Info($"enhanced: on via {Describe(node)} ({endpoint})");
            UpdateConnection(c => c with { Phase = ConnectionPhase.On, Reason = null, Retryable = false, CanTakeOver = false, Detail = c.Detail with { LatencyMs = RealLatency(node.Id) } });
        }
        catch (OperationCanceledException) { }
    }

    async Task DisconnectCore()
    {
        if (Snapshot().Connection.Phase == ConnectionPhase.Off) return;
        StopConnectTimers();
        _log.Info("disconnecting");
        UpdateConnection(c => c with { Phase = ConnectionPhase.Disconnecting });
        await Delay(900).ConfigureAwait(false);
        _log.Info("disconnected");
        UpdateConnection(_ => ConnectionStates.Off with { Detail = StandardPath() });
    }

    void UpdateConnection(Func<ConnectionState, ConnectionState> change) =>
        Update(s => s with { Connection = change(s.Connection) });

    /// <summary>The standard core's path to the current node (the detail while the connection is off).</summary>
    ConnectionDetail StandardPath() =>
        Snapshot().Standard is StandardState.Ready && CurrentNode() is { } node ? ConnectionTo(node) : NoConnection;

    string? Label(string endpointKey) => _nodes.SelectMany(n => n.Replicas).FirstOrDefault(r => r.EndpointKey == endpointKey)?.Label;

    async Task ServiceInstallCore()
    {
        _log.Info("installing the privileged service");
        try
        {
            await Task.Run(_platform.InstallPrivilegedService).ConfigureAwait(false);
        }
        catch (PlatformException.Cancelled)
        {
            // Like the crate: Cancelled, and no state changes.
            throw new ClientException.Cancelled();
        }
        catch (PlatformException error)
        {
            _log.Error($"service install failed: {error.Message}");
            throw Report(new ClientException.Failed(ErrorCode.ServiceInstallFailed, error.Message));
        }
        var installed = _platform.PrivilegedServiceInstalled();
        Update(s => s with { ServiceInstalled = installed, LastError = null });
    }

    async Task ServiceUninstallCore()
    {
        if (Snapshot().ConnectionMode == ConnectionMode.Enhanced) await DisconnectCore().ConfigureAwait(false);
        _log.Info("uninstalling the privileged service");
        try
        {
            await Task.Run(_platform.UninstallPrivilegedService).ConfigureAwait(false);
        }
        catch (PlatformException.Cancelled)
        {
            throw Report(new ClientException.Failed(ErrorCode.ServiceUninstallCancelled, "admin prompt dismissed"));
        }
        catch (PlatformException error)
        {
            _log.Error($"service uninstall failed: {error.Message}");
            throw Report(new ClientException.Failed(ErrorCode.ServiceInstallFailed, error.Message));
        }
        var installed = _platform.PrivilegedServiceInstalled();
        Update(s => s with { ServiceInstalled = installed });
    }

    public Task ShutdownAsync() => Task.Run(Shutdown);

    void Shutdown()
    {
        if (Interlocked.Exchange(ref _shutdown, 1) != 0) return;
        _log.Info("client shutting down");
        StopConnectTimers();
        StopTraffic();
        // The real call blocks while it releases the session and stops the cores.
        Thread.Sleep(TimeSpan.FromMilliseconds(300 * _options.TimeScale));
        Interlocked.Exchange(ref _login, null)?.Cancel();
        _life.Cancel();
        _events.Writer.TryComplete();
    }

    public void Dispose() => Shutdown();

    // --- internals -------------------------------------------------------

    async Task RestoreAsync()
    {
        if (_options.RestoreGate is { } gate) await gate.ConfigureAwait(false);
        await Delay(600).ConfigureAwait(false);
        byte[]? blob;
        try
        {
            blob = _options.IgnoreSavedCredential ? null : _platform.CredentialLoad();
        }
        catch (PlatformException error)
        {
            // Like the crate: keep the saved login and read the store again in the background.
            _log.Error($"credential load failed: {error.Message}");
            _credentialRetryPending = true;
            Update(s => s with { Auth = new AuthState.SignedOut(), LastError = new(StoreErrorCode(error), error.Message) });
            _ = Task.Run(CredentialRetryAsync);
            return;
        }
        if (blob is null)
        {
            _log.Info("no saved session");
            Update(s => s with { Auth = new AuthState.SignedOut() });
            return;
        }
        _log.Info($"restoring saved session ({blob.Length} bytes)");
        try
        {
            await SignInAsync(restored: true, _life.Token).ConfigureAwait(false);
        }
        catch (OperationCanceledException) { }
    }

    static ErrorCode StoreErrorCode(PlatformException error) =>
        error is PlatformException.Locked ? ErrorCode.CredentialStoreLocked : ErrorCode.CredentialStoreFailed;

    static bool IsStoreError(ClientErrorInfo? error) =>
        error?.Code is ErrorCode.CredentialStoreFailed or ErrorCode.CredentialStoreLocked;

    /// <summary>Re-reads the credential store until readable, or until a sign-in supersedes it.</summary>
    async Task CredentialRetryAsync()
    {
        try
        {
            while (true)
            {
                await _credentialWake.WaitAsync(CredentialRetryInterval * _options.TimeScale, _life.Token).ConfigureAwait(false);
                if (Snapshot().Auth is not AuthState.SignedOut) return;
                byte[]? blob;
                try
                {
                    blob = _platform.CredentialLoad();
                }
                catch (PlatformException error)
                {
                    _log.Info($"credential store still unavailable: {error.Message}");
                    Update(s => IsStoreError(s.LastError) ? s with { LastError = new(StoreErrorCode(error), error.Message) } : s);
                    continue;
                }
                _credentialRetryPending = false;
                Update(s => IsStoreError(s.LastError) ? s with { LastError = null } : s);
                if (blob is null) return;
                _log.Info("credential store readable again; restoring");
                await SignInAsync(restored: true, _life.Token).ConfigureAwait(false);
                return;
            }
        }
        catch (OperationCanceledException) { }
        finally
        {
            _credentialRetryPending = false;
        }
    }

    public bool RetryCredentialRestore()
    {
        if (!_credentialRetryPending) return false;
        _credentialWake.Release();
        return true;
    }

    async Task SignInAsync(bool restored, CancellationToken token)
    {
        // The session's background work ends with it (sign-out cancels it).
        var session = CancellationTokenSource.CreateLinkedTokenSource(token);
        Interlocked.Exchange(ref _session, session)?.Cancel();
        token = session.Token;
        ClientErrorInfo? credentialError = null;
        if (!restored)
        {
            var blob = JsonSerializer.SerializeToUtf8Bytes(new
            {
                access_token = "fake-" + Guid.NewGuid().ToString("N"),
                refresh_token = "fake-" + Guid.NewGuid().ToString("N"),
                issued_at = DateTimeOffset.UtcNow,
            });
            try
            {
                _platform.CredentialSave(blob);
            }
            catch (PlatformException error)
            {
                _log.Error($"credential save failed: {error.Message}");
                credentialError = new(StoreErrorCode(error), error.Message);
            }
        }

        _log.Info(restored ? "session restored" : "device login approved");
        var installed = _platform.PrivilegedServiceInstalled();
        // Before the SignedIn snapshot, so a list loaded at sign-in already has them.
        SeedMessages();
        Update(s => s with
        {
            Auth = new AuthState.SignedIn(),
            UnreadNotifications = UnreadCount(),
            Account = new Account("acct-7f3a", "Alice", null, AccountEmail),
            Team = _teams.First(t => t.Id == PersonalTeamId),
            ProfileStatus = new ProfileStatus.Loading(),
            Standard = new StandardState.Stopped(),
            ServiceInstalled = installed,
            LastError = credentialError,
        });

        await Delay(500, token).ConfigureAwait(false);
        await FetchProfileAsync(userAction: false, token).ConfigureAwait(false);
        token.ThrowIfCancellationRequested();
        StartTraffic();
        if (_options.UnavailableRuleSets.Count > 0) SimulateRuleSets(token);

        _ = Task.Run(async () =>
        {
            try
            {
                await Delay(_options.FirstPollAfter, token).ConfigureAwait(false);
                Poll();
            }
            catch (OperationCanceledException) { }
        }, CancellationToken.None);

        if (_options.FailBackgroundRefresh)
        {
            await Delay(1500, token).ConfigureAwait(false);
            _log.Warn("profile refresh failed: error sending request (dns error)");
            Update(s => s with { LastError = new(ErrorCode.NetworkUnreachable, "dns error: no such host") });
        }
    }

    /// <summary>The core could not download some rule sets at first, then loads them (the crate polls status).</summary>
    void SimulateRuleSets(CancellationToken token)
    {
        var ids = _options.UnavailableRuleSets.ToArray();
        _log.Warn($"rule sets unavailable: {string.Join(", ", ids)} (RULE_SET_DOWNLOAD_FAILED)");
        UpdateSignedIn(s => s with { RuleSetsUnavailable = ids });
        _ = Task.Run(async () =>
        {
            try
            {
                await Delay(_options.RuleSetsLoadAfter, token).ConfigureAwait(false);
                _log.Info("rule sets ready");
                UpdateSignedIn(s => s with { RuleSetsUnavailable = [] });
            }
            catch (OperationCanceledException) { }
        }, CancellationToken.None);
    }

    sealed record Blocked(ProfileStatus Status, ErrorCode Code, string Detail);

    /// <summary>
    /// Like the crate: a blocking answer (no subscription, expired, team disabled) becomes
    /// <c>ProfileStatus</c>, drops the profile and stops the cores, and stays out of
    /// <c>LastError</c>; user-initiated calls still fail with its code.
    /// </summary>
    async Task FetchProfileAsync(bool userAction, CancellationToken token)
    {
        token.ThrowIfCancellationRequested();
        var teamId = Snapshot().Team?.Id ?? PersonalTeamId;
        FakeProfileScenario scenario;
        lock (_gate) scenario = _scenarios.GetValueOrDefault(teamId, FakeProfileScenario.Active);

        var blocked = scenario switch
        {
            FakeProfileScenario.NoSubscription => new Blocked(new ProfileStatus.NoSubscription(), ErrorCode.NoSubscription, "GET /api/v1/me/proxy-profile -> HTTP 402 402001"),
            FakeProfileScenario.SubscriptionExpired => new Blocked(new ProfileStatus.SubscriptionExpired(ExpiredAt()), ErrorCode.SubscriptionExpired, "GET /api/v1/me/proxy-profile -> HTTP 402 402002"),
            FakeProfileScenario.TeamDisabled => new Blocked(new ProfileStatus.TeamDisabled(), ErrorCode.TeamDisabled, "GET /api/v1/me/proxy-profile -> HTTP 403 403012"),
            _ => null,
        };
        if (blocked is { } b)
        {
            _log.Warn($"profile unavailable: {b.Code} ({b.Detail})");
            StopConnectTimers();
            UpdateSignedIn(s => s with
            {
                Profile = null,
                ProfileStatus = b.Status,
                Standard = new StandardState.Stopped(),
                Connection = ConnectionStates.Off,
                LastError = userAction || IsAccountError(s.LastError) ? null : s.LastError,
            });
            if (userAction) throw new ClientException.Failed(b.Code, b.Detail);
            return;
        }

        if (scenario == FakeProfileScenario.InvalidNoHistory)
        {
            var info = new ClientErrorInfo(ErrorCode.ProfileInvalid, "outbound 3 has unknown field \"flow\"");
            _log.Error($"profile rejected: {info.Detail}");
            UpdateSignedIn(s => s with { Profile = null, ProfileStatus = new ProfileStatus.Invalid(info) });
            if (userAction) throw new ClientException.Failed(info.Code, info.Detail);
            return;
        }

        var revision = NextRevision();
        var changed = false;
        _log.Info($"profile {revision}: {_nodes.Length} nodes");
        UpdateSignedIn(s =>
        {
            changed = s.Profile is null;
            if (!changed && scenario != FakeProfileScenario.Invalid)
                return s with { ProfileStatus = new ProfileStatus.Ready(), LastError = userAction ? null : s.LastError };
            var selected = s.SelectedNodeId is { } id && _nodes.Any(n => n.Id == id) ? id : _nodes[0].Id;
            return s with
            {
                Profile = SampleProfile(revision),
                ProfileStatus = new ProfileStatus.Ready(),
                SelectedNodeId = selected,
                Standard = new StandardState.Starting(),
                LastError = userAction ? null : s.LastError,
            };
        });
        if (!changed && scenario != FakeProfileScenario.Invalid) return;
        SaveSelection(Snapshot().SelectedNodeId);

        await Delay(700, token).ConfigureAwait(false);
        if (scenario == FakeProfileScenario.Invalid)
        {
            var info = new ClientErrorInfo(ErrorCode.ProfileInvalid, $"core rejected {revision}: outbound 3 has unknown field \"flow\"");
            _log.Error($"standard core rejected the profile: {info.Detail}");
            // The previous profile stays in use; the standard core keeps running on it.
            UpdateSignedIn(s => s with
            {
                ProfileStatus = new ProfileStatus.Invalid(info),
                Standard = new StandardState.Ready(s.Profile?.Revision ?? revision),
                Connection = s.Connection.Phase == ConnectionPhase.Off && CurrentNode() is { } current ? s.Connection with { Detail = ConnectionTo(current) } : s.Connection,
            });
            return;
        }
        _log.Info($"standard core ready ({revision})");
        UpdateSignedIn(s => s with
        {
            Standard = new StandardState.Ready(s.Profile?.Revision ?? revision),
            Connection = s.Connection.Phase == ConnectionPhase.Off && CurrentNode() is { } current ? s.Connection with { Detail = ConnectionTo(current) } : s.Connection,
        });
    }

    static bool IsAccountError(ClientErrorInfo? error) =>
        error?.Code is ErrorCode.NetworkUnreachable or ErrorCode.ServerUnavailable or ErrorCode.RateLimited
            or ErrorCode.ProfileFetchFailed or ErrorCode.ProfileInvalid;

    /// <summary>Traffic whenever signed in (like the crate since 0c83d56): standard + enhanced cores.</summary>
    void StartTraffic()
    {
        if (!_options.Traffic) return;
        var cts = new CancellationTokenSource();
        if (Interlocked.CompareExchange(ref _traffic, cts, null) is not null) return;
        var token = cts.Token;
        _ = Task.Run(async () =>
        {
            double down = 900_000, up = 60_000;
            while (!token.IsCancellationRequested)
            {
                var s = Snapshot();
                var k = s.Connection.Phase != ConnectionPhase.On ? 0.04 : s.ConnectionMode == ConnectionMode.Enhanced ? 1 : 0.35;
                // Smooth random walk so the numbers look like real traffic (bytes per second).
                down = Math.Clamp(down * (0.75 + Random.Shared.NextDouble() * 0.5) + Random.Shared.Next(-50_000, 50_000), 12_000, 6_500_000);
                up = Math.Clamp(up * (0.75 + Random.Shared.NextDouble() * 0.5) + Random.Shared.Next(-5_000, 5_000), 2_000, 700_000);
                var sample = new TrafficSample((ulong)(up * k), (ulong)(down * k), 0, 0);
                Emit(() => _listener.OnTraffic(sample));
                try { await Delay(1000, token).ConfigureAwait(false); } catch (OperationCanceledException) { break; }
            }
        }, CancellationToken.None);
    }

    void StopTraffic()
    {
        if (Interlocked.Exchange(ref _traffic, null) is { } traffic)
        {
            traffic.Cancel();
            Emit(() => _listener.OnTraffic(new TrafficSample(0, 0, 0, 0)));
        }
    }

    void StopConnectTimers() => Interlocked.Exchange(ref _connect, null)?.Cancel();

    /// <summary>Like the crate's finish_action: a failed action lands in LastError.</summary>
    ClientException Report(ClientException.Failed error)
    {
        var info = new ClientErrorInfo(error.code, error.detail);
        Update(s => s with { LastError = info });
        return error;
    }

    /// <summary>
    /// <see cref="Update"/> for session work (sign-in, profile fetch): ignored once signed out, so
    /// an in-flight fetch that a sign-out overtook cannot bring the session's state back.
    /// </summary>
    void UpdateSignedIn(Func<ClientSnapshot, ClientSnapshot> change) =>
        Update(s => s.Auth is AuthState.SignedIn ? change(s) : s);

    void Update(Func<ClientSnapshot, ClientSnapshot> change)
    {
        ClientSnapshot snapshot;
        lock (_gate)
        {
            snapshot = change(_snapshot);
            // Like the crate: an unchanged snapshot is never sent (the initial one is read via Snapshot()).
            if (Equals(snapshot, _snapshot)) return;
            _snapshot = snapshot;
        }
        Emit(() => _listener.OnSnapshot(snapshot));
    }

    /// <summary>Queue a listener callback for the background pump (keeps order).</summary>
    void Emit(Action callback) => _events.Writer.TryWrite(callback);

    /// <summary><see cref="Emit"/>, completing once the listener has been called.</summary>
    Task EmitAsync(Action callback)
    {
        var delivered = new TaskCompletionSource(TaskCreationOptions.RunContinuationsAsynchronously);
        if (!_events.Writer.TryWrite(() =>
            {
                try { callback(); }
                finally { delivered.TrySetResult(); }
            }))
            delivered.TrySetResult();
        return delivered.Task;
    }

    async Task PumpEventsAsync()
    {
        await foreach (var callback in _events.Reader.ReadAllAsync().ConfigureAwait(false))
        {
            try { callback(); }
            catch (Exception error) { _log.Error($"listener threw: {error}"); }
        }
    }

    void RequireSignedIn()
    {
        if (Snapshot().Auth is not AuthState.SignedIn) throw new ClientException.NotSignedIn();
    }

    Node? CurrentNode()
    {
        var id = Snapshot().SelectedNodeId;
        return _nodes.FirstOrDefault(n => n.Id == id);
    }

    Task Delay(int milliseconds, CancellationToken token = default) =>
        Delay(TimeSpan.FromMilliseconds(milliseconds), token);

    Task Delay(TimeSpan delay, CancellationToken token = default) =>
        Task.Delay(delay * _options.TimeScale, token);

    string NextRevision() => $"rev-{Interlocked.Increment(ref _revision)}";

    ProfileSummary SampleProfile(string revision) =>
        new(revision, DateTimeOffset.Now.AddDays(92).Date.AddHours(23).AddMinutes(59).ToString("yyyy-MM-ddTHH:mm:sszzz", CultureInfo.InvariantCulture), (uint)_nodes.Length);

    const uint TimeoutLatency = uint.MaxValue;
    const uint FailedLatency = uint.MaxValue - 1;

    /// <summary>The design's sample latency: a number, or the timeout / failed markers.</summary>
    static uint? SampleLatency(string nodeId) => nodeId switch
    {
        "hk1" => 38, "hk2" => 46, "tw1" => 55, "jp1" => 62, "sg1" => 74, "kr1" => 83, "jp2" => 91,
        "us1" => 168, "us2" => 231, "de1" => TimeoutLatency, "gb1" => FailedLatency, _ => null,
    };

    static readonly ConnectionDetail NoConnection = new(null, null, null, null);

    /// <summary>The standard core's path to <paramref name="node"/>: its first replica and the sample latency.</summary>
    static ConnectionDetail ConnectionTo(Node node) => new(node.Replicas[0].EndpointKey, node.Replicas[0].Label, null, RealLatency(node.Id));

    static string ExpiredAt() =>
        DateTimeOffset.Now.AddDays(-29).Date.ToString("yyyy-MM-ddT00:00:00zzz", CultureInfo.InvariantCulture);

    static uint? RealLatency(string nodeId) => SampleLatency(nodeId) is { } v && v < FailedLatency ? v : null;

    /// <summary>Whether a probe of this sample node times out (Frankfurt 01) or fails (London 01).</summary>
    public static string? ExpectedFailure(string nodeId) => nodeId switch { "de1" => "timeout", "gb1" => "failed", _ => null };

    static string Describe(Node? node) => node is null ? "?" : $"{node.Name} ({node.EntryLabel ?? node.EntryKey})";

    static string RandomLetters(int count)
    {
        const string alphabet = "BCDFGHJKLMNPQRSTVWXZ";
        return new string(Enumerable.Range(0, count).Select(_ => alphabet[Random.Shared.Next(alphabet.Length)]).ToArray());
    }

    string SelectionPath => Path.Combine(_config.DataDir, "selected-node");

    string? LoadSelection()
    {
        try { return File.Exists(SelectionPath) ? File.ReadAllText(SelectionPath).Trim() : null; }
        catch (IOException) { return null; }
        catch (UnauthorizedAccessException) { return null; }
    }

    void SaveSelection(string? nodeId)
    {
        try
        {
            Directory.CreateDirectory(_config.DataDir);
            if (nodeId is null) File.Delete(SelectionPath);
            else File.WriteAllText(SelectionPath, nodeId);
        }
        catch (IOException) { }
        catch (UnauthorizedAccessException) { }
    }

    /// <summary>
    /// The design's sample nodes (sample-nodes.json). Replica endpoint keys are ids ("hk1-r0");
    /// their labels (<c>Replica.label</c>) are the line names the design shows ("HKG-A").
    /// </summary>
    internal static Node[] SampleNodes()
    {
        static Node N(string id, string name, string? tier, string region, string cc, params string[] routes) =>
            new(id, name, tier ?? "standard", tier, region, cc, true,
                routes.Select((r, i) => new Replica($"{id}-r{i}", (uint)i, i % 2 == 0 ? "vless" : "anytls", r)).ToArray());

        return
        [
            N("hk1", "香港 01", "专线", "中国香港", "HK", "HKG-A", "HKG-B", "SZX-R"),
            N("hk2", "香港 02", null, "中国香港", "HK", "HKG-C", "HKG-D"),
            N("tw1", "台北 01", "优化", "中国台湾", "TW", "TPE-A", "TPE-B"),
            N("jp1", "东京 01", "专线", "日本", "JP", "NRT-A", "NRT-B", "HND-A"),
            N("sg1", "新加坡 01", "优化", "新加坡", "SG", "SIN-A", "SIN-B"),
            N("kr1", "首尔 01", null, "韩国", "KR", "ICN-A", "ICN-B"),
            N("jp2", "大阪 01", null, "日本", "JP", "KIX-A"),
            N("us1", "洛杉矶 01", "优化", "美国", "US", "LAX-A", "LAX-B", "SJC-A"),
            N("us2", "纽约 01", null, "美国", "US", "JFK-A"),
            N("de1", "法兰克福 01", null, "德国", "DE", "FRA-A", "FRA-B"),
            N("gb1", "伦敦 01", null, "英国", "GB", "LHR-A"),
        ];
    }
}

/// <summary>Daily log file in the same directory and format the Rust crate writes.</summary>
sealed class FakeLog(string directory)
{
    readonly object _gate = new();

    public void Info(string message) => Write("INFO", message);
    public void Warn(string message) => Write("WARN", message);
    public void Error(string message) => Write("ERROR", message);

    void Write(string level, string message)
    {
        var now = DateTimeOffset.UtcNow;
        var line = $"{now.ToString("yyyy-MM-ddTHH:mm:ss.ffffffZ", CultureInfo.InvariantCulture)}  {level} ppvpn_client: {message}{Environment.NewLine}";
        lock (_gate)
        {
            try
            {
                Directory.CreateDirectory(directory);
                File.AppendAllText(Path.Combine(directory, $"ppvpn-client.{now:yyyy-MM-dd}.log"), line);
            }
            catch (IOException) { }
            catch (UnauthorizedAccessException) { }
        }
    }
}
