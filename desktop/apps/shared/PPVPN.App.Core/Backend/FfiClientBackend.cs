using PPVPN.Ffi;

namespace PPVPN.App.Core.Backend;

/// <summary>
/// <see cref="IClientBackend"/> over the generated <see cref="PPVPN.Ffi.Client"/> (the Rust crate).
/// The listener passed in receives callbacks on the crate's own threads; pass a
/// <see cref="ViewModels.SynchronizedListener"/> (MainViewModel does) so they are marshalled
/// to the UI thread.
/// </summary>
public sealed class FfiClientBackend : IClientBackend
{
    readonly Client _client;
    int _shutdown;

    public FfiClientBackend(ClientConfig config, PlatformHooks platform, ClientListener listener)
    {
        NativeLoader.Install();
        _client = new Client(config, platform, listener);
    }

    /// <summary>A factory for <c>MainViewModel</c>'s <c>createBackend</c> argument.</summary>
    public static Func<ClientListener, IClientBackend> Factory(ClientConfig config, PlatformHooks platform) =>
        listener => new FfiClientBackend(config, platform, listener);

    public ClientSnapshot Snapshot() => _client.Snapshot();

    public string LogDir() => _client.LogDir();
    public string PurchaseUrl() => _client.PurchaseUrl();
    public string RoutingRulesUrl() => _client.RoutingRulesUrl();

    public void NetworkChanged() => _client.NetworkChanged();

    public Task<InboxPage> Notifications(uint page, uint pageSize) => _client.Notifications(page, pageSize);
    public Task MarkNotificationRead(ulong id) => _client.MarkNotificationRead(id);
    public Task MarkNotificationUnread(ulong id) => _client.MarkNotificationUnread(id);
    public Task MarkAllNotificationsRead() => _client.MarkAllNotificationsRead();
    public PushMessage? ShownPush(ulong pushId) => _client.ShownPush(pushId);

    public Task<DeviceCode> AuthStart() => _client.AuthStart();
    public void AuthCancel() => _client.AuthCancel();
    public bool RetryCredentialRestore() => _client.RetryCredentialRestore();
    public Task Logout() => _client.Logout();

    public Task<Team[]> Teams() => _client.Teams();
    public Task SwitchTeam(string teamId) => _client.SwitchTeam(teamId);

    public Task RefreshProfile() => _client.RefreshProfile();
    public Node[] Nodes() => _client.Nodes();
    public Task SelectNode(string nodeId) => _client.SelectNode(nodeId);
    public Task Probe(ProbeMethod method, string[] nodeIds) => _client.Probe(method, nodeIds);
    public Task<LocalProxy[]> LocalProxies() => _client.LocalProxies();
    public Task<LocalProxy?> RoutedLocalProxy() => _client.RoutedLocalProxy();

    public Task SetConnectionMode(ConnectionMode mode) => _client.SetConnectionMode(mode);
    public Task SetRoutingMode(RoutingMode mode) => _client.SetRoutingMode(mode);
    public Task PinIngress(string nodeId, string? endpointKey) => _client.PinIngress(nodeId, endpointKey);
    public void DismissClearedIngressPins() => _client.DismissClearedIngressPins();
    public void DismissLocalProxyCredentialsReset() => _client.DismissLocalProxyCredentialsReset();
    public Task Connect() => _client.Connect();
    public Task Disconnect() => _client.Disconnect();
    public Task Retry() => _client.Retry();
    public Task EnhancedTakeOver() => _client.EnhancedTakeOver();
    public Task ServiceInstall() => _client.ServiceInstall();
    public Task ServiceUninstall() => _client.ServiceUninstall();
    public Task<bool> RefreshServiceInstalled() => _client.RefreshServiceInstalled();

    public Task ShutdownAsync() =>
        Interlocked.Exchange(ref _shutdown, 1) == 0
            ? Task.Run(_client.Shutdown)
            : Task.CompletedTask;

    /// <summary>Releases the native object. Call <see cref="ShutdownAsync"/> first when quitting.</summary>
    public void Dispose() => _client.Dispose();
}
