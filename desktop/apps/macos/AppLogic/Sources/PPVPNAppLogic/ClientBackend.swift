import Foundation
import PPVPNClient

// The UI talks to the shared Rust client (crates/ppvpn-client) through
// `ClientBackend`, using the UniFFI-generated records directly. The protocol
// exists so `PreviewBackend` can drive every screen without the crate doing
// real work (SwiftUI previews, UI iteration before a module is ported).

@MainActor
public protocol ClientEvents: AnyObject {
    func clientDidUpdate(_ snapshot: ClientSnapshot)
    func clientDidProbe(_ result: ProbeResult)
    func clientDidSampleTraffic(_ sample: TrafficSample)
}

@MainActor
public protocol ClientBackend: AnyObject {
    var events: ClientEvents? { get set }
    var logDirectory: URL { get }

    func snapshot() -> ClientSnapshot

    func authStart() async throws -> DeviceCode
    func authCancel()
    func logout() async throws

    func teams() async throws -> [Team]
    func switchTeam(id: String) async throws

    func refreshProfile() async throws
    func nodes() -> [Node]
    func selectNode(id: String) async throws
    /// Results stream through `ClientEvents.clientDidProbe`; empty = all nodes.
    func probe(method: ProbeMethod, nodeIDs: [String]) async throws
    func localProxies() async throws -> [LocalProxy]
    /// The routed user of the same port (Profile rules, then the selected
    /// node; follows the routing mode); nil when the standard core predates
    /// it (0.5.12).
    func routedLocalProxy() async throws -> LocalProxy?

    /// Connect / disconnect / retry with the snapshot's connection mode.
    func connect() async throws
    func disconnect() async throws
    func retry() async throws
    /// Persisted by the client; switching while connected reconnects.
    func setConnectionMode(_ mode: ConnectionMode) async throws
    /// Persisted by the client; applied at once, without a reconnect.
    func setRoutingMode(_ mode: RoutingMode) async throws
    /// Pins `nodeId` to one line (ingress); nil goes back to automatic failover.
    func pinIngress(nodeId: String, endpointKey: String?) async throws
    /// The user saw `snapshot.clearedIngressPins`: empty it.
    func dismissClearedIngressPins()
    /// The OS reported a network change: enhanced mode checks its data path
    /// soon, a waiting reconnect retries now. Safe in any state.
    func networkChanged()
    /// Checks on the system whether the privileged service is installed
    /// (e.g. by an admin outside the app) and updates the snapshot; no prompt.
    func refreshServiceInstalled() async -> Bool
    /// Take Enhanced Mode over from another device of this account.
    func enhancedTakeOver() async throws
    func serviceInstall() async throws
    /// Disconnects if needed, then removes the privileged service.
    func serviceUninstall() async throws

    /// Releases the enhanced session and stops the standard core; blocks up
    /// to ~10 s inside the client, so it runs off the main thread.
    func shutdown() async

    /// Where to buy a subscription; owned by the client, never built here.
    func purchaseURL() -> String
    /// The team's routing-rules page on the web (`Client.routing_rules_url`).
    func routingRulesURL() -> String

    /// Inbox, newest first; `page` starts at 1.
    func notifications(page: UInt32, pageSize: UInt32) async throws -> InboxPage
    func markNotificationRead(id: UInt64) async throws
    func markNotificationUnread(id: UInt64) async throws
    func markAllNotificationsRead() async throws

    /// A push the agent showed, by push id (a local file the client keeps).
    func shownPush(id: UInt64) -> PushMessage?
}

/// Production backend: owns the Rust `Client` and hops its background-thread
/// callbacks onto the main actor.
@MainActor
public final class RustBackend: ClientBackend {
    public weak var events: ClientEvents?
    public let logDirectory: URL
    private var client: Client!

    public init(config: ClientConfig, platform: PlatformHooks) {
        logDirectory = URL(fileURLWithPath: config.logDir, isDirectory: true)
        client = Client(config: config, platform: platform, listener: Listener(owner: self))
    }

    public func snapshot() -> ClientSnapshot { client.snapshot() }

    public func authStart() async throws -> DeviceCode { try await client.authStart() }
    public func authCancel() { client.authCancel() }
    public func logout() async throws { try await client.logout() }

    public func teams() async throws -> [Team] { try await client.teams() }
    public func switchTeam(id: String) async throws { try await client.switchTeam(teamId: id) }

    public func refreshProfile() async throws { try await client.refreshProfile() }
    public func nodes() -> [Node] { client.nodes() }
    public func selectNode(id: String) async throws { try await client.selectNode(nodeId: id) }
    public func probe(method: ProbeMethod, nodeIDs: [String]) async throws {
        try await client.probe(method: method, nodeIds: nodeIDs)
    }
    public func localProxies() async throws -> [LocalProxy] { try await client.localProxies() }
    public func routedLocalProxy() async throws -> LocalProxy? { try await client.routedLocalProxy() }

    public func connect() async throws { try await client.connect() }
    public func disconnect() async throws { try await client.disconnect() }
    public func retry() async throws { try await client.retry() }
    public func setConnectionMode(_ mode: ConnectionMode) async throws { try await client.setConnectionMode(mode: mode) }
    public func setRoutingMode(_ mode: RoutingMode) async throws { try await client.setRoutingMode(mode: mode) }
    public func pinIngress(nodeId: String, endpointKey: String?) async throws {
        try await client.pinIngress(nodeId: nodeId, endpointKey: endpointKey)
    }
    public func dismissClearedIngressPins() { client.dismissClearedIngressPins() }
    public func networkChanged() { client.networkChanged() }
    public func refreshServiceInstalled() async -> Bool { await client.refreshServiceInstalled() }
    public func enhancedTakeOver() async throws { try await client.enhancedTakeOver() }
    public func serviceInstall() async throws { try await client.serviceInstall() }

    public func serviceUninstall() async throws { try await client.serviceUninstall() }

    public func purchaseURL() -> String { client.purchaseUrl() }
    public func routingRulesURL() -> String { client.routingRulesUrl() }

    public func notifications(page: UInt32, pageSize: UInt32) async throws -> InboxPage {
        try await client.notifications(page: page, pageSize: pageSize)
    }
    public func markNotificationRead(id: UInt64) async throws { try await client.markNotificationRead(id: id) }
    public func markNotificationUnread(id: UInt64) async throws { try await client.markNotificationUnread(id: id) }
    public func markAllNotificationsRead() async throws { try await client.markAllNotificationsRead() }
    public func shownPush(id: UInt64) -> PushMessage? { client.shownPush(pushId: id) }

    public func shutdown() async {
        let client = client!
        await Task.detached(priority: .userInitiated) { client.shutdown() }.value
    }

    private final class Listener: ClientListener, @unchecked Sendable {
        private weak var owner: RustBackend?

        init(owner: RustBackend) { self.owner = owner }

        func onSnapshot(snapshot: ClientSnapshot) {
            Task { @MainActor [weak owner] in owner?.events?.clientDidUpdate(snapshot) }
        }

        func onProbeResult(result: ProbeResult) {
            Task { @MainActor [weak owner] in owner?.events?.clientDidProbe(result) }
        }

        func onTraffic(sample: TrafficSample) {
            Task { @MainActor [weak owner] in owner?.events?.clientDidSampleTraffic(sample) }
        }
    }
}
