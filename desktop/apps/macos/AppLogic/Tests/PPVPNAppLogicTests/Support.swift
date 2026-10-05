import Foundation
import PPVPNAppLogic
import PPVPNClient
import XCTest

/// Repository paths, from this file's location in the checkout.
enum Repo {
    static let root = URL(fileURLWithPath: #filePath)
        .deletingLastPathComponent() // PPVPNAppLogicTests
        .deletingLastPathComponent() // Tests
        .deletingLastPathComponent() // AppLogic
        .deletingLastPathComponent() // macos
        .deletingLastPathComponent() // apps
        .deletingLastPathComponent()
    static let strings = root.appendingPathComponent("apps/shared/PPVPN.App.Core/Strings/strings.json")
    static let macCatalog = root.appendingPathComponent("apps/macos/PPVPN/Resources/Localizable.xcstrings")
    static let sources = root.appendingPathComponent("apps/macos")
}

/// The shared strings.json plus the macOS-only `x_` keys, the same texts the
/// app's catalogs are generated from (see scripts/import-strings.py).
struct SharedStrings: Localizer {
    let texts: [String: String]

    static func load(_ language: String) throws -> SharedStrings {
        let shared = try JSONDecoder().decode([String: [String: String]].self, from: Data(contentsOf: Repo.strings))
        var texts = shared[language] ?? [:]
        let catalog = try JSONSerialization.jsonObject(with: Data(contentsOf: Repo.macCatalog)) as? [String: Any]
        let entries = catalog?["strings"] as? [String: [String: Any]] ?? [:]
        let locale = language == "zh" ? "zh-Hans" : language
        for (key, entry) in entries where key.hasPrefix("x_") {
            let unit = ((entry["localizations"] as? [String: Any])?[locale] as? [String: Any])?["stringUnit"]
            texts[key] = (unit as? [String: Any])?["value"] as? String
        }
        return SharedStrings(texts: texts)
    }

    func string(_ key: String, table: String?) -> String {
        texts[table == "Errors" ? "Error_" + key : key] ?? key
    }
}

/// Runs with the shared Chinese strings and a silent log. Set up once per
/// class: XCTest runs an async `setUp()` before `setUpWithError()`.
class LogicTestCase: XCTestCase {
    // Assigned once in `setUp()` (class), before any test runs.
    nonisolated(unsafe) private static var strings: SharedStrings?
    var zh: SharedStrings { Self.strings! }

    override class func setUp() {
        super.setUp()
        strings = try? SharedStrings.load("zh")
        Localization.localizer = strings ?? SharedStrings(texts: [:])
        AppLog.sink = { _, _ in }
    }

    override func setUpWithError() throws {
        XCTAssertNotNil(Self.strings, "could not load \(Repo.strings.path)")
    }
}

// MARK: - Fixtures

extension ClientSnapshot {
    static func fixture(
        auth: AuthState = .signedIn,
        profileStatus: ProfileStatus = .ready,
        mode: ConnectionMode = .enhanced,
        phase: ConnectionPhase = .off,
        reason: ClientErrorInfo? = nil,
        retryable: Bool = false,
        canTakeOver: Bool = false,
        suggestCompatible: Bool = false,
        endpoint: String? = nil,
        endpointLabel: String? = nil,
        previousEndpoint: String? = nil,
        latency: UInt32? = nil,
        standard: StandardState = .stopped,
        competitor: String? = nil,
        ruleSetsUnavailable: [String] = [],
        selected: String? = "hk-1",
        unread: UInt32 = 0,
        serviceInstalled: Bool = true,
        pins: [IngressPin] = [],
        ingresses: [NodeIngresses] = [],
        clearedPins: [IngressPin] = [],
        proxyReset: Bool = false
    ) -> ClientSnapshot {
        ClientSnapshot(
            auth: auth, account: nil, team: Team(id: "t-1", name: "Acme", personal: false, active: true),
            profile: nil, profileStatus: profileStatus, standard: standard, connectionMode: mode, routingMode: .rules,
            connection: ConnectionState(
                phase: phase, reason: reason, retryable: retryable, canTakeOver: canTakeOver,
                suggestCompatible: suggestCompatible,
                competitor: competitor, proxyWasForeign: false,
                detail: ConnectionDetail(endpointKey: endpoint, endpointLabel: endpointLabel,
                                         previousEndpointKey: previousEndpoint, latencyMs: latency)),
            serviceInstalled: serviceInstalled, selectedNodeId: selected, lastError: nil, unreadNotifications: unread,
            ruleSetsUnavailable: ruleSetsUnavailable,
            ingressPins: pins, nodeIngresses: ingresses, clearedIngressPins: clearedPins,
            localProxyCredentialsReset: proxyReset)
    }
}

func node(_ id: String, _ name: String, replicas: [String?] = ["A"]) -> Node {
    Node(id: id, name: name, entryKey: "cn", entryLabel: nil, exitRegion: nil, exitCountryCode: "HK", udp: true,
         replicas: replicas.enumerated().map { ordinal, label in
             Replica(endpointKey: "\(id)-\(ordinal)", replicaOrdinal: UInt32(ordinal), protocol: "vless",
                     label: label)
         })
}

func message(_ id: UInt64, read: Bool = false, link: String? = nil) -> InboxMessage {
    InboxMessage(id: id, title: "m\(id)", content: "", kind: "broadcast", eventKey: "e\(id)",
                 category: .announcement, severity: .normal, deepLink: link, push: true, read: read,
                 createdAt: "2026-09-30T00:00:00Z")
}

/// Scriptable backend: fixed snapshot, a server-side inbox, recorded calls.
@MainActor
final class StubBackend: ClientBackend {
    weak var events: ClientEvents?
    let logDirectory = URL(fileURLWithPath: NSTemporaryDirectory())
    var current = ClientSnapshot.fixture()
    var catalog: [Node] = [node("hk-1", "香港 01", replicas: ["HKG-A", "HKG-B"])]
    var serverInbox: [InboxMessage] = []
    /// Overrides the reported total (the server may over-report).
    var reportedTotal: UInt32?
    var failure: Error?
    /// What the agent recorded as shown, by push id.
    var shownPushes: [UInt64: PushMessage] = [:]
    private(set) var calls: [String] = []
    private(set) var pagesRequested: [UInt32] = []

    func push(_ snapshot: ClientSnapshot) {
        current = snapshot
        events?.clientDidUpdate(snapshot)
    }

    private func record(_ call: String) throws {
        calls.append(call)
        if let failure { throw failure }
    }

    func snapshot() -> ClientSnapshot { current }
    func authStart() async throws -> DeviceCode { try record("authStart"); throw ClientError.NotImplemented }
    func authCancel() { calls.append("authCancel") }
    func logout() async throws { try record("logout") }
    func teams() async throws -> [Team] { try record("teams"); return [] }
    func switchTeam(id: String) async throws { try record("switchTeam \(id)") }
    func refreshProfile() async throws { try record("refreshProfile") }
    func nodes() -> [Node] { catalog }
    func selectNode(id: String) async throws { try record("select \(id)") }
    func probe(method: ProbeMethod, nodeIDs: [String]) async throws { try record("probe \(nodeIDs)") }
    var proxyList: [LocalProxy] = []
    var routedProxy: LocalProxy?
    var routedFailure: Error?
    func localProxies() async throws -> [LocalProxy] { proxyList }
    func routedLocalProxy() async throws -> LocalProxy? {
        if let routedFailure { throw routedFailure }
        return routedProxy
    }
    func connect() async throws { try record("connect") }
    func disconnect() async throws { try record("disconnect") }
    func retry() async throws { try record("retry") }
    func setConnectionMode(_ mode: ConnectionMode) async throws { try record("mode \(mode)") }
    func setRoutingMode(_ mode: RoutingMode) async throws { try record("routing \(mode)") }
    func pinIngress(nodeId: String, endpointKey: String?) async throws {
        try record("pin \(nodeId) \(endpointKey ?? "auto")")
    }
    func dismissClearedIngressPins() { calls.append("dismissClearedPins") }
    func dismissLocalProxyCredentialsReset() { calls.append("dismissProxyReset") }
    func networkChanged() { calls.append("networkChanged") }
    /// What the system check finds (an admin may have installed the service).
    var serviceOnSystem = false
    func refreshServiceInstalled() async -> Bool {
        calls.append("refreshService")
        return serviceOnSystem
    }
    func enhancedTakeOver() async throws { try record("takeOver") }
    func serviceInstall() async throws { try record("install") }
    func serviceUninstall() async throws { try record("uninstall") }
    func shutdown() async {}
    func purchaseURL() -> String { "https://example.com/buy" }
    func routingRulesURL() -> String { "https://example.com/rules" }

    func notifications(page: UInt32, pageSize: UInt32) async throws -> InboxPage {
        pagesRequested.append(page)
        if let failure { throw failure }
        let start = Int(page - 1) * Int(pageSize)
        return InboxPage(items: Array(serverInbox.dropFirst(start).prefix(Int(pageSize))),
                         total: reportedTotal ?? UInt32(serverInbox.count))
    }

    func markNotificationRead(id: UInt64) async throws { try record("read \(id)") }
    func markNotificationUnread(id: UInt64) async throws { try record("unread \(id)") }
    func markAllNotificationsRead() async throws { try record("readAll") }
    func shownPush(id: UInt64) -> PushMessage? { shownPushes[id] }
}

/// `AppState` with its platform hooks recorded.
@MainActor
final class RecordingState: AppState {
    private(set) var opened: [URL] = []
    private(set) var changes = 0
    private(set) var updates = 0

    override func willChange() { changes += 1 }
    override func open(_ url: URL) { opened.append(url) }
    override func didUpdate(from previous: ClientSnapshot) { updates += 1 }
}

/// Lets the state's unstructured tasks run until `condition` holds.
@MainActor
func settle(timeout: TimeInterval = 2, until condition: () -> Bool = { false }) async {
    let deadline = Date().addingTimeInterval(timeout)
    repeat {
        for _ in 0..<20 { await Task.yield() }
        if condition() { return }
        try? await Task.sleep(nanoseconds: 1_000_000)
    } while Date() < deadline
}
