import Foundation
import PPVPNClient

/// In-process stand-in for ppvpn-client: drives every UI state with fake data.
/// Selected with `PPVPN_PREVIEW=1` in Debug builds and used by SwiftUI previews.
@MainActor
public final class PreviewBackend: ClientBackend {
    public weak var events: ClientEvents? {
        didSet { events?.clientDidUpdate(state) }
    }

    public let logDirectory = FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
        .appending(path: "Logs/PPVPN", directoryHint: .isDirectory)

    private var state = ClientSnapshot(
        auth: .restoring, account: nil, team: nil, profile: nil, profileStatus: .loading, standard: .stopped,
        connectionMode: .enhanced, routingMode: .rules, connection: PreviewBackend.idle, serviceInstalled: false,
        selectedNodeId: nil, lastError: nil, unreadNotifications: 0, ruleSetsUnavailable: [],
        ingressPins: [], nodeIngresses: [], clearedIngressPins: [], localProxyCredentialsReset: false
    ) {
        didSet { events?.clientDidUpdate(state) }
    }
    private static let idle = ConnectionState(
        phase: .off, reason: nil, retryable: false, canTakeOver: false, suggestCompatible: false,
        competitor: nil, proxyWasForeign: false,
        detail: ConnectionDetail(endpointKey: nil, endpointLabel: nil, previousEndpointKey: nil, latencyMs: nil))
    private static func scenario(_ name: String) -> ConnectionState {
        var state = idle
        switch name {
        case "connecting":
            state.phase = .connecting
        case "occupied":
            (state.phase, state.canTakeOver) = (.contended, true)
            state.reason = ClientErrorInfo(code: .serviceBusy, detail: "preview")
        case "path":
            (state.phase, state.retryable, state.suggestCompatible) = (.contended, true, true)
            state.reason = ClientErrorInfo(code: .networkPathContended, detail: "NETWORK_PATH_CONTENDED")
        case "occupied-user":
            state.phase = .contended
            state.reason = ClientErrorInfo(code: .serviceOwnedByAnotherUser, detail: "CONNECTION_OWNED_BY_ANOTHER_USER")
        case "failed":
            (state.phase, state.retryable) = (.error, true)
            state.reason = ClientErrorInfo(code: .connectFailed, detail: "preview")
        case "failed-install":
            (state.phase, state.retryable, state.suggestCompatible) = (.error, true, true)
            state.reason = ClientErrorInfo(code: .serviceInstallCancelled, detail: "preview")
        case "surge":
            // Surge's enhanced mode holding the path (ConnectHealthCheckFailed).
            (state.phase, state.retryable, state.suggestCompatible) = (.error, true, true)
            state.reason = ClientErrorInfo(code: .connectHealthCheckFailed, detail: "preview")
            state.competitor = "Surge"
        case "admin":
            (state.phase, state.retryable) = (.error, true)
            state.reason = ClientErrorInfo(code: .systemProxyFailed, detail: "ADMIN_REQUIRED: preview")
        default:
            break
        }
        return state
    }

    private var signedInDelay = 4.0
    private var loginTask: Task<Void, Never>?
    private var trafficTask: Task<Void, Never>?
    private var totals = (up: UInt64(0), down: UInt64(0))

    private let sampleTeams = [
        Team(id: "t-personal", name: "个人", personal: true, active: true),
        Team(id: "t-acme", name: "Acme 团队", personal: false, active: true),
        Team(id: "t-gone", name: "已解散团队", personal: false, active: false),
    ]
    private let sampleNodes: [Node] = [
        node("hk-1", "香港 01", "香港", "HK", ["vless-reality", "hysteria2"], tier: "专线"),
        node("jp-1", "东京 01", "日本", "JP", ["vless-reality"]),
        node("sg-1", "新加坡 01", "新加坡", "SG", ["hysteria2"], tier: "优化"),
        node("us-1", "洛杉矶 01", "美国", "US", ["vless-reality", "trojan"]),
    ]

    /// `signedIn: true` skips the device-login dance (PPVPN_PREVIEW=signed-in);
    /// `team` switches right after (t-acme has no subscription).
    public init(signedIn: Bool = false, team: String? = nil) {
        Task {
            try? await Task.sleep(for: .milliseconds(400))
            state.auth = .signedOut
            if signedIn {
                signedInDelay = 0
                _ = try? await authStart()
                signedInDelay = 4
                if let team {
                    try? await Task.sleep(for: .milliseconds(1200))
                    try? await switchTeam(id: team)
                }
            }
        }
    }

    public func snapshot() -> ClientSnapshot { state }

    public func authStart() async throws -> DeviceCode {
        let code = DeviceCode(userCode: "SMDR-M4VY",
                              verificationUrl: "https://example.com/dashboard/device/authorize?code=SMDR-M4VY",
                              expiresInSecs: 600, browserOpened: false)
        state.auth = .awaitingBrowser(code: code)
        loginTask?.cancel()
        let delay = signedInDelay
        loginTask = Task {
            try? await Task.sleep(for: .seconds(delay))
            guard !Task.isCancelled else { return }
            state.auth = .signedIn
            state.account = Account(id: "u-1", name: "Jerry", avatarUrl: nil, email: "jerry@example.com")
            state.team = sampleTeams[0]
            state.profileStatus = .ready
            state.profile = ProfileSummary(
                revision: "rev-1",
                expiresAt: ISO8601DateFormatter().string(from: .now.addingTimeInterval(86_400 * 23)),
                nodeCount: UInt32(sampleNodes.count))
            state.selectedNodeId = sampleNodes.first?.id
            state.unreadNotifications = UInt32(inbox.filter { !$0.read }.count)
            state.standard = .starting
            try? await Task.sleep(for: .milliseconds(600))
            state.standard = .ready(revision: "rev-1")
            // PPVPN_PREVIEW_CONNECTION=connecting|occupied|occupied-user|path|surge|failed|failed-install|admin
            // puts the connection into that state for screenshots.
            if let scenario = ProcessInfo.processInfo.environment["PPVPN_PREVIEW_CONNECTION"] {
                state.connection = Self.scenario(scenario)
            }
            // PPVPN_PREVIEW_RULES=1: some routing rule sets not loaded yet.
            if ProcessInfo.processInfo.environment["PPVPN_PREVIEW_RULES"] == "1" {
                state.ruleSetsUnavailable = ["geosite-cn"]
            }
        }
        return code
    }

    public func authCancel() {
        loginTask?.cancel()
        state.auth = .signedOut
    }

    public func logout() async throws {
        stopTraffic()
        state.auth = .signedOut
        state.account = nil
        state.team = nil
        state.profile = nil
        state.standard = .stopped
        state.connection = Self.idle
        state.selectedNodeId = nil
    }

    public func teams() async throws -> [Team] { sampleTeams }

    /// The non-personal preview team has no subscription.
    public func switchTeam(id: String) async throws {
        guard let team = sampleTeams.first(where: { $0.id == id }) else { return }
        state.team = team
        if team.personal {
            state.selectedNodeId = sampleNodes.first?.id
            state.profileStatus = .ready
            state.profile = ProfileSummary(
                revision: "rev-1",
                expiresAt: ISO8601DateFormatter().string(from: .now.addingTimeInterval(86_400 * 23)),
                nodeCount: UInt32(sampleNodes.count))
            state.standard = .ready(revision: "rev-1")
        } else {
            state.profileStatus = .noSubscription
            state.profile = nil
            state.standard = .stopped
            state.selectedNodeId = nil
        }
    }

    public func refreshProfile() async throws {}

    public func nodes() -> [Node] { state.profileStatus == .ready ? sampleNodes : [] }

    public func selectNode(id: String) async throws { state.selectedNodeId = id }

    public func probe(method: ProbeMethod, nodeIDs: [String]) async throws {
        for id in nodeIDs.isEmpty ? sampleNodes.map(\.id) : nodeIDs {
            Task {
                try? await Task.sleep(for: .milliseconds(Int.random(in: 200...1500)))
                let failed = Int.random(in: 0..<8) == 0
                events?.clientDidProbe(ProbeResult(
                    nodeId: id, method: method, success: !failed,
                    latencyMs: failed ? nil : UInt32.random(in: 20...320),
                    endpointKey: nil,
                    error: failed ? ClientErrorInfo(code: .timeout, detail: "preview") : nil))
            }
        }
    }

    public func localProxies() async throws -> [LocalProxy] {
        sampleNodes.enumerated().map { index, node in
            LocalProxy(nodeId: node.id, host: "127.0.0.1", port: UInt16(17890 + index),
                       username: "ppvpn", password: "preview")
        }
    }

    public func routedLocalProxy() async throws -> LocalProxy? {
        LocalProxy(nodeId: "", host: "127.0.0.1", port: 17890, username: "ppvpn", password: "preview")
    }

    public func connect() async throws {
        let node = sampleNodes.first { $0.id == state.selectedNodeId }
        let route = node?.replicas.first?.endpointKey
        state.connection = ConnectionState(
            phase: .connecting, reason: nil, retryable: false, canTakeOver: false, suggestCompatible: false,
        competitor: nil, proxyWasForeign: false,
            detail: ConnectionDetail(endpointKey: route, endpointLabel: nil, previousEndpointKey: nil, latencyMs: nil))
        try await Task.sleep(for: .seconds(1))
        if state.connectionMode == .enhanced { state.serviceInstalled = true }
        state.connection.phase = .on
        state.connection.detail.latencyMs = 38
        startTraffic()
    }

    public func disconnect() async throws {
        stopTraffic()
        state.connection = Self.idle
    }

    public func retry() async throws { try await connect() }

    public func setConnectionMode(_ mode: ConnectionMode) async throws {
        let reconnect = state.connection.phase != .off
        if reconnect { try await disconnect() }
        state.connectionMode = mode
        if reconnect { try await connect() }
    }

    public func setRoutingMode(_ mode: RoutingMode) async throws { state.routingMode = mode }

    public func pinIngress(nodeId: String, endpointKey: String?) async throws {
        state.ingressPins.removeAll { $0.nodeId == nodeId }
        if let endpointKey { state.ingressPins.append(IngressPin(nodeId: nodeId, endpointKey: endpointKey)) }
    }

    public func dismissClearedIngressPins() { state.clearedIngressPins = [] }

    public func dismissLocalProxyCredentialsReset() { state.localProxyCredentialsReset = false }

    public func networkChanged() {}

    public func refreshServiceInstalled() async -> Bool { state.serviceInstalled }

    public func enhancedTakeOver() async throws { try await connect() }

    public func serviceInstall() async throws { state.serviceInstalled = true }

    public func serviceUninstall() async throws {
        try await disconnect()
        state.serviceInstalled = false
    }

    public func shutdown() async {}

    public func purchaseURL() -> String { "https://example.com/dashboard/products" }
    public func routingRulesURL() -> String { "https://example.com/dashboard/routing-rules" }

    private lazy var inbox: [InboxMessage] = (1...27).reversed().map(previewMessage)

    public func notifications(page: UInt32, pageSize: UInt32) async throws -> InboxPage {
        let start = Int(max(page, 1) - 1) * Int(pageSize)
        let items = Array(inbox.dropFirst(start).prefix(Int(pageSize)))
        return InboxPage(items: items, total: UInt32(inbox.count))
    }

    public func markNotificationRead(id: UInt64) async throws {
        if let index = inbox.firstIndex(where: { $0.id == id }) { inbox[index].read = true }
        state.unreadNotifications = UInt32(inbox.filter { !$0.read }.count)
    }

    public func markNotificationUnread(id: UInt64) async throws {
        if let index = inbox.firstIndex(where: { $0.id == id }) { inbox[index].read = false }
        state.unreadNotifications = UInt32(inbox.filter { !$0.read }.count)
    }

    public func shownPush(id: UInt64) -> PushMessage? { nil }

    public func markAllNotificationsRead() async throws {
        for index in inbox.indices { inbox[index].read = true }
        state.unreadNotifications = 0
    }

    private func startTraffic() {
        trafficTask?.cancel()
        trafficTask = Task {
            while !Task.isCancelled {
                let up = UInt64.random(in: 2_000...80_000)
                let down = UInt64.random(in: 20_000...2_400_000)
                totals.up += up
                totals.down += down
                events?.clientDidSampleTraffic(TrafficSample(
                    upBps: up, downBps: down, upTotal: totals.up, downTotal: totals.down))
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    private func stopTraffic() {
        trafficTask?.cancel()
        events?.clientDidSampleTraffic(TrafficSample(upBps: 0, downBps: 0, upTotal: totals.up, downTotal: totals.down))
    }
}

private func node(_ id: String, _ name: String, _ region: String, _ country: String, _ protocols: [String],
                  tier: String? = nil) -> Node {
    Node(id: id, name: name, entryKey: "cn-optimized", entryLabel: tier,
         exitRegion: region, exitCountryCode: country, udp: true,
         replicas: protocols.enumerated().map { ordinal, proto in
             Replica(endpointKey: "\(id)-\(ordinal)", replicaOrdinal: UInt32(ordinal), protocol: proto,
                     label: "\(id.prefix(2).uppercased())G-\(Character(UnicodeScalar(65 + ordinal)!))")
         })
}

private func previewMessage(_ n: Int) -> InboxMessage {
    let latest = n == 27
    let age = TimeInterval(3_600 * (27 - n))
    return InboxMessage(
        id: UInt64(n),
        title: latest ? "订阅即将到期" : "系统公告 \(n)",
        content: latest ? "你的订阅将在 3 天后到期，续费后不影响使用。" : "这是第 \(n) 条预览消息。",
        kind: latest ? "subscription" : ["broadcast", "invoice", "order", "proxy"][n % 4],
        eventKey: latest ? "subscription.expire_reminder_3d" : "preview.\(n)",
        category: latest ? .subscriptionExpiring : [.announcement, .billing, .order, .route][n % 4],
        severity: latest ? .critical : n == 26 ? .important : .normal,
        deepLink: n % 3 == 0 ? "https://example.com/dashboard" : nil,
        push: true,
        read: n < 24,
        createdAt: ISO8601DateFormatter().string(from: Date.now.addingTimeInterval(-age)))
}
