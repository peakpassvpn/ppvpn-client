using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.App.Core.Backend;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// The connection as the UI names it (core.js <c>tun</c> states). Compatible mode uses Off /
/// Connecting ("applying the system proxy") / On / Reconnecting / Disconnecting / Failed.
/// </summary>
public enum ConnectState { Off, Preparing, Authorizing, Connecting, On, Reconnecting, Occupied, Disconnecting, Failed }

/// <summary>The connect switch: off, on, or the indeterminate middle state (knob centred + spinner).</summary>
public enum SwitchVisual { Off, On, Indeterminate }

/// <summary>
/// Which anomaly a <see cref="ConnectionNotice"/> is (append-only). <c>Conflict</c> replaces
/// <c>Failed</c> when the crate names another proxy/VPN app competing for the network path
/// (<see cref="ConnectionState.Competitor"/>). <c>RulesUnavailable</c> is informational (tone
/// Warn, no action): some routing rule sets have not loaded, so that traffic goes through the proxy.
/// <c>IngressUnavailable</c>: the current node is pinned to a line the core reports down (it stays
/// pinned; action <c>backToAuto</c>). <c>IngressPinCleared</c>: a profile refresh dropped a pinned
/// line, the node is back on automatic (action <c>ok</c> dismisses). <c>LocalProxyCredentialsReset</c>:
/// the local proxy got new user names and passwords, apps using it must copy them again (action
/// <c>ok</c> dismisses).
/// </summary>
public enum NoticeKind { Failed, Occupied, ProxyFailed, Conflict, RulesUnavailable, IngressUnavailable, IngressPinCleared, LocalProxyCredentialsReset }

/// <summary>
/// One anomaly row under the connection card (design ③; InfoBar Error/Warning with buttons):
/// the connection failed (<c>retry</c>, and <c>useCompatible</c> when compatibility mode would
/// avoid the failure), it failed because another app took the network over (<c>conflictT</c> /
/// <c>conflictD {app}</c>, same buttons), it is used on another device (<c>takeOver</c>), or the
/// standard core failed so the local proxy is unavailable (<c>proxyFailedT</c>, <c>retry</c>), or
/// some routing rules have not loaded yet (<c>rulesT</c> / <c>rulesUnavailableD</c>, no action).
/// Actions are null when there is nothing to offer.
/// </summary>
public sealed record ConnectionNotice(
    NoticeKind Kind,
    ConnectionTone Tone,
    string Title,
    string Message,
    string? ActionText,
    IAsyncRelayCommand? Action,
    string? SecondaryActionText = null,
    IAsyncRelayCommand? SecondaryAction = null);

/// <summary>Which user of the shared local proxy the overview card shows and copies.</summary>
public enum LocalProxyScope
{
    /// <summary>The routed user: Profile rules (direct where they say so), then the selected node.</summary>
    Routed,
    /// <summary>The current node's user: everything through that node.</summary>
    Node,
}

/// <summary>An option of the local-proxy user picker (<c>proxyRouted</c> / <c>proxyNode</c>).</summary>
public sealed record LocalProxyScopeOption(LocalProxyScope Scope, string Title, string Description)
{
    public override string ToString() => Title;
}

public sealed record RoutingModeOption(RoutingMode Mode, string Title, string Description)
{
    public override string ToString() => Title;
}

/// <summary>A connection mode in Settings › Advanced (title + one-line description).</summary>
public sealed record ConnectionModeOption(ConnectionMode Mode, string Title, string Description)
{
    public override string ToString() => Title;
}

public sealed partial class MainViewModel
{
    // --- connection card header / title bar -------------------------------------

    /// <summary>Enhanced (TUN, default) or Compatible (system proxy); changed in Settings › Advanced.</summary>
    [ObservableProperty] ConnectionMode connectionMode;
    /// <summary><c>routingRules</c> (default) or <c>routingGlobal</c>; changed in Settings, applied without a reconnect.</summary>
    [ObservableProperty] RoutingMode routingMode;
    [ObservableProperty] ConnectState connectState;

    /// <summary>Status title (24 Semibold), from the connection's phase in the current method.</summary>
    [ObservableProperty] string connectionTitle = "";
    /// <summary>Line 2 after flag + node name: "HKG-A · 38 ms", "正在尝试 HKG-A", ….</summary>
    [ObservableProperty] string connectionDetail = "";
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsConnectionBusy), nameof(IsConnectionError))] ConnectionTone connectionTone;
    /// <summary>The status circle spins.</summary>
    public bool IsConnectionBusy => ConnectionTone == ConnectionTone.Busy;
    /// <summary>The title uses the danger colour.</summary>
    public bool IsConnectionError => ConnectionTone == ConnectionTone.Error;

    /// <summary>Title-bar status line: a short state ("已连接", "正在连接", or the failure/contention title), the restricted title, <c>notSignedIn</c>, or empty while restoring the saved login. Never the node name.</summary>
    [ObservableProperty] string statusLine = "";
    /// <summary>Title-bar dot: <see cref="ConnectionTone.Idle"/> while signed out or restricted.</summary>
    [ObservableProperty] ConnectionTone statusDotTone;

    // --- the connect switch (right of the status header) ------------------------------

    [ObservableProperty] SwitchVisual connectSwitch;
    /// <summary>
    /// False while preparing, authorizing and disconnecting (and without a usable profile).
    /// Connecting and reconnecting stay enabled: a tap cancels.
    /// </summary>
    [ObservableProperty] bool connectSwitchEnabled;
    /// <summary>
    /// The weakest-tone line under the status header, naming the connection mode:
    /// <c>captionEnhanced</c> "增强模式" / <c>captionCompatible</c>.
    /// </summary>
    [ObservableProperty] string connectionMethodCaption = "";

    [ObservableProperty] bool serviceInstalled;
    /// <summary><c>serviceOn</c> / <c>serviceOff</c> (Settings › Advanced).</summary>
    [ObservableProperty] string serviceStatusText = "";
    /// <summary>Enhanced method and the service missing: the accent box <c>needInstall</c> + <c>installBtn</c>.</summary>
    [ObservableProperty] bool showInstallHint;
    [ObservableProperty] bool isInstallingService;

    /// <summary>
    /// Anomalies (design ③): failed (retry / use compatibility mode), occupied (take over), or
    /// the local proxy is unavailable because the standard core failed (retry).
    /// </summary>
    public ObservableCollection<ConnectionNotice> Notices { get; } = [];

    /// <summary>Settings › Advanced "Connection Method" options: Enhanced (recommended), Compatible.</summary>
    public IReadOnlyList<ConnectionModeOption> ConnectionModes { get; private set; } = [];

    /// <summary>Settings <c>routingMode</c> options: Rules (default), Global.</summary>
    public IReadOnlyList<RoutingModeOption> RoutingModes { get; private set; } = [];

    // --- current node, local proxy, traffic -------------------------------------------

    /// <summary>The current node (combo on the connection card; items are <c>Nodes.Items</c>). Two-way.</summary>
    [ObservableProperty] NodeItemViewModel? currentNode;
    [ObservableProperty] string currentNodeName = "";
    [ObservableProperty] string currentNodeCountryCode = "";
    /// <summary>Latency of the current line ("38 ms" / "—") for the tray header and the title detail.</summary>
    [ObservableProperty] string currentNodeLatencyText = "—";

    /// <summary>Local proxy of the current node (overview ⑤); null while the standard core is not ready or failed.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasCurrentNodeProxy), nameof(ShownProxy), nameof(HasShownProxy), nameof(ProxyNote))] ProxyInfo? currentNodeProxy;
    public bool HasCurrentNodeProxy => CurrentNodeProxy is not null;

    /// <summary>
    /// The routed user of the same port (follows the routing rules and mode); null while the standard
    /// core is not ready, failed, or predates it (0.5.12).
    /// </summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasRoutedProxy), nameof(ShownProxy), nameof(HasShownProxy), nameof(ProxyNote))] ProxyInfo? routedProxy;
    public bool HasRoutedProxy => RoutedProxy is not null;

    IReadOnlyList<LocalProxyScopeOption>? _proxyScopes;
    /// <summary>The user picker of the local proxy card: follow the rules (default), or this node only.</summary>
    public IReadOnlyList<LocalProxyScopeOption> ProxyScopes => _proxyScopes ??=
    [
        new(LocalProxyScope.Routed, _strings.Get("proxyRouted"), _strings.Get("proxyRoutedD")),
        new(LocalProxyScope.Node, _strings.Get("proxyNode"), _strings.Get("proxyNodeD")),
    ];

    LocalProxyScopeOption? _selectedProxyScope;
    /// <summary>
    /// The picker's value, two-way. A null written back by a view, or the same value, is ignored
    /// without a change notification: re-raising made WinUI's ComboBox write back again (stack
    /// overflow).
    /// </summary>
    public LocalProxyScopeOption SelectedProxyScope
    {
        get => _selectedProxyScope ??= ProxyScopes[0];
        set
        {
            if (value is null || value == SelectedProxyScope) return;
            _selectedProxyScope = value;
            OnPropertyChanged();
            OnPropertyChanged(nameof(ShownProxy));
            OnPropertyChanged(nameof(HasShownProxy));
            OnPropertyChanged(nameof(ProxyNote));
        }
    }

    /// <summary>The card's note: what the shown user does (<c>proxyNoteRouted</c> / <c>proxyNote</c>).</summary>
    public string ProxyNote => _strings.Get(ShownProxy?.IsRouted == true ? "proxyNoteRouted" : "proxyNote");

    /// <summary>
    /// What the card shows and copies: the routed user when chosen and the core serves it, else the
    /// current node's user. Examples use http:// or socks5h:// (SOCKS5 by IP misses domain rules).
    /// </summary>
    public ProxyInfo? ShownProxy =>
        SelectedProxyScope.Scope == LocalProxyScope.Routed && RoutedProxy is { } routed ? routed : CurrentNodeProxy;
    public bool HasShownProxy => ShownProxy is not null;
    /// <summary>Why there is no local proxy yet (<c>proxyStarting</c> / <c>proxyFailed</c>); null when available.</summary>
    [ObservableProperty] string? proxyUnavailableText;

    /// <summary>Traffic cards (④): shown whenever signed in with access.</summary>
    [ObservableProperty] bool showTraffic;
    [ObservableProperty] string upRate = Formatting.Rate(0);
    [ObservableProperty] string downRate = Formatting.Rate(0);

    // --- commands --------------------------------------------------------------------

    /// <summary>
    /// The connect switch. Off / Failed / Occupied → connect (Enhanced asks
    /// <see cref="PromptKind.InstallService"/> first when the service is missing; cancelling
    /// leaves the switch off, not failed); On / Connecting / Reconnecting → disconnect;
    /// otherwise nothing.
    /// </summary>
    [RelayCommand]
    Task ToggleConnectAsync() => ConnectState switch
    {
        ConnectState.Off or ConnectState.Failed or ConnectState.Occupied => ConnectAsync(),
        ConnectState.On or ConnectState.Connecting or ConnectState.Reconnecting => RunAsync("disconnect", Backend.Disconnect),
        _ => ResyncSwitches(),
    };

    /// <summary>The failure notice's Retry (asks to install again when needed).</summary>
    [RelayCommand]
    Task RetryConnectAsync() => ConnectAsync();

    /// <summary>
    /// The local-proxy notice's Retry (standard core failed): refetches the profile. The crate has
    /// no call that restarts the standard core by itself; <c>refresh_profile</c> re-applies the
    /// profile to the core only when the revision changed.
    /// </summary>
    [RelayCommand]
    Task RetryLocalProxyAsync() => RunAsync("refresh_profile", Backend.RefreshProfile);

    /// <summary>The occupied notice's "Use on This Device".</summary>
    [RelayCommand]
    Task TakeOverAsync() => RunAsync("enhanced_take_over", Backend.EnhancedTakeOver, quiet: _ => IsShownByNotice);

    /// <summary>The failure notice's "Use Compatibility Mode": switch the method, then connect.</summary>
    [RelayCommand]
    async Task UseCompatibleAsync()
    {
        if (await RunAsync("set_connection_mode", () => Backend.SetConnectionMode(ConnectionMode.Compatible), quiet: _ => IsShownByNotice))
            await ConnectAsync();
    }

    /// <summary>The <c>IngressUnavailable</c> notice's <c>backToAuto</c>: the current node follows automatic failover again.</summary>
    [RelayCommand]
    Task UnpinCurrentNodeAsync() =>
        CurrentNode is { } node ? RunAsync("pin_ingress", () => Backend.PinIngress(node.Id, null)) : Task.CompletedTask;

    /// <summary>The <c>IngressPinCleared</c> notice's <c>ok</c>.</summary>
    [RelayCommand]
    Task DismissClearedPinsAsync()
    {
        Backend.DismissClearedIngressPins();
        return Task.CompletedTask;
    }

    /// <summary>The <c>LocalProxyCredentialsReset</c> notice's <c>ok</c>.</summary>
    [RelayCommand]
    Task DismissProxyResetAsync()
    {
        Backend.DismissLocalProxyCredentialsReset();
        return Task.CompletedTask;
    }

    /// <summary>Settings: rules or global routing (no reconnect).</summary>
    public Task SetRoutingModeAsync(RoutingMode mode) =>
        mode == RoutingMode
            ? Task.CompletedTask
            : RunAsync("set_routing_mode", () => Backend.SetRoutingMode(mode));

    /// <summary>Settings › Advanced: choose the mode (reconnects in it while connected).</summary>
    public Task SetConnectionModeAsync(ConnectionMode mode) =>
        mode == ConnectionMode
            ? Task.CompletedTask
            : RunAsync("set_connection_mode", () => Backend.SetConnectionMode(mode), quiet: _ => IsShownByNotice);

    /// <summary>
    /// "Install System Service…" (hint box and Settings): explain first, then the OS prompt.
    /// Install only; a dismissed OS prompt changes nothing.
    /// </summary>
    [RelayCommand]
    async Task InstallServiceAsync()
    {
        if (!await _prompts.ConfirmAsync(PromptKind.InstallService)) return;
        IsInstallingService = true;
        try
        {
            await RunAsync("service_install", Backend.ServiceInstall, "installFailT");
        }
        finally
        {
            IsInstallingService = false;
        }
    }

    /// <summary>Settings › Advanced "Uninstall…": confirm, then the OS prompt. A dismissed prompt is silent.</summary>
    [RelayCommand]
    async Task UninstallServiceAsync()
    {
        if (!await _prompts.ConfirmAsync(PromptKind.UninstallService)) return;
        await RunAsync("service_uninstall", Backend.ServiceUninstall, "uninstallFailT",
            quiet: error => ErrorMessages.Code(error) == ErrorCode.ServiceUninstallCancelled);
    }

    public Task SelectNodeAsync(string nodeId) => RunAsync("select_node", () => Backend.SelectNode(nodeId));

    async Task ConnectAsync()
    {
        // The snapshot may be stale (an admin installed the service outside the app): check the
        // system before explaining an install.
        if (ConnectionMode == ConnectionMode.Enhanced && !Snapshot.ServiceInstalled
            && !await Backend.RefreshServiceInstalled()
            && !await _prompts.ConfirmAsync(PromptKind.InstallService))
        {
            // Cancelled at the explanation: the switch goes back to off, not failed.
            await ResyncSwitches();
            return;
        }
        var retry = Snapshot.Connection.Phase is ConnectionPhase.Error or ConnectionPhase.Contended;
        // A failure that leaves Error / Contended (e.g. the OS prompt was denied: fr_auth) is shown by the notice.
        await RunAsync(retry ? "retry" : "connect", retry ? Backend.Retry : Backend.Connect, quiet: _ => IsShownByNotice);
    }

    /// <summary>
    /// The connection is failed or occupied, so its notice (reason + Retry / Take Over / Use
    /// Compatibility Mode) already shows the failure: a connection command's error is logged only,
    /// never a dialog on top. Read from the backend's current snapshot, not <see cref="ConnectState"/>:
    /// the command's error returns before the snapshot that failed it has been posted to the UI thread.
    /// </summary>
    bool IsShownByNotice => State(Backend.Snapshot().Connection) is ConnectState.Failed or ConnectState.Occupied;

    Task ResyncSwitches()
    {
        SyncControls();
        return Task.CompletedTask;
    }

    partial void OnCurrentNodeChanged(NodeItemViewModel? value)
    {
        if (_applying || value is null || value.Id == Snapshot.SelectedNodeId) return;
        _ = SelectNodeAsync(value.Id);
    }

    // --- derivation ----------------------------------------------------------------

    void ApplyConnection(ClientSnapshot next)
    {
        var connection = next.Connection;
        var method = next.ConnectionMode;
        var signedIn = next.Auth is AuthState.SignedIn;
        var usable = signedIn && !IsRestricted && next.Profile is not null;
        var state = State(connection);
        ConnectionMode = method;
        RoutingMode = next.RoutingMode;
        ConnectState = state;
        if (RoutingModes.Count == 0)
        {
            RoutingModes =
            [
                new(RoutingMode.Rules, _strings.Get("routingRules"), _strings.Get("routingRulesD")),
                new(RoutingMode.Global, _strings.Get("routingGlobal"), _strings.Get("routingGlobalD")),
            ];
        }
        if (ConnectionModes.Count == 0)
        {
            ConnectionModes =
            [
                new(ConnectionMode.Enhanced, _strings.Get("methodEnhanced"), _strings.Get("methodEnhancedD")),
                new(ConnectionMode.Compatible, _strings.Get("methodCompatible"), _strings.Get("methodCompatibleD")),
            ];
        }

        // Current node and its line (the label when the backend sends one).
        var node = Nodes.Items.FirstOrDefault(i => i.Id == next.SelectedNodeId);
        CurrentNode = node;
        CurrentNodeName = node?.Name ?? "";
        CurrentNodeCountryCode = node?.CountryCode ?? "";
        // The standard core failed: its local proxy (and the system proxy on it) is unavailable.
        var standardError = next.Standard is StandardState.Failed failed ? failed.Error : null;
        var idleDetail = _strings.Get(standardError is null ? "d_idle" : "d_idleProxyFailed");
        // The line (the connection's path; the standard core's while off): the core's label,
        // else the replica's label. Endpoint keys are internal: without a label the {r} part
        // (and its separator) is left out.
        var detail = connection.Detail;
        // A pinned node only ever uses its pinned line: say that one, whatever the core last
        // reported (it may still name the line before the pin). Otherwise the core's report,
        // then the line it marks active; the first line only for a single-line node (guessing
        // the primary of several showed the wrong line after a failover).
        var pinnedKey = next.IngressPins.FirstOrDefault(p => p.NodeId == next.SelectedNodeId)?.EndpointKey;
        var active = next.NodeIngresses.FirstOrDefault(n => n.NodeId == next.SelectedNodeId)?
            .Ingresses.FirstOrDefault(i => i.Active);
        var route = pinnedKey is not null ? Formatting.LineName(node?.Node, pinnedKey, _strings)
            : detail.EndpointLabel is { Length: > 0 } label ? label
            : detail.EndpointKey is { } key ? Formatting.RouteLabel(node?.Node, key)
            : active is not null ? active.Label is { Length: > 0 } activeLabel ? activeLabel
                : Formatting.RouteLabel(node?.Node, active.EndpointKey)
            : node is { Node.Replicas.Length: 1 } single ? Formatting.RouteLabel(single.Node.Replicas[0])
            : null;
        var latency = detail.LatencyMs ?? node?.LatencyMs;
        var ms = Formatting.Milliseconds(latency);
        CurrentNodeLatencyText = ms;
        string OnDetail(string key) => route is null ? ms : _strings.Format(key, ("r", route), ("ms", ms));
        // Automatic failover moved to another line: say so, lightly (a pinned node never switches).
        var switched = detail.PreviousEndpointKey is not null && detail.PreviousEndpointKey != detail.EndpointKey
            && !next.IngressPins.Any(p => p.NodeId == next.SelectedNodeId);
        var trying = route is null ? "" : _strings.Format("d_connecting", ("r", route));

        // Header (core.js vals(): the enhanced table in Enhanced, the system-proxy one in Compatible).
        ConnectionTone = state switch
        {
            ConnectState.Off => ConnectionTone.Idle,
            ConnectState.On => ConnectionTone.Ok,
            ConnectState.Occupied => ConnectionTone.Warn,
            ConnectState.Failed => ConnectionTone.Error,
            _ => ConnectionTone.Busy,
        };
        if (method == ConnectionMode.Enhanced)
        {
            ConnectionTitle = state switch
            {
                ConnectState.Off => _strings.Get("h_idle"),
                ConnectState.On => _strings.Get("h_on"),
                ConnectState.Failed => _strings.Get(PathTakenOver(connection) ? "h_pathContended" : "h_failed"),
                ConnectState.Occupied => _strings.Get("h_occupied"),
                _ => _strings.Get($"st_{StateKey(state)}"),
            };
            ConnectionDetail = state switch
            {
                ConnectState.Off => idleDetail,
                ConnectState.Preparing => _strings.Get("d_preparing"),
                ConnectState.Authorizing => _strings.Get("d_authorizing"),
                ConnectState.Connecting => trying,
                ConnectState.On => OnDetail(switched ? "d_switched" : "d_on"),
                ConnectState.Reconnecting => Reconnecting(connection, node, route),
                ConnectState.Disconnecting => _strings.Get("d_disconnecting"),
                // Failed / occupied: the notice carries the reason, and the method
                // caption already says "增强模式".
                _ => "",
            };
        }
        else
        {
            (ConnectionTitle, ConnectionDetail) = state switch
            {
                ConnectState.Off => (_strings.Get("h_idle"), idleDetail),
                // Automatic failover says so here too, as in enhanced mode.
                ConnectState.On => (_strings.Get("h_on"), OnDetail(switched ? "d_switched" : "d_stdOn")),
                ConnectState.Failed => (_strings.Get("h_stdFail"), FailReason(connection, method)),
                ConnectState.Reconnecting => (_strings.Get("st_reconnecting"), Reconnecting(connection, node, route)),
                ConnectState.Disconnecting => (_strings.Get("st_disconnecting"), _strings.Get("d_disconnecting")),
                _ => (_strings.Get("h_stdStarting"), ""),
            };
        }

        // Restoring the saved login at launch is not "signed out": say nothing yet.
        StatusLine = next.Auth is AuthState.Restoring ? ""
            : !signedIn ? _strings.Get("notSignedIn")
            : IsRestricted ? RestrictedTitle
            // A short state only (no node name): the title bar has little room.
            : state switch
            {
                ConnectState.Off => _strings.Get("h_idle"),
                ConnectState.On => _strings.Get("h_on"),
                ConnectState.Preparing or ConnectState.Authorizing or ConnectState.Connecting => _strings.Get("st_connecting"),
                // As the card says: the connection dropped and is coming back.
                ConnectState.Reconnecting => _strings.Get("st_reconnecting"),
                ConnectState.Disconnecting => _strings.Get("st_disconnecting"),
                _ => ConnectionTitle,
            };
        StatusDotTone = !signedIn || IsRestricted ? ConnectionTone.Idle : ConnectionTone;

        // The switch (Spec §4).
        ConnectSwitch = state switch
        {
            ConnectState.On => SwitchVisual.On,
            ConnectState.Preparing or ConnectState.Authorizing or ConnectState.Connecting or ConnectState.Reconnecting or ConnectState.Disconnecting => SwitchVisual.Indeterminate,
            _ => SwitchVisual.Off,
        };
        ConnectSwitchEnabled = usable && node is not null && state is not (ConnectState.Preparing or ConnectState.Authorizing or ConnectState.Disconnecting);
        ConnectionMethodCaption = _strings.Get(method == ConnectionMode.Enhanced ? "captionEnhanced" : "captionCompatible");

        ServiceInstalled = next.ServiceInstalled;
        ServiceStatusText = _strings.Get(ServiceInstalled ? "serviceOn" : "serviceOff");
        ShowInstallHint = usable && method == ConnectionMode.Enhanced && !ServiceInstalled && state is (ConnectState.Off or ConnectState.Failed);

        ApplyNotices(connection, method, usable, state, standardError, next.RuleSetsUnavailable, next);

        // Local proxy of the current node (independent of the connection). A failed standard core
        // serves nothing, whatever proxies were listed before.
        CurrentNodeProxy = standardError is null ? node?.Proxy : null;
        var routedProxy = standardError is null ? Nodes.RoutedProxy : null;
        if (routedProxy is null) RoutedProxy = null;
        else if (RoutedProxy?.Proxy != routedProxy) RoutedProxy = new ProxyInfo(routedProxy, _services);
        ProxyUnavailableText = !usable ? null
            : standardError is not null ? _strings.Format("proxyFailed", ("reason", _strings.Message(standardError)))
            : node?.Proxy is not null ? null
            : _strings.Get("proxyStarting");
        ShowTraffic = signedIn && !IsRestricted;
    }

    /// <summary>
    /// <c>d_reconnecting {r0} {r}</c> when both lines have labels, <c>d_connecting {r}</c> when
    /// only the new one has, otherwise nothing.
    /// </summary>
    string Reconnecting(ConnectionState connection, NodeItemViewModel? node, string? route)
    {
        if (route is null) return "";
        // The previous line may belong to the previous node.
        var previous = connection.Detail.PreviousEndpointKey is { } key
            && Nodes.Items.Select(i => i.Node).Prepend(node?.Node).SelectMany(n => n?.Replicas ?? [])
                .FirstOrDefault(r => r.EndpointKey == key) is { } replica
            ? Formatting.RouteLabel(replica)
            : null;
        return previous is null
            ? _strings.Format("d_connecting", ("r", route))
            : _strings.Format("d_reconnecting", ("r0", previous), ("r", route));
    }

    void ApplyNotices(ConnectionState connection, ConnectionMode method, bool usable, ConnectState state, ClientErrorInfo? standardError,
        IReadOnlyCollection<string> rulesUnavailable, ClientSnapshot next)
    {
        var notices = new List<ConnectionNotice>();
        if (usable && state == ConnectState.Failed)
        {
            var compatible = method == ConnectionMode.Enhanced && connection.SuggestCompatible;
            var retry = _strings.Get("retry");
            var useCompatible = compatible ? _strings.Get("useCompatible") : null;
            // One surface for a failed connect: a named competitor turns it into the conflict notice.
            if (Competitor(connection) is { } app)
            {
                notices.Add(new(NoticeKind.Conflict, ConnectionTone.Error, _strings.Get("conflictT"),
                    _strings.Format("conflictD", ("app", app)),
                    retry, RetryConnectCommand, useCompatible, compatible ? UseCompatibleCommand : null));
            }
            else
            {
                var title = method == ConnectionMode.Enhanced
                    ? $"{_strings.Get("tunMode")} · {_strings.Get("st_failed")}"
                    : $"{_strings.Get("stdMode")} · {_strings.Get("std_failed")}";
                notices.Add(new(NoticeKind.Failed, ConnectionTone.Error, title, FailReason(connection, method),
                    retry, RetryConnectCommand, useCompatible, compatible ? UseCompatibleCommand : null));
            }
        }
        if (usable && state == ConnectState.Occupied)
        {
            var reason = connection.Reason;
            var anotherUser = reason?.Code == ErrorCode.ServiceOwnedByAnotherUser;
            notices.Add(new(NoticeKind.Occupied, ConnectionTone.Warn, _strings.Get("st_occupied"),
                anotherUser ? _strings.Message(reason!) : _strings.Get("d_occupied"),
                connection.CanTakeOver ? _strings.Get("takeOver") : anotherUser ? null : _strings.Get("retry"),
                connection.CanTakeOver ? TakeOverCommand : anotherUser ? null : RetryConnectCommand));
        }
        if (usable && standardError is not null)
        {
            notices.Add(new(NoticeKind.ProxyFailed, ConnectionTone.Error, _strings.Get("proxyFailedT"),
                _strings.Message(standardError), _strings.Get("retry"), RetryLocalProxyCommand));
        }
        // Low-key and self-clearing: the crate empties the list once the rule sets load (stale
        // copies still route and are not listed). Nothing to do about it, so no action.
        if (usable && rulesUnavailable.Count > 0)
        {
            notices.Add(new(NoticeKind.RulesUnavailable, ConnectionTone.Warn, _strings.Get("rulesT"),
                _strings.Get("rulesUnavailableD"), ActionText: null, Action: null));
        }
        // The user chose this line: it stays pinned while down; offer automatic failover.
        if (usable && CurrentNode is { } current
            && next.IngressPins.FirstOrDefault(p => p.NodeId == current.Id) is { } pin
            && next.NodeIngresses.FirstOrDefault(n => n.NodeId == current.Id) is { } reported
            && reported.Ingresses.Any(i => i.EndpointKey == pin.EndpointKey && i.Healthy == false))
        {
            var line = Formatting.LineName(current.Node, pin.EndpointKey, _strings) ?? "";
            notices.Add(new(NoticeKind.IngressUnavailable, ConnectionTone.Warn, _strings.Get("ingressDownT"),
                _strings.Format("ingressDownD", ("n", current.Name), ("r", line)),
                _strings.Get("backToAuto"), UnpinCurrentNodeCommand));
        }
        if (usable && next.ClearedIngressPins.Length > 0)
        {
            var names = string.Join(", ", next.ClearedIngressPins.Select(p =>
                Nodes.Items.FirstOrDefault(i => i.Id == p.NodeId)?.Name ?? p.NodeId).Distinct());
            notices.Add(new(NoticeKind.IngressPinCleared, ConnectionTone.Warn, _strings.Get("pinClearedT"),
                _strings.Format("pinClearedD", ("n", names)), _strings.Get("ok"), DismissClearedPinsCommand));
        }
        // The standard core rebuilt the local proxy credentials: apps holding the old ones (a browser
        // extension) fail until the user copies the new ones. Said once.
        if (usable && next.LocalProxyCredentialsReset)
        {
            notices.Add(new(NoticeKind.LocalProxyCredentialsReset, ConnectionTone.Warn, _strings.Get("proxyResetT"),
                _strings.Get("proxyResetD"), _strings.Get("ok"), DismissProxyResetCommand));
        }
        if (notices.SequenceEqual(Notices)) return;
        Notices.Clear();
        foreach (var notice in notices) Notices.Add(notice);
    }

    /// <summary>
    /// Why the connection failed: Compatible → <c>fr_port</c>, or <c>sysproxyUnavailable</c> when
    /// the desktop has no proxy settings (SystemProxyUnavailable); Enhanced → <c>fr_auth</c> (denied
    /// admin prompt), <c>fr_coreStopped</c> (the core exited), <c>fr_timeout</c> (ConnectFailed / Timeout / Unreachable, and
    /// NetworkPathContended with nobody named: "taken over" is only said when the crate names the
    /// app), or the error text (e.g. <c>Error_ConnectHealthCheckFailed</c>: connected, but the
    /// service is unreachable).
    /// </summary>
    string FailReason(ConnectionState connection, ConnectionMode method)
    {
        var reason = connection.Reason;
        if (method == ConnectionMode.Compatible)
            return reason?.Code == ErrorCode.SystemProxyUnavailable ? _strings.Get("sysproxyUnavailable")
                : reason is null || IsSystemProxyFailure(reason) ? _strings.Get("fr_port")
                : _strings.Message(reason);
        return reason?.Code switch
        {
            ErrorCode.ServiceInstallCancelled => _strings.Get("fr_auth"),
            // The service saw the core exit (crash, killed): not a route timing out.
            ErrorCode.ConnectFailed when reason.Detail.StartsWith("CORE_STOPPED:", StringComparison.Ordinal) =>
                _strings.Get("fr_coreStopped"),
            null or ErrorCode.ConnectFailed or ErrorCode.Timeout or ErrorCode.Unreachable => _strings.Get("fr_timeout"),
            ErrorCode.NetworkPathContended when Competitor(connection) is null => _strings.Get("fr_timeout"),
            _ => _strings.Message(reason),
        };
    }

    static bool IsSystemProxyFailure(ClientErrorInfo reason) => reason.Code == ErrorCode.SystemProxyFailed;

    /// <summary>
    /// The app the crate named as competing for the network path (enhanced mode contended or
    /// failed its health check, e.g. Surge's enhanced mode), or null. In compatible mode the crate
    /// names the owner of the OS proxy it replaced while On; that is not a failure and has no notice.
    /// </summary>
    static string? Competitor(ConnectionState connection) =>
        connection.Competitor is { Length: > 0 } app ? app : null;

    /// <summary>
    /// Enhanced failed because another app took the route or DNS over (<c>h_pathContended</c>):
    /// only when the crate named that app. NetworkPathContended with nobody named (e.g. the
    /// service restarted and the reconnect failed) is an ordinary failure.
    /// </summary>
    static bool PathTakenOver(ConnectionState connection) => Competitor(connection) is not null;

    /// <summary>
    /// Crate phase → UI state. Contended is "occupied" when another session or user owns the
    /// connection (ServiceBusy / ServiceOwnedByAnotherUser); another cause (another VPN took the
    /// route) is shown as failed with its reason.
    /// </summary>
    static ConnectState State(ConnectionState connection) => connection.Phase switch
    {
        ConnectionPhase.Preparing => ConnectState.Preparing,
        ConnectionPhase.WaitingPermission => ConnectState.Authorizing,
        ConnectionPhase.Connecting => ConnectState.Connecting,
        ConnectionPhase.On => ConnectState.On,
        ConnectionPhase.Reconnecting => ConnectState.Reconnecting,
        ConnectionPhase.Contended => connection.CanTakeOver || connection.Reason?.Code is ErrorCode.ServiceBusy or ErrorCode.ServiceOwnedByAnotherUser
            ? ConnectState.Occupied
            : ConnectState.Failed,
        ConnectionPhase.Disconnecting => ConnectState.Disconnecting,
        ConnectionPhase.Error => ConnectState.Failed,
        _ => ConnectState.Off,
    };

    /// <summary>The strings.json suffix of a state: <c>st_{key}</c>.</summary>
    internal static string StateKey(ConnectState state) => state.ToString().ToLowerInvariant();

    /// <summary>The system-proxy wording of a state in Compatible: <c>std_{key}</c>.</summary>
    internal static string StdKey(ConnectState state) => state switch
    {
        ConnectState.On => "on",
        ConnectState.Failed => "failed",
        ConnectState.Off => "off",
        _ => "starting",
    };
}
