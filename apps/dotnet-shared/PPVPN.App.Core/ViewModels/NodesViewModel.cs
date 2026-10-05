using System.Collections.ObjectModel;
using CommunityToolkit.Mvvm.ComponentModel;
using CommunityToolkit.Mvvm.Input;
using PPVPN.Ffi;

namespace PPVPN.App.Core.ViewModels;

/// <summary>
/// One choice of a node's line picker: automatic failover (<see cref="EndpointKey"/> null,
/// <c>lineAuto</c>) or one line (ingress) the node is pinned to.
/// </summary>
public sealed record LineOption(string? EndpointKey, string Title)
{
    public override string ToString() => Title;
}

/// <summary>What the Nodes page shows instead of the table (design <c>nodesView</c> + access).</summary>
public enum NodesViewState
{
    /// <summary>The table.</summary>
    Data,
    /// <summary>40 spinner + <c>loadingT</c> / <c>loadingD</c>.</summary>
    Loading,
    /// <summary><c>invalidT</c> / <c>invalidD</c> + <c>refreshNodes</c>.</summary>
    InvalidNoHistory,
    /// <summary>The restricted card (same as the overview).</summary>
    Restricted,
}

/// <summary>
/// The local proxy of one node: one shared loopback port for every node (7890 when free),
/// told apart by user name. <see cref="HttpDisplay"/> / <see cref="SocksDisplay"/> are shown
/// without credentials; the copy commands put the full URL with credentials on the clipboard
/// (the ✓ feedback is view-local). The user name and password can be copied on their own; the
/// password is masked until <see cref="PasswordRevealed"/> (reset whenever the proxy changes).
/// </summary>
public sealed partial class ProxyInfo : ObservableObject
{
    readonly IAppServices _services;

    internal ProxyInfo(LocalProxy proxy, IAppServices services)
    {
        _services = services;
        Proxy = proxy;
    }

    public LocalProxy Proxy { get; }
    public string Host => Proxy.Host;
    public ushort Port => Proxy.Port;
    /// <summary>"http://127.0.0.1:7890".</summary>
    public string HttpDisplay => $"http://{Proxy.Host}:{Proxy.Port}";
    /// <summary>
    /// The routed user (no node): its traffic follows domain rules, which SOCKS5 by IP misses, so
    /// its SOCKS URLs ask the proxy to resolve (<c>socks5h://</c>).
    /// </summary>
    public bool IsRouted => Proxy.NodeId.Length == 0;
    string SocksScheme => IsRouted ? "socks5h" : "socks5";
    /// <summary>"socks5://127.0.0.1:7890" ("socks5h://…" for the routed user).</summary>
    public string SocksDisplay => $"{SocksScheme}://{Proxy.Host}:{Proxy.Port}";
    /// <summary>"127.0.0.1:7890" (the Nodes bottom card buttons).</summary>
    public string Endpoint => $"{Proxy.Host}:{Proxy.Port}";
    public string Username => Proxy.Username;
    /// <summary>"••••••" (the length is not revealed).</summary>
    public string MaskedPassword => "••••••";

    /// <summary>Show the password in clear (the eye toggle); off by default.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(PasswordDisplay))] bool passwordRevealed;

    /// <summary>The password, or <see cref="MaskedPassword"/> while not revealed.</summary>
    public string PasswordDisplay => PasswordRevealed ? Proxy.Password : MaskedPassword;
    public string HttpUrl => $"http://{Uri.EscapeDataString(Proxy.Username)}:{Uri.EscapeDataString(Proxy.Password)}@{Proxy.Host}:{Proxy.Port}";
    public string SocksUrl => $"{SocksScheme}://{Uri.EscapeDataString(Proxy.Username)}:{Uri.EscapeDataString(Proxy.Password)}@{Proxy.Host}:{Proxy.Port}";

    [RelayCommand]
    void CopyHttp() => _services.CopyText(HttpUrl);

    [RelayCommand]
    void CopySocks() => _services.CopyText(SocksUrl);

    [RelayCommand]
    void CopyUsername() => _services.CopyText(Proxy.Username);

    /// <summary>Copies the password whether or not it is revealed.</summary>
    [RelayCommand]
    void CopyPassword() => _services.CopyText(Proxy.Password);

    [RelayCommand]
    void TogglePasswordRevealed() => PasswordRevealed = !PasswordRevealed;
}

/// <summary>
/// The Nodes page: a flat table in profile order (no instance grouping), speed tests with a
/// method, the selected row's local proxy, and the empty states.
/// </summary>
public sealed partial class NodesViewModel : ObservableObject
{
    static readonly ProbeMethod[] Methods = [ProbeMethod.Icmp, ProbeMethod.Tcp, ProbeMethod.Connect];
    static readonly string[] MethodLabels = ["Ping", "TCP", "Connect"];

    readonly MainViewModel _main;
    readonly ILocalizer _strings;
    readonly IAppServices _services;
    Dictionary<string, LocalProxy> _proxies = [];
    /// <summary>The routed user of the shared port (null: none yet, or a core before 0.5.12).</summary>
    internal LocalProxy? RoutedProxy { get; private set; }
    string? _proxyRevision;
    int _probeRun;

    internal NodesViewModel(MainViewModel main, ILocalizer strings, IAppServices services)
    {
        _main = main;
        _strings = strings;
        _services = services;
        nodeCountText = strings.Format("nodeCount", ("n", 0));
    }

    public MainViewModel Main => _main;

    /// <summary>Raised after the node list was reloaded from the backend.</summary>
    public event Action? CatalogChanged;

    /// <summary>Every node, in profile order.</summary>
    public ObservableCollection<NodeItemViewModel> Items { get; } = [];

    public IEnumerable<Node> AllNodes => Items.Select(i => i.Node);

    /// <summary>Row focus (single click), two-way; the bottom card shows its local proxy.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(SelectedNodeProxy), nameof(SelectedNodeProxyTitle), nameof(HasSelection))] NodeItemViewModel? selectedItem;
    public bool HasSelection => SelectedItem is not null;
    public ProxyInfo? SelectedNodeProxy => SelectedItem?.Proxy;
    /// <summary><c>nodeProxyT</c>: "香港 01 的本地代理".</summary>
    public string SelectedNodeProxyTitle => SelectedItem is { } item ? _strings.Format("nodeProxyT", ("n", item.Name)) : "";

    /// <summary>0 = Ping (ICMP), 1 = TCP, 2 = Connect. TCP by default.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(Method), nameof(MethodLabel))] int methodIndex = 1;
    public ProbeMethod Method => Methods[Math.Clamp(MethodIndex, 0, Methods.Length - 1)];
    public string MethodLabel => MethodLabels[Math.Clamp(MethodIndex, 0, Methods.Length - 1)];
    public IReadOnlyList<string> MethodLabelsList => MethodLabels;

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(TestingText))] bool isProbing;
    /// <summary>Speed tests run through the standard-mode core: only while it is Ready.</summary>
    [ObservableProperty] bool canProbe;
    /// <summary>Status line: "· 正在测速（TCP）" part; empty when idle.</summary>
    public string TestingText => IsProbing ? _strings.Format("testingWith", ("m", MethodLabel)) : "";

    [ObservableProperty] NodesViewState viewState = NodesViewState.Loading;
    /// <summary><c>nodeCount</c>: "11 个节点".</summary>
    [ObservableProperty] string nodeCountText;
    /// <summary>Stale-but-usable configuration: <c>cfgT</c> / <c>cfgD</c> warning above the table.</summary>
    [ObservableProperty] bool configInvalid;

    [RelayCommand(CanExecute = nameof(CanProbe))]
    Task TestAllAsync() => ProbeAsync(Items.ToList());

    /// <summary><c>refreshNodes</c> (and F5).</summary>
    [RelayCommand]
    Task RefreshAsync() => _main.RunAsync("refresh_profile", _main.Backend.RefreshProfile);

    partial void OnCanProbeChanged(bool value)
    {
        TestAllCommand.NotifyCanExecuteChanged();
        foreach (var item in Items) item.NotifyCanProbeChanged();
    }

    partial void OnMethodIndexChanged(int value)
    {
        foreach (var item in Items) item.ClearLatency();
    }

    internal Task ProbeAsync(IReadOnlyList<NodeItemViewModel> items)
    {
        if (items.Count == 0 || !CanProbe) return Task.CompletedTask;
        foreach (var item in items) item.BeginProbe();
        var run = ++_probeRun;
        IsProbing = true;
        var method = Method;
        // An empty list tests every node.
        var ids = items.Count == Items.Count ? [] : items.Select(i => i.Id).ToArray();
        return RunProbeAsync(run, method, ids, items);
    }

    async Task RunProbeAsync(int run, ProbeMethod method, string[] ids, IReadOnlyList<NodeItemViewModel> items)
    {
        await _main.RunAsync("probe", () => _main.Backend.Probe(method, ids));
        // Every node reports before Probe returns; anything still spinning means the call failed.
        foreach (var item in items) item.EndProbeWithoutResult();
        if (run == _probeRun) IsProbing = false;
    }

    internal Task SetCurrentAsync(NodeItemViewModel item) => _main.SelectNodeAsync(item.Id);

    /// <summary>Pin <paramref name="item"/> to a line, or back to automatic with null.</summary>
    internal Task PinAsync(NodeItemViewModel item, string? endpointKey) =>
        endpointKey == item.PinnedEndpointKey
            ? Task.CompletedTask
            : _main.RunAsync("pin_ingress", () => _main.Backend.PinIngress(item.Id, endpointKey));



    // --- from MainViewModel (UI thread) ----------------------------------

    internal void OnSnapshot(ClientSnapshot previous, ClientSnapshot next)
    {
        var signedIn = next.Auth is AuthState.SignedIn;
        CanProbe = signedIn && next.Standard is StandardState.Ready;
        ConfigInvalid = next.ProfileStatus is ProfileStatus.Invalid && next.Profile is not null;
        ViewState = !signedIn ? NodesViewState.Loading
            : next.ProfileStatus is ProfileStatus.NoSubscription or ProfileStatus.SubscriptionExpired or ProfileStatus.TeamDisabled ? NodesViewState.Restricted
            : next.Profile is not null ? NodesViewState.Data
            : next.ProfileStatus is ProfileStatus.Invalid ? NodesViewState.InvalidNoHistory
            : NodesViewState.Loading;

        if (!signedIn || next.Profile is null)
        {
            if (Items.Count > 0) Clear();
            return;
        }

        if (previous.Profile?.Revision != next.Profile.Revision || Items.Count == 0) ReloadCatalog(next);

        foreach (var item in Items)
        {
            item.IsCurrent = item.Id == next.SelectedNodeId;
            item.ApplyIngress(next.IngressPins.FirstOrDefault(p => p.NodeId == item.Id)?.EndpointKey,
                next.NodeIngresses.FirstOrDefault(n => n.NodeId == item.Id));
        }

        if (next.Standard is StandardState.Ready { Revision: var revision })
        {
            if (revision != _proxyRevision)
            {
                _proxyRevision = revision;
                _ = LoadProxiesAsync(revision);
            }
        }
        else if (_proxyRevision is not null)
        {
            _proxyRevision = null;
            RoutedProxy = null;
            SetProxies([]);
        }
    }

    /// <summary>One result per node per run; results of another method (a stale run) are ignored.</summary>
    internal void OnProbeResult(ProbeResult result)
    {
        if (result.Method != Method) return;
        var item = Items.FirstOrDefault(i => i.Id == result.NodeId);
        if (item is null) return;
        if (result.Success && result.LatencyMs is { } latency) item.SetLatency(latency);
        else item.SetFailure(result.Error?.Code ?? ErrorCode.ProbeFailed);
    }

    void ReloadCatalog(ClientSnapshot snapshot)
    {
        var nodes = _main.Backend.Nodes();
        var existing = Items.ToDictionary(i => i.Id);
        var selectedId = SelectedItem?.Id;
        Items.Clear();
        foreach (var node in nodes)
        {
            if (existing.TryGetValue(node.Id, out var item)) item.Update(node);
            else item = new NodeItemViewModel(this, node, _strings, _services);
            item.IsCurrent = node.Id == snapshot.SelectedNodeId;
            item.SetProxy(_proxies.GetValueOrDefault(node.Id));
            Items.Add(item);
        }
        NodeCountText = _strings.Format("nodeCount", ("n", Items.Count));
        SelectedItem = Items.FirstOrDefault(i => i.Id == selectedId) ?? Items.FirstOrDefault(i => i.IsCurrent);
        CatalogChanged?.Invoke();
    }

    async Task LoadProxiesAsync(string revision)
    {
        try
        {
            var proxies = await _main.Backend.LocalProxies();
            LocalProxy? routed = null;
            try
            {
                routed = await _main.Backend.RoutedLocalProxy();
            }
            catch (Exception error)
            {
                _main.Log.Warn($"routed_local_proxy failed: {ErrorMessages.Describe(error)}");
            }
            // The core stopped, failed or moved on meanwhile: these proxies are stale.
            if (revision != _proxyRevision) return;
            RoutedProxy = routed;
            SetProxies(proxies);
        }
        catch (Exception error)
        {
            _main.Log.Warn($"local_proxies failed: {ErrorMessages.Describe(error)}");
        }
    }

    void SetProxies(IEnumerable<LocalProxy> proxies)
    {
        _proxies = proxies.ToDictionary(p => p.NodeId);
        foreach (var item in Items) item.SetProxy(_proxies.GetValueOrDefault(item.Id));
        OnPropertyChanged(nameof(SelectedNodeProxy));
        CatalogChanged?.Invoke();
    }

    void Clear()
    {
        Items.Clear();
        _proxies = [];
        RoutedProxy = null;
        _proxyRevision = null;
        SelectedItem = null;
        IsProbing = false;
        NodeCountText = _strings.Format("nodeCount", ("n", 0));
        CatalogChanged?.Invoke();
    }
}

/// <summary>One table row (and a current-node combo / tray submenu item).</summary>
public sealed partial class NodeItemViewModel : ObservableObject
{
    readonly NodesViewModel _owner;
    readonly ILocalizer _strings;
    readonly IAppServices _services;

    internal NodeItemViewModel(NodesViewModel owner, Node node, ILocalizer strings, IAppServices services)
    {
        _owner = owner;
        _strings = strings;
        _services = services;
        this.node = node;
    }

    [ObservableProperty]
    [NotifyPropertyChangedFor(nameof(Id), nameof(Name), nameof(CountryCode), nameof(Tier), nameof(HasTier), nameof(Region), nameof(Routes), nameof(Lines), nameof(HasLineChoice))]
    Node node;

    public string Id => Node.Id;
    public string Name => Node.Name;
    /// <summary>ISO country code for the flag, lower case ("hk").</summary>
    public string CountryCode => (Node.ExitCountryCode ?? "").ToLowerInvariant();
    /// <summary>Entry tier chip (the node's entry label, e.g. "专线"); empty for none.</summary>
    public string Tier => Node.EntryLabel ?? "";
    public bool HasTier => Tier.Length > 0;
    public string Region => Node.ExitRegion ?? "";
    /// <summary>
    /// Lines in failover order: labels, else <c>routeN</c> by position ("HKG-A → 线路 2"); empty for
    /// a single unlabelled line.
    /// </summary>
    public string Routes => Formatting.Routes(Node, _strings);

    /// <summary>The current node (✓ + weight 600).</summary>
    [ObservableProperty] bool isCurrent;

    // --- line (ingress) pinning ------------------------------------------------------

    /// <summary>
    /// The line picker: <c>lineAuto</c> (automatic failover) then each line in failover order,
    /// named like <see cref="Routes"/>. The same list instance until the lines change: a new list
    /// resets a view's selection (and a two-way binding then writes that reset back).
    /// </summary>
    public IReadOnlyList<LineOption> Lines => _lines ??= BuildLines(Node);

    IReadOnlyList<LineOption>? _lines;

    IReadOnlyList<LineOption> BuildLines(Node value) =>
    [
        new(null, _strings.Get("lineAuto")),
        .. value.Replicas.OrderBy(r => r.ReplicaOrdinal)
            .Select(r => new LineOption(r.EndpointKey, Formatting.LineName(value, r.EndpointKey, _strings) ?? "")),
    ];

    partial void OnNodeChanging(Node value)
    {
        // A refresh with the same lines keeps the list (records compare by value).
        if (_lines is not null && !BuildLines(value).SequenceEqual(_lines)) _lines = null;
    }

    /// <summary>More than one line: a pin changes something (single-line nodes hide the picker).</summary>
    public bool HasLineChoice => Node.Replicas.Length > 1;

    /// <summary>The picker's value, two-way: choosing a line pins it, <c>lineAuto</c> unpins.</summary>
    [ObservableProperty] LineOption? selectedLine;

    /// <summary>The pinned line; null while automatic.</summary>
    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsPinned))] string? pinnedEndpointKey;
    public bool IsPinned => PinnedEndpointKey is not null;

    /// <summary>The pinned line is down (the core's check): it stays pinned, the overview says so.</summary>
    [ObservableProperty] bool pinnedUnavailable;

    /// <summary>The line carrying the node's latest connection, as the core reports it.</summary>
    [ObservableProperty] string? activeEndpointKey;

    /// <summary>Context menu / picker: pin to <paramref name="option"/>'s line (null key: automatic).</summary>
    [RelayCommand]
    Task PinAsync(LineOption? option) => _owner.PinAsync(this, option?.EndpointKey);

    bool _applyingIngress;

    partial void OnSelectedLineChanged(LineOption? value)
    {
        if (_applyingIngress || value is null) return;
        _ = _owner.PinAsync(this, value.EndpointKey);
    }

    internal void ApplyIngress(string? pinned, NodeIngresses? reported)
    {
        _applyingIngress = true;
        try
        {
            PinnedEndpointKey = pinned;
            SelectedLine = Lines.FirstOrDefault(l => l.EndpointKey == pinned) ?? Lines[0];
            var ingresses = reported?.Ingresses ?? [];
            PinnedUnavailable = pinned is not null
                && ingresses.Any(i => i.EndpointKey == pinned && i.Healthy == false);
            ActiveEndpointKey = ingresses.FirstOrDefault(i => i.Active)?.EndpointKey;
        }
        finally
        {
            _applyingIngress = false;
        }
    }

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(IsTesting))] LatencyKind latencyKind;
    [ObservableProperty] string latencyText = "—";
    [ObservableProperty] StatusTone latencyTone;
    /// <summary>The error message of a failed test (tooltip).</summary>
    [ObservableProperty] string? latencyTooltip;
    public bool IsTesting => LatencyKind == LatencyKind.Testing;
    /// <summary>The last measured latency, if any.</summary>
    public uint? LatencyMs { get; private set; }

    [ObservableProperty] [NotifyPropertyChangedFor(nameof(HasProxy))] ProxyInfo? proxy;
    public bool HasProxy => Proxy is not null;

    /// <summary>Follows <see cref="NodesViewModel.CanProbe"/> (standard core Ready); also <see cref="TestOneCommand"/>'s CanExecute.</summary>
    public bool CanProbe => _owner.CanProbe;

    /// <summary>Double-click / context menu <c>setCurrent</c>; disabled for the current node.</summary>
    [RelayCommand(CanExecute = nameof(CanSetCurrent))]
    Task SetCurrentAsync() => _owner.SetCurrentAsync(this);

    bool CanSetCurrent() => !IsCurrent;

    /// <summary>Context menu <c>testOne</c>.</summary>
    [RelayCommand(CanExecute = nameof(CanProbe))]
    Task TestOneAsync() => _owner.ProbeAsync([this]);

    /// <summary>Context menu <c>copyHttp</c> (full URL with credentials).</summary>
    [RelayCommand]
    void CopyHttp()
    {
        if (Proxy is { } p) _services.CopyText(p.HttpUrl);
    }

    /// <summary>Context menu <c>copySocks</c>.</summary>
    [RelayCommand]
    void CopySocks()
    {
        if (Proxy is { } p) _services.CopyText(p.SocksUrl);
    }

    partial void OnIsCurrentChanged(bool value) => SetCurrentCommand.NotifyCanExecuteChanged();

    internal void NotifyCanProbeChanged()
    {
        OnPropertyChanged(nameof(CanProbe));
        TestOneCommand.NotifyCanExecuteChanged();
    }

    internal void Update(Node value) => Node = value;

    internal void SetProxy(LocalProxy? value)
    {
        if (value is null) Proxy = null;
        else if (Proxy?.Proxy != value) Proxy = new ProxyInfo(value, _services);
    }

    internal void BeginProbe()
    {
        LatencyKind = LatencyKind.Testing;
        LatencyText = "";
        LatencyTooltip = null;
    }

    internal void EndProbeWithoutResult()
    {
        if (LatencyKind != LatencyKind.Testing) return;
        ClearLatency();
    }

    internal void ClearLatency()
    {
        LatencyKind = LatencyKind.None;
        LatencyText = "—";
        LatencyTone = StatusTone.Neutral;
        LatencyTooltip = null;
        LatencyMs = null;
    }

    internal void SetLatency(uint milliseconds)
    {
        LatencyMs = milliseconds;
        LatencyKind = LatencyKind.Value;
        LatencyText = $"{milliseconds} ms";
        LatencyTone = Formatting.LatencyTone(milliseconds);
        LatencyTooltip = null;
    }

    internal void SetFailure(ErrorCode code)
    {
        LatencyMs = null;
        var timeout = code == ErrorCode.Timeout;
        LatencyKind = timeout ? LatencyKind.Timeout : LatencyKind.Failed;
        LatencyText = _strings.Get(timeout ? "timeout" : "failed");
        LatencyTone = timeout ? StatusTone.Neutral : StatusTone.Bad;
        LatencyTooltip = _strings.Message(code);
    }
}
