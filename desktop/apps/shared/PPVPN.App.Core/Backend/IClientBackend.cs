using PPVPN.Ffi;

namespace PPVPN.App.Core.Backend;

/// <summary>
/// The methods of the generated <see cref="PPVPN.Ffi.Client"/>, one to one, except that the
/// blocking <c>shutdown()</c> is only available as <see cref="ShutdownAsync"/>. <see cref="FfiClientBackend"/>
/// is the real implementation, <see cref="FakeClientBackend"/> the in-process stand-in for
/// previews and tests. Both take a <see cref="ClientConfig"/>, <see cref="PlatformHooks"/> and
/// <see cref="ClientListener"/> exactly like the Rust constructor, and restoring the saved
/// session starts immediately.
/// </summary>
/// <remarks>
/// Every method may throw <see cref="ClientException"/> (NotSignedIn, StandardNotReady,
/// Failed{code, detail}, NotImplemented, Cancelled).
/// </remarks>
public interface IClientBackend : IDisposable
{
    ClientSnapshot Snapshot();

    string LogDir();

    /// <summary>Where to buy or renew a subscription (site root + products path).</summary>
    string PurchaseUrl();

    /// <summary>The dashboard page where the team's routing rules are edited (site root + path).</summary>
    string RoutingRulesUrl();

    // --- notifications (backend inbox) -------------------------------------

    /// <summary>
    /// One page (1-based) of backend messages, newest first; <paramref name="pageSize"/> is clamped
    /// to 1–100. Also refreshes <c>Snapshot.UnreadNotifications</c>.
    /// </summary>
    Task<InboxPage> Notifications(uint page, uint pageSize);

    /// <summary>Mark one message read (idempotent); refreshes the unread count.</summary>
    Task MarkNotificationRead(ulong id);

    /// <summary>Mark one message unread again; refreshes the unread count.</summary>
    Task MarkNotificationUnread(ulong id);

    /// <summary>Mark every message up to the newest one the client has seen as read.</summary>
    Task MarkAllNotificationsRead();

    /// <summary>
    /// The push the push agent showed as OS notification <paramref name="pushId"/> (the push queue
    /// id, the only thing a notification carries), or null when unknown, older than 7 days or no
    /// longer among the 50 newest. Local file read, no network; resolves notification clicks.
    /// </summary>
    PushMessage? ShownPush(ulong pushId);

    // --- auth ------------------------------------------------------------

    /// <summary>
    /// Start browser device login and open the verification page. Polling runs internally and
    /// ends in <see cref="AuthState.SignedIn"/>, or in the previous state with <c>LastError</c>
    /// AuthExpired / AuthDenied. Starting again supersedes a running login (Cancelled).
    /// </summary>
    Task<DeviceCode> AuthStart();

    /// <summary>Abandon a running device login. A late approval is revoked.</summary>
    void AuthCancel();

    /// <summary>
    /// Read the credential store again now, after the launch restore failed with
    /// CredentialStoreLocked / CredentialStoreFailed (the crate keeps the saved login and
    /// retries in the background anyway; the platform may show its unlock prompt once for
    /// this read). False when no such retry is pending, e.g. the store failed while saving a
    /// new sign-in: offer signing in again instead.
    /// </summary>
    bool RetryCredentialRestore();

    /// <summary>Signs out; fails only when the credential could not be deleted (signed out either way).</summary>
    Task Logout();

    // --- account ---------------------------------------------------------

    /// <summary>Teams the account can switch between; also refreshes <c>Snapshot.Team</c>.</summary>
    Task<Team[]> Teams();

    /// <summary>Switch team; refetches the profile and restarts standard mode. A refused switch keeps the team.</summary>
    Task SwitchTeam(string teamId);

    // --- profile & nodes -------------------------------------------------

    /// <summary>
    /// Refetch the profile now. A team without a usable subscription fails with
    /// NoSubscription / SubscriptionExpired / TeamDisabled; those also become
    /// <c>Snapshot.ProfileStatus</c> and never <c>LastError</c>.
    /// </summary>
    Task RefreshProfile();

    /// <summary>Nodes of the current profile, in profile order; empty until loaded.</summary>
    Node[] Nodes();

    /// <summary>Choose the exit node (remembered across launches). Unknown id: NodeNotFound.</summary>
    Task SelectNode(string nodeId);

    /// <summary>
    /// Run a speed test through the standard-mode core; results stream through
    /// <see cref="ClientListener.OnProbeResult"/>. An empty list tests every node. Completes only
    /// after every requested node has reported exactly one result. Fails with StandardNotReady
    /// until <c>Snapshot.Standard</c> is Ready.
    /// </summary>
    Task Probe(ProbeMethod method, string[] nodeIds);

    /// <summary>
    /// Local proxies: one shared loopback port (7890 when free) for every node, told apart by
    /// user name. Fails with StandardNotReady until the standard core is ready.
    /// </summary>
    Task<LocalProxy[]> LocalProxies();

    /// <summary>
    /// The routed user of the same port (Profile rules, then the selected node; follows the routing
    /// mode); null when the standard core predates it (0.5.12). Fails with StandardNotReady until
    /// the standard core is ready.
    /// </summary>
    Task<LocalProxy?> RoutedLocalProxy();

    // --- connection -------------------------------------------------------------------
    // One "Connect" switch (snapshot.connection); the mode (Settings › Advanced) is Enhanced
    // (TUN, default) or Compatible (system proxy, no admin rights), snapshot.connection_mode.

    /// <summary>Choose the connection mode (persisted). While connected it reconnects in the new mode.</summary>
    Task SetConnectionMode(ConnectionMode mode);

    /// <summary>
    /// Choose rules or global routing (persisted on this device, snapshot.routing_mode). The
    /// running cores apply it at once, without a reconnect.
    /// </summary>
    Task SetRoutingMode(RoutingMode mode);

    /// <summary>
    /// Pin a node to one ingress (an endpoint key of <c>Node.Replicas</c>), or back to automatic
    /// failover with null (persisted on this device, snapshot.ingress_pins; applied live). A pinned
    /// ingress that fails stays pinned (snapshot.node_ingresses reports it). Fails with NodeNotFound.
    /// </summary>
    Task PinIngress(string nodeId, string? endpointKey);

    /// <summary>The user saw snapshot.cleared_ingress_pins (pins a profile refresh dropped).</summary>
    void DismissClearedIngressPins();

    /// <summary>
    /// Connect in the current mode. Enhanced: when the privileged service is missing it is
    /// installed first (phase WaitingPermission); a dismissed OS prompt ends in Error with reason
    /// ServiceInstallCancelled (retryable, suggest_compatible) and fails with that code.
    /// Compatible: points the OS proxy at the local endpoint; failure is Error with SystemProxyFailed.
    /// </summary>
    Task Connect();

    /// <summary>Disconnect; always ends Off.</summary>
    Task Disconnect();

    /// <summary>Reconnect from Error / Contended with a fresh session.</summary>
    Task Retry();

    /// <summary>Take the connection over from another session of this OS user (<c>ConnectionState.CanTakeOver</c>).</summary>
    Task EnhancedTakeOver();

    /// <summary>
    /// Install the privileged service without turning enhanced mode on (hint and Settings
    /// buttons) and wait until it answers; a no-op when it already answers. A dismissed OS
    /// prompt fails with <see cref="ClientException.Cancelled"/> and changes nothing.
    /// </summary>
    Task ServiceInstall();

    /// <summary>
    /// Turn enhanced mode off and uninstall the privileged service. Cancelling the admin prompt
    /// fails with ServiceUninstallCancelled.
    /// </summary>
    Task ServiceUninstall();

    /// <summary>
    /// Check on the system whether the privileged service is installed (it may have been installed
    /// outside the app) and update <see cref="ClientSnapshot.ServiceInstalled"/>; no prompt.
    /// </summary>
    Task<bool> RefreshServiceInstalled();

    /// <summary>
    /// The OS reported a network change: enhanced mode checks its data path within ~2 s instead of
    /// at the next 30 s check, and a reconnect waiting out a pause tries again now. Cheap, any state.
    /// </summary>
    void NetworkChanged();

    /// <summary>
    /// Release the enhanced-mode session and stop the standard core; call before the app quits.
    /// The native call blocks for up to ~10 s, so it runs on a thread-pool thread.
    /// </summary>
    Task ShutdownAsync();
}

public static class ClientSnapshots
{
    /// <summary>The snapshot before the first callback: restoring, nothing loaded.</summary>
    public static ClientSnapshot Initial { get; } = new(
        new AuthState.Restoring(),
        Account: null,
        Team: null,
        Profile: null,
        new ProfileStatus.Loading(),
        new StandardState.Stopped(),
        ConnectionMode.Enhanced,
        RoutingMode.Rules,
        ConnectionStates.Off,
        ServiceInstalled: false,
        SelectedNodeId: null,
        LastError: null,
        UnreadNotifications: 0,
        RuleSetsUnavailable: [],
        IngressPins: [],
        NodeIngresses: [],
        ClearedIngressPins: []);
}


public static class ConnectionStates
{
    /// <summary>Off, no reason, no path.</summary>
    public static ConnectionState Off { get; } = new(ConnectionPhase.Off, Reason: null, Retryable: false, CanTakeOver: false,
        SuggestCompatible: false, Competitor: null, ProxyWasForeign: false, new ConnectionDetail(EndpointKey: null, EndpointLabel: null, PreviousEndpointKey: null, LatencyMs: null));
}
