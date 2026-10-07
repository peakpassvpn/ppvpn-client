import Foundation
import PPVPNClient

/// UI-side probe state: the backend only reports finished results, the
/// spinner between request and result is ours.
public enum ProbeOutcome: Equatable, Sendable {
    case running
    case latency(milliseconds: UInt32)
    case failed(ErrorCode)
}

/// Single source of UI state. Everything here is derived from the backend's
/// snapshot and callbacks; views never hold connection state of their own.
///
/// Platform glue subclasses it: `willChange()` feeds the UI's change
/// notification, `open(_:)` opens URLs and `didUpdate(from:)` follows each
/// snapshot (dock badge, tray icon).
@MainActor
open class AppState: ClientEvents {
    public private(set) var snapshot: ClientSnapshot { willSet { willChange() } }
    public private(set) var nodes: [Node] = [] { willSet { willChange() } }
    public private(set) var teams: [Team] = [] { willSet { willChange() } }
    public private(set) var probes: [String: ProbeOutcome] = [:] { willSet { willChange() } }
    public private(set) var proxies: [String: LocalProxy] = [:] { willSet { willChange() } }
    /// The routed user of the shared port (the overview card); nil before core 0.5.12.
    public private(set) var routedProxy: LocalProxy? { willSet { willChange() } }
    public private(set) var traffic = TrafficSample(upBps: 0, downBps: 0, upTotal: 0, downTotal: 0) { willSet { willChange() } }
    /// Absolute expiry of the pending device code; the snapshot only carries
    /// a relative lifetime.
    public private(set) var deviceCodeExpiresAt: Date? { willSet { willChange() } }
    /// Loaded inbox pages, newest first.
    public private(set) var inbox: [InboxMessage] = [] { willSet { willChange() } }
    public private(set) var inboxTotal: UInt32 = 0 { willSet { willChange() } }
    public private(set) var inboxLoading = false { willSet { willChange() } }
    public var probeMethod: ProbeMethod = .icmp { willSet { willChange() } }
    /// One-time failure shown as a sheet alert on the main window.
    public var presentedError: AlertContent? { willSet { willChange() } }
    public var tab = MainTab.overview { willSet { willChange() } }
    /// Set by entry points outside the overview (menu bar, settings) that
    /// need the one-time install explanation shown there.
    public var installExplanationRequested = false { willSet { willChange() } }
    public let backend: ClientBackend
    private static let inboxPageSize: UInt32 = 20

    private var networkWatcher: NetworkChangeWatcher?

    /// `networkPath`: the system's network path (NWPathMonitor); its changes
    /// go to `backend.networkChanged()`, debounced.
    public init(backend: ClientBackend, networkPath: NetworkPathSource? = nil,
                networkDebounce: DispatchTimeInterval = .seconds(1)) {
        self.backend = backend
        snapshot = backend.snapshot()
        backend.events = self
        if let networkPath {
            let watcher = NetworkChangeWatcher(source: networkPath, debounce: networkDebounce) { [weak self] in
                Task { @MainActor in
                    clientLog.info("network changed")
                    self?.backend.networkChanged()
                }
            }
            watcher.start()
            networkWatcher = watcher
        }
    }

    // MARK: Platform hooks

    /// Called before any published state changes.
    open func willChange() {}

    /// Opens a web page or deep link.
    open func open(_ url: URL) {}

    /// Called after each snapshot has been applied.
    open func didUpdate(from previous: ClientSnapshot) {}

    // MARK: Derived state

    public var isSignedIn: Bool { snapshot.auth == .signedIn }

    public var selectedNode: Node? {
        nodes.first { $0.id == snapshot.selectedNodeId }
    }

    public var connectionMode: ConnectionMode { snapshot.connectionMode }
    public var routingMode: RoutingMode { snapshot.routingMode }

    /// Account-level states with no usable nodes; the UI shows a dedicated
    /// empty state instead of the connection controls.
    public var accessBlock: ErrorCode? {
        switch snapshot.profileStatus {
        case .noSubscription: .noSubscription
        case .subscriptionExpired: .subscriptionExpired
        case .teamDisabled: .teamDisabled
        case .loading, .ready, .invalid: nil
        }
    }

    public func openPurchasePage() {
        if let url = URL(string: backend.purchaseURL()) { open(url) }
    }

    /// Settings › routing mode: the team's routing rules on the web.
    public func openRoutingRulesPage() {
        if let url = URL(string: backend.routingRulesURL()) { open(url) }
    }

    public var profileExpiresAt: Date? {
        snapshot.profile.flatMap { ISO8601DateFormatter.parse($0.expiresAt) }
    }

    // MARK: Actions

    public func signIn() {
        perform {
            let code = try await self.backend.authStart()
            self.deviceCodeExpiresAt = .now.addingTimeInterval(TimeInterval(code.expiresInSecs))
            if !code.browserOpened, let url = URL(string: code.verificationUrl) {
                self.open(url)
            }
        }
    }

    public func cancelSignIn() { backend.authCancel() }

    public func signOut() {
        perform { try await self.backend.logout() }
    }

    public var isConnectedOrConnecting: Bool { presentation.connectSwitch != .off }

    /// Connect with the snapshot's method; in Enhanced Mode the client
    /// installs the service first when needed (the caller has shown the
    /// one-time explanation).
    public func connect() {
        perform(quiet: isShownByNotice) { try await self.backend.connect() }
    }

    /// Connects, unless Enhanced Mode still needs its one-time service
    /// install: then returns true and the caller explains it first. The
    /// system is checked before (an admin may have installed the service
    /// outside the app), so the explanation only shows when it is missing.
    public func connectUnlessInstallNeeded() async -> Bool {
        if connectionMode == .enhanced, !snapshot.serviceInstalled {
            guard await backend.refreshServiceInstalled() else { return true }
            clientLog.info("service installed outside the app; connecting")
        }
        connect()
        return false
    }

    public func disconnect() {
        perform { try await self.backend.disconnect() }
    }

    public func retry() {
        perform(quiet: isShownByNotice) { try await self.backend.retry() }
    }

    public func takeOver() {
        perform(quiet: isShownByNotice) { try await self.backend.enhancedTakeOver() }
    }

    /// Switching while connected reconnects with the new method (client side).
    public func setConnectionMode(_ mode: ConnectionMode) {
        guard mode != connectionMode else { return }
        perform(quiet: isShownByNotice) { try await self.backend.setConnectionMode(mode) }
    }

    /// Rules (default) or global; the client applies it without a reconnect.
    public func setRoutingMode(_ mode: RoutingMode) {
        guard mode != routingMode else { return }
        perform(quiet: isShownByNotice) { try await self.backend.setRoutingMode(mode) }
    }

    /// The line `node` is pinned to; nil while automatic.
    public func pinnedEndpointKey(of node: Node) -> String? {
        snapshot.ingressPins.first { $0.nodeId == node.id }?.endpointKey
    }

    /// Pins `node` to one line, or back to automatic with nil.
    public func pinIngress(_ node: Node, endpointKey: String?) {
        guard endpointKey != pinnedEndpointKey(of: node) else { return }
        perform { try await self.backend.pinIngress(nodeId: node.id, endpointKey: endpointKey) }
    }

    /// The ingressDown notice's 改回自动: the current node follows failover again.
    public func unpinCurrentNode() {
        guard let node = selectedNode else { return }
        pinIngress(node, endpointKey: nil)
    }

    /// The pinCleared notice's 好.
    public func dismissClearedIngressPins() {
        backend.dismissClearedIngressPins()
    }

    /// The proxyReset notice's 好.
    public func dismissLocalProxyCredentialsReset() {
        backend.dismissLocalProxyCredentialsReset()
    }

    public func installService() {
        perform(failureTitle: tr("installFailT")) { try await self.backend.serviceInstall() }
    }

    public func select(_ node: Node) {
        perform { try await self.backend.selectNode(id: node.id) }
    }

    public func uninstallService() {
        perform(failureTitle: tr("uninstallFailT")) { try await self.backend.serviceUninstall() }
    }

    public func switchTeam(_ team: Team) {
        guard team.id != snapshot.team?.id, team.active else { return }
        perform(failureTitle: tr("switchFailT"), failureMessage: { tr("switchFailD", ["reason": $0]) }) {
            try await self.backend.switchTeam(id: team.id)
        }
    }

    public func refreshProfile() {
        perform { try await self.backend.refreshProfile() }
    }

    public func probeAll() {
        startProbe(nodes.map(\.id))
    }

    public func probe(_ node: Node) {
        startProbe([node.id])
    }

    // MARK: Inbox

    /// False once a page came back short or added nothing new, whatever the
    /// reported total says, so paging can never spin on the same page.
    public private(set) var inboxHasMore = false { willSet { willChange() } }
    /// The last load failed; the list keeps what it already has.
    public private(set) var inboxFailed = false { willSet { willChange() } }
    /// At least one page arrived since sign-in (skeleton until then).
    public private(set) var inboxLoaded = false { willSet { willChange() } }
    private var inboxNextPage: UInt32 = 1

    public func reloadInbox() {
        // At launch the message centre can open before the session is
        // restored: load once signed in instead of failing now.
        guard isSignedIn else {
            inboxWanted = true
            return
        }
        guard !inboxLoading else { return }
        loadInbox(page: 1)
    }

    private var inboxWanted = false

    public func loadMoreInbox() {
        guard inboxHasMore, !inboxLoading else { return }
        loadInbox(page: inboxNextPage)
    }

    private enum LoadMode { case reset, more, merge }

    private func loadInbox(page: UInt32) {
        loadInbox(page == 1 ? .reset : .more, page: page)
    }

    private func loadInbox(_ requested: LoadMode, page: UInt32 = 1) {
        inboxLoading = true
        clientLog.info("inbox: load page \(page)\(requested == .merge ? " (live)" : "")")
        Task {
            defer { inboxLoading = false }
            do {
                let result = try await backend.notifications(page: page, pageSize: Self.inboxPageSize)
                inboxTotal = result.total
                var mode = requested
                // More new messages than a page: page 1 no longer joins the
                // loaded list, so start over (the window keeps what it shows).
                if mode == .merge, !inbox.isEmpty, !result.items.isEmpty,
                   !result.items.contains(where: { item in inbox.contains { $0.id == item.id } }) {
                    mode = .reset
                }
                var added = result.items.count
                switch mode {
                case .reset:
                    inbox = result.items
                    inboxNextPage = page + 1
                case .more:
                    let known = Set(inbox.map(\.id))
                    let fresh = result.items.filter { !known.contains($0.id) }
                    inbox += fresh
                    added = fresh.count
                    inboxNextPage = page + 1
                case .merge:
                    mergeFirstPage(result.items)
                }
                inboxFailed = false
                inboxLoaded = true
                inboxHasMore = mode == .merge
                    ? UInt32(inbox.count) < result.total
                    : added > 0 && result.items.count >= Int(Self.inboxPageSize) && UInt32(inbox.count) < result.total
            } catch {
                if let error = error as? ClientError { error.log("inbox") } else {
                    clientLog.error("inbox: \(error.localizedDescription)")
                }
                // A background refresh failing leaves the list as it was.
                if requested != .merge { inboxFailed = true }
            }
        }
    }

    /// Prepends the messages of a fresh page 1 that are not loaded yet and
    /// takes the read state of those that are; later pages stay.
    private func mergeFirstPage(_ items: [InboxMessage]) {
        var merged = inbox
        var insertAt = 0
        for item in items {
            if let index = merged.firstIndex(where: { $0.id == item.id }) {
                merged[index].read = item.read
            } else {
                merged.insert(item, at: insertAt)
                insertAt += 1
            }
        }
        inbox = merged
        // Everything loaded is contiguous from the top again; the next page
        // may overlap the end (known ids are skipped) but skips nothing.
        inboxNextPage = UInt32(inbox.count) / Self.inboxPageSize + 1
    }

    /// How long a rising unread count waits before page 1 is re-fetched;
    /// rises within it share one fetch.
    public var liveRefreshDelay: Duration = .seconds(1)
    private var liveRefreshScheduled = false

    /// New messages arrived (the unread count rose): once the debounce has
    /// passed, merge a fresh page 1 into a loaded list.
    private func scheduleLiveRefresh() {
        guard !liveRefreshScheduled else { return }
        liveRefreshScheduled = true
        Task {
            repeat {
                try? await Task.sleep(for: liveRefreshDelay)
                guard isSignedIn, inboxLoaded else {
                    liveRefreshScheduled = false
                    return
                }
            } while inboxLoading // another load is running: wait another round
            // Rises from now on may miss this fetch: let them schedule another.
            liveRefreshScheduled = false
            loadInbox(.merge)
        }
    }

    /// Marks a message read and follows its deep link, if any.
    public func open(_ message: InboxMessage) {
        openMessage(id: message.id, link: message.deepLink.flatMap(URL.init(string:)))
    }

    public func markRead(_ message: InboxMessage) {
        guard !message.read else { return }
        openMessage(id: message.id, link: nil)
    }

    public var messagesWindowRequested = false { willSet { willChange() } }

    /// What the message centre should show in detail after a notification
    /// click. The window shows it and clears it; opening and marking read
    /// already happened here.
    public enum DetailRequest: Equatable, Sendable {
        /// An inbox message (already marked read).
        case message(InboxMessage)
        /// A push shown read-only: not an inbox message, nothing to mark.
        case push(InboxMessage)
    }

    public var detailRequest: DetailRequest? { willSet { willChange() } }
    /// The push whose read-only detail is on screen, if any (the window
    /// reports leaving it through `pushDetailClosed()`).
    private var pushOnScreen: UInt64?
    /// A click that needs the session, made before sign-in finished.
    private var pendingActivation: (push: PushMessage, messageId: UInt64, open: Bool)?
    private static let maxSearchPages: UInt32 = 10

    /// A notification click from the push agent (ppvpn://push/<id>). The URL
    /// carries only the push id (any web page can open ppvpn:// URLs); the
    /// push is resolved through the client's record of shown pushes and
    /// nothing else is trusted:
    /// 1. an http(s) deep link opens, and its inbox message is marked read;
    /// 2. an inbox message opens in the message centre (marked read);
    /// 3. otherwise, or when that message is not found, the push shows
    ///    read-only (no inbox calls);
    /// 4. an unknown push just brings up the message centre.
    /// Before sign-in has finished, the push shows read-only and the message
    /// opens (or is marked read) once signed in; signed out drops that.
    public func activatePush(id: UInt64) {
        guard let push = backend.shownPush(id: id) else {
            messagesWindowRequested = true
            return
        }
        if let link = push.deepLink.flatMap(URL.init(string:)),
           ["http", "https"].contains(link.scheme?.lowercased()) {
            open(link)
            if let messageId = push.messageId {
                if isSignedIn {
                    markMessageRead(messageId)
                } else if snapshot.auth.isPending {
                    pendingActivation = (push, messageId, false)
                }
            }
            return
        }
        messagesWindowRequested = true
        guard let messageId = push.messageId else {
            showPush(push)
            return
        }
        if isSignedIn {
            Task { await openInboxMessage(messageId, otherwise: push) }
        } else {
            showPush(push)
            if snapshot.auth.isPending { pendingActivation = (push, messageId, true) }
        }
    }

    /// The message centre left the read-only push detail.
    public func pushDetailClosed() {
        pushOnScreen = nil
    }

    private func showPush(_ push: PushMessage) {
        pushOnScreen = push.id
        detailRequest = .push(push.readOnlyMessage)
    }

    /// Opens inbox message `id` (the loaded pages, then the server a page at
    /// a time, bounded); shows `push` read-only when it is not found.
    private func openInboxMessage(_ id: UInt64, otherwise push: PushMessage?) async {
        guard var message = await findMessage(id: id) else {
            if let push { showPush(push) }
            return
        }
        markMessageRead(id)
        message.read = true
        pushOnScreen = nil
        detailRequest = .message(message)
    }

    private func findMessage(id: UInt64) async -> InboxMessage? {
        if let message = inbox.first(where: { $0.id == id }) { return message }
        for page in UInt32(1)...Self.maxSearchPages {
            guard let result = try? await backend.notifications(page: page, pageSize: Self.inboxPageSize) else {
                return nil
            }
            if let message = result.items.first(where: { $0.id == id }) { return message }
            if result.items.count < Int(Self.inboxPageSize) { return nil }
        }
        return nil
    }

    /// Signed in after a click that needed the session.
    private func completeActivation() {
        guard let pending = pendingActivation else { return }
        pendingActivation = nil
        if pending.open, pushOnScreen == pending.push.id {
            // Still showing the push: replace it with the message.
            Task { await openInboxMessage(pending.messageId, otherwise: nil) }
        } else {
            markMessageRead(pending.messageId)
        }
    }

    private func markMessageRead(_ id: UInt64) {
        if let index = inbox.firstIndex(where: { $0.id == id }) {
            guard !inbox[index].read else { return }
            inbox[index].read = true
        }
        perform { try await self.backend.markNotificationRead(id: id) }
    }

    private func openMessage(id: UInt64, link: URL?) {
        if let index = inbox.firstIndex(where: { $0.id == id }) {
            guard !inbox[index].read || link != nil else { return }
            inbox[index].read = true
        }
        if let link { open(link) }
        perform { try await self.backend.markNotificationRead(id: id) }
    }

    public func markUnread(_ message: InboxMessage) {
        guard message.read else { return }
        if let index = inbox.firstIndex(where: { $0.id == message.id }) { inbox[index].read = false }
        perform { try await self.backend.markNotificationUnread(id: message.id) }
    }

    public func markAllRead() {
        for index in inbox.indices { inbox[index].read = true }
        perform { try await self.backend.markAllNotificationsRead() }
    }

    private func startProbe(_ ids: [String]) {
        for id in ids { probes[id] = .running }
        let method = probeMethod
        perform { try await self.backend.probe(method: method, nodeIDs: ids) }
    }

    private func reloadCatalog() {
        nodes = backend.nodes()
        probes = [:]
        clientLog.info("catalog: \(nodes.count) nodes, revision \(snapshot.profile?.revision ?? "-")")
        for node in nodes {
            clientLog.debug("node \(node.id) \(node.name) entry=\(node.entryKey) country=\(node.exitCountryCode ?? "-") region=\(node.exitRegion ?? "-") replicas=\(node.orderedReplicas.map { "\($0.replicaOrdinal):\($0.protocol)" }.joined(separator: ","))")
        }
        guard case .ready = snapshot.standard else {
            proxies = [:]
            routedProxy = nil
            return
        }
        // Background refresh: failures surface through the snapshot's
        // standard state, not as an alert.
        let standard = snapshot.standard
        Task {
            do {
                let proxies = try await backend.localProxies()
                // Optional (core 0.5.12): without it the card says the proxy is unavailable.
                var routed: LocalProxy?
                do {
                    routed = try await backend.routedLocalProxy()
                } catch {
                    clientLog.error("routed local proxy: \(error.localizedDescription)")
                }
                // The core stopped, failed or moved on meanwhile: these are stale.
                guard snapshot.standard == standard else { return }
                self.proxies = Dictionary(proxies.map { ($0.nodeId, $0) }, uniquingKeysWith: { $1 })
                routedProxy = routed
            } catch let error as ClientError {
                error.log("local proxies")
            } catch {
                clientLog.error("local proxies: \(error.localizedDescription)")
            }
        }
    }

    private func reloadTeams() {
        perform { self.teams = try await self.backend.teams() }
    }

    /// The connection is failed or contended, so its notice (reason + Retry /
    /// Take Over / Use Compatibility Mode) already shows the failure: a
    /// connection command's error is only logged, never an alert on top.
    /// Read from the backend's current snapshot, not `snapshot`: the
    /// command's error returns before the snapshot that failed it arrives.
    private func isShownByNotice() -> Bool {
        [.error, .contended].contains(backend.snapshot().connection.phase)
    }

    /// Runs an action; a failure becomes a sheet alert titled `failureTitle`,
    /// its message optionally wrapped (e.g. 「服务器拒绝了此请求：{reason}」),
    /// unless `quiet` says it is already on screen.
    private func perform(failureTitle: String = tr("errorT"),
                         failureMessage: @escaping (String) -> String = { $0 },
                         quiet: @escaping @MainActor () -> Bool = { false },
                         _ action: @escaping @MainActor () async throws -> Void) {
        Task {
            do { try await action() } catch is CancellationError {
            } catch ClientError.Cancelled {
                // Sign-in cancelled or superseded: the snapshot already says so.
            } catch let error as ClientError {
                error.log("action failed")
                guard !quiet() else { return }
                presentedError = AlertContent(title: failureTitle, message: failureMessage(error.userMessage))
            } catch {
                presentedError = AlertContent(title: failureTitle, message: failureMessage(error.localizedDescription))
            }
        }
    }

    // MARK: ClientEvents

    open func clientDidUpdate(_ snapshot: ClientSnapshot) {
        let previous = self.snapshot
        self.snapshot = snapshot
        defer { didUpdate(from: previous) }
        clientLog.debug("snapshot auth=\(String(describing: snapshot.auth.kind)) standard=\(String(describing: snapshot.standard)) mode=\(String(describing: snapshot.connectionMode)) connection=\(String(describing: snapshot.connection.phase)) error=\(snapshot.lastError?.code.key ?? "-") unread=\(snapshot.unreadNotifications)")

        if snapshot.auth == .signedIn, previous.auth != .signedIn {
            reloadTeams()
            completeActivation()
            if inboxWanted {
                inboxWanted = false
                reloadInbox()
            }
        }
        if previous.profile?.revision != snapshot.profile?.revision || previous.standard != snapshot.standard
            || previous.profileStatus != snapshot.profileStatus {
            reloadCatalog()
        }
        // New messages arrived: bring a loaded list up to date (the unread
        // count comes from the client's poll; the list only from paging).
        if inboxLoaded, snapshot.auth == .signedIn,
           snapshot.unreadNotifications > previous.unreadNotifications {
            scheduleLiveRefresh()
        }
        if snapshot.auth == .signedOut {
            pendingActivation = nil
            inboxLoaded = false
            inboxFailed = false
            inbox = []
            inboxTotal = 0
            inboxHasMore = false
            inboxNextPage = 1
            nodes = []
            teams = []
            probes = [:]
            proxies = [:]
            routedProxy = nil
        }
        if let error = snapshot.lastError, error != previous.lastError {
            error.log("snapshot")
        }
        if let reason = snapshot.connection.reason, reason != previous.connection.reason {
            reason.log("connection")
        }
        if case .awaitingBrowser = snapshot.auth {} else {
            deviceCodeExpiresAt = nil
        }
    }

    public func clientDidProbe(_ result: ProbeResult) {
        guard result.method == probeMethod else { return }
        probes[result.nodeId] = if result.success, let latency = result.latencyMs {
            .latency(milliseconds: latency)
        } else {
            .failed(result.error?.code ?? .probeFailed)
        }
    }

    public func clientDidSampleTraffic(_ sample: TrafficSample) {
        traffic = sample
    }
}

public struct AlertContent: Equatable, Sendable {
    public var title: String
    public var message: String

    public init(title: String, message: String) {
        self.title = title
        self.message = message
    }
}

public enum MainTab: String, CaseIterable, Identifiable, Sendable {
    case overview, nodes, logs
    public var id: Self { self }
    public var title: String { tr(rawValue) }
}

extension Node: @retroactive Identifiable {}
extension Team: @retroactive Identifiable {}

extension ProbeMethod: @retroactive CaseIterable {
    public static var allCases: [ProbeMethod] { [.icmp, .tcp, .connect] }
}

extension AuthState {
    /// Sign-in may still finish on its own (restoring, or the browser step).
    var isPending: Bool {
        switch self {
        case .restoring, .awaitingBrowser: true
        case .signedOut, .signedIn: false
        }
    }
}

extension PushMessage {
    /// The push as a read-only message-centre item (not an inbox message).
    var readOnlyMessage: InboxMessage {
        InboxMessage(id: id, title: title, content: body, kind: "push", eventKey: eventKey, category: category,
                     severity: severity, deepLink: deepLink, push: true, read: true, createdAt: createdAt)
    }
}

extension LocalProxy {
    /// The routed user (no node): its traffic follows domain rules, which
    /// SOCKS5 by IP misses, so its SOCKS URLs ask the proxy to resolve.
    public var isRouted: Bool { nodeId.isEmpty }
    /// The proxy resolves (socks5h) for every user: the routed user's rules need the name, and a
    /// node user resolving locally can get a poisoned answer and leaks the lookup.
    private var socksScheme: String { "socks5h" }

    /// The same port serves HTTP and SOCKS5; copies carry the credentials.
    public var httpURL: String { "http://\(username):\(password)@\(host):\(port)" }
    public var socksURL: String { "\(socksScheme)://\(username):\(password)@\(host):\(port)" }
    /// Display forms without credentials.
    public var httpAddress: String { "http://\(host):\(port)" }
    public var socksAddress: String { "\(socksScheme)://\(host):\(port)" }
}

private extension AuthState {
    /// Case name only; the device code stays out of the log.
    var kind: String {
        switch self {
        case .restoring: "restoring"
        case .signedOut: "signedOut"
        case .awaitingBrowser: "awaitingBrowser"
        case .signedIn: "signedIn"
        }
    }
}
