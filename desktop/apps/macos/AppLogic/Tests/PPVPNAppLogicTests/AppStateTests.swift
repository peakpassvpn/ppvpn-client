import Foundation
import PPVPNAppLogic
import PPVPNClient
import XCTest

@MainActor
final class AppStateTests: LogicTestCase {
    private var backend: StubBackend!
    private var state: RecordingState!

    override func setUp() async throws {
        backend = StubBackend()
        backend.current = .fixture(profileStatus: .loading)
        state = RecordingState(backend: backend)
        backend.push(.fixture())
    }

    // MARK: Snapshot handling

    func testChangesAndUpdatesReachTheHooks() {
        let before = (state.changes, state.updates)
        backend.push(.fixture(unread: 3))
        XCTAssertGreaterThan(state.changes, before.0)
        XCTAssertEqual(state.updates, before.1 + 1)
        XCTAssertEqual(state.snapshot.unreadNotifications, 3)
    }

    func testCatalogReloadsWithTheProfile() {
        XCTAssertEqual(state.nodes.map(\.id), ["hk-1"])
        XCTAssertEqual(state.selectedNode?.name, "香港 01")
    }

    func testSignOutClearsAccountState() async {
        backend.serverInbox = [message(1)]
        state.reloadInbox()
        await settle { self.state.inboxLoaded }
        backend.push(.fixture(auth: .signedOut))
        XCTAssertTrue(state.inbox.isEmpty)
        XCTAssertFalse(state.inboxLoaded)
        XCTAssertTrue(state.nodes.isEmpty)
        XCTAssertTrue(state.probes.isEmpty)
        XCTAssertEqual(state.inboxTotal, 0)
    }

    // MARK: Actions

    func testFailureBecomesAnAlert() async {
        backend.failure = ClientError.Failed(code: .serviceBusy, detail: "busy")
        state.installService()
        await settle { self.state.presentedError != nil }
        XCTAssertEqual(state.presentedError, AlertContent(title: tr("installFailT"),
                                                          message: ErrorCode.serviceBusy.message))
    }

    /// A failed or contended connection already shows its notice: the
    /// command's own error is logged, not alerted. The backend's current
    /// snapshot decides (the failing one may not have reached the state yet).
    func testConnectionCommandErrorsShownByTheNoticeStayQuiet() async {
        backend.failure = ClientError.Failed(code: .connectHealthCheckFailed, detail: "")
        for phase in [ConnectionPhase.error, .contended] {
            backend.current = .fixture(phase: phase)
            state.connect()
            state.retry()
            state.takeOver()
            state.setConnectionMode(.compatible)
            await settle { self.backend.calls.filter { $0.hasPrefix("mode") }.count > 0 }
            await settle(timeout: 0.1)
            XCTAssertNil(state.presentedError, "\(phase)")
        }
    }

    func testConnectionCommandErrorsAlertOtherwise() async {
        backend.failure = ClientError.Failed(code: .serviceBusy, detail: "")
        backend.current = .fixture(phase: .off)
        state.connect()
        await settle { self.state.presentedError != nil }
        XCTAssertEqual(state.presentedError?.message, ErrorCode.serviceBusy.message)
    }

    func testSwitchTeamWrapsTheReason() async {
        backend.failure = ClientError.Failed(code: .serviceBusy, detail: "")
        state.switchTeam(Team(id: "t-2", name: "B", personal: false, active: true))
        await settle { self.state.presentedError != nil }
        XCTAssertEqual(state.presentedError?.title, tr("switchFailT"))
        XCTAssertEqual(state.presentedError?.message, tr("switchFailD", ["reason": ErrorCode.serviceBusy.message]))
    }

    func testSwitchTeamIgnoresCurrentAndInactive() async {
        state.switchTeam(Team(id: "t-1", name: "Acme", personal: false, active: true))
        state.switchTeam(Team(id: "t-9", name: "Gone", personal: false, active: false))
        await settle(timeout: 0.1)
        XCTAssertEqual(backend.calls.filter { $0.hasPrefix("switchTeam") }, [])
    }

    func testCancellationIsSilent() async {
        backend.failure = ClientError.Cancelled
        state.connect()
        await settle { self.backend.calls.contains("connect") }
        await settle(timeout: 0.1)
        XCTAssertNil(state.presentedError)
    }

    func testSameConnectionModeIsANoOp() async {
        state.setConnectionMode(.enhanced)
        state.setConnectionMode(.compatible)
        await settle { self.backend.calls.contains("mode compatible") }
        XCTAssertEqual(backend.calls.filter { $0.hasPrefix("mode") }, ["mode compatible"])
    }

    func testSameRoutingModeIsANoOp() async {
        state.setRoutingMode(.rules)
        state.setRoutingMode(.global)
        await settle { self.backend.calls.contains("routing global") }
        XCTAssertEqual(backend.calls.filter { $0.hasPrefix("routing") }, ["routing global"])
    }

    func testPinningSkipsTheCurrentChoice() async {
        let node = state.selectedNode!
        state.pinIngress(node, endpointKey: nil)
        state.pinIngress(node, endpointKey: "hk-1-1")
        await settle { self.backend.calls.contains("pin hk-1 hk-1-1") }
        XCTAssertEqual(backend.calls.filter { $0.hasPrefix("pin") }, ["pin hk-1 hk-1-1"])
    }

    func testNoticeActionsUnpinAndDismiss() async {
        backend.push(.fixture(pins: [IngressPin(nodeId: "hk-1", endpointKey: "hk-1-1")]))
        state.unpinCurrentNode()
        state.dismissClearedIngressPins()
        await settle { self.backend.calls.contains("pin hk-1 auto") }
        XCTAssertTrue(backend.calls.contains("dismissClearedPins"))
    }

    // MARK: Service install explanation

    private var serviceCalls: [String] { backend.calls.filter { ["refreshService", "connect"].contains($0) } }

    func testMissingServiceAsksForTheExplanation() async {
        backend.push(.fixture(serviceInstalled: false))
        let explain = await state.connectUnlessInstallNeeded()
        XCTAssertTrue(explain)
        XCTAssertEqual(serviceCalls, ["refreshService"])
    }

    func testServiceInstalledOutsideTheAppConnects() async {
        backend.push(.fixture(serviceInstalled: false))
        backend.serviceOnSystem = true
        let explain = await state.connectUnlessInstallNeeded()
        XCTAssertFalse(explain)
        await settle { self.backend.calls.contains("connect") }
        XCTAssertEqual(serviceCalls, ["refreshService", "connect"])
    }

    func testKnownServiceOrCompatibleModeConnectsWithoutACheck() async {
        var explain = await state.connectUnlessInstallNeeded()
        XCTAssertFalse(explain)
        backend.push(.fixture(mode: .compatible, serviceInstalled: false))
        explain = await state.connectUnlessInstallNeeded()
        XCTAssertFalse(explain)
        await settle { self.backend.calls.filter { $0 == "connect" }.count == 2 }
        XCTAssertFalse(backend.calls.contains("refreshService"))
    }

    func testProbeMarksNodesRunning() {
        state.probeAll()
        XCTAssertEqual(state.probes["hk-1"], .running)
    }

    func testPurchasePageOpensTheClientURL() {
        state.openPurchasePage()
        XCTAssertEqual(state.opened, [URL(string: "https://example.com/buy")!])
    }

    func testRoutingRulesPageOpensTheClientURL() {
        state.openRoutingRulesPage()
        XCTAssertEqual(state.opened, [URL(string: "https://example.com/rules")!])
    }

    // MARK: Inbox

    func testPagingAppendsAndStops() async {
        backend.serverInbox = (1...25).reversed().map { message(UInt64($0)) }
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        XCTAssertEqual(state.inbox.count, 20)
        XCTAssertTrue(state.inboxHasMore)

        state.loadMoreInbox()
        await settle { !self.state.inboxLoading && self.state.inbox.count == 25 }
        XCTAssertEqual(state.inbox.count, 25)
        XCTAssertFalse(state.inboxHasMore)
        XCTAssertEqual(backend.pagesRequested, [1, 2])
    }

    /// A server that over-reports its total must not make paging spin.
    func testPagingStopsWhenAPageAddsNothing() async {
        backend.serverInbox = (1...20).map { message(UInt64($0)) }
        backend.reportedTotal = 100
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        XCTAssertTrue(state.inboxHasMore)
        state.loadMoreInbox()
        await settle { self.backend.pagesRequested.count == 2 && !self.state.inboxLoading }
        XCTAssertFalse(state.inboxHasMore)
        state.loadMoreInbox()
        await settle(timeout: 0.1)
        XCTAssertEqual(backend.pagesRequested, [1, 2])
    }

    func testInboxOpenedBeforeRestoreLoadsOnceSignedIn() async {
        backend.push(.fixture(auth: .restoring))
        backend.serverInbox = [message(1)]
        state.reloadInbox()
        await settle(timeout: 0.1)
        XCTAssertEqual(backend.pagesRequested, [])
        XCTAssertFalse(state.inboxFailed)
        backend.push(.fixture())
        await settle { self.state.inboxLoaded }
        XCTAssertEqual(state.inbox.map(\.id), [1])
        XCTAssertEqual(backend.pagesRequested, [1])
    }

    func testNewUnreadMessagesMergeIntoALoadedList() async {
        state.liveRefreshDelay = .milliseconds(20)
        backend.serverInbox = (1...25).reversed().map { message(UInt64($0)) }
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        state.loadMoreInbox()
        await settle { self.state.inbox.count == 25 }
        // Two new messages, one old one read elsewhere; two rises share one fetch.
        backend.serverInbox = [message(27), message(26)] + (1...25).reversed().map { message(UInt64($0), read: $0 == 24) }
        backend.push(.fixture(unread: 1))
        backend.push(.fixture(unread: 2))
        await settle { self.state.inbox.count == 27 }
        await settle(timeout: 0.1)
        XCTAssertEqual(state.inbox.prefix(3).map(\.id), [27, 26, 25])
        XCTAssertEqual(state.inbox.count, 27, "later pages stay")
        XCTAssertEqual(state.inbox.first { $0.id == 24 }?.read, true)
        XCTAssertEqual(backend.pagesRequested, [1, 2, 1])
        // Fewer unread (read elsewhere) does not reload.
        backend.push(.fixture(unread: 0))
        await settle(timeout: 0.1)
        XCTAssertEqual(backend.pagesRequested, [1, 2, 1])
    }

    func testMoreNewMessagesThanAPageStartOver() async {
        state.liveRefreshDelay = .milliseconds(20)
        backend.serverInbox = [message(1)]
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        backend.serverInbox = (100...130).reversed().map { message(UInt64($0)) } + [message(1)]
        backend.push(.fixture(unread: 31))
        await settle { self.state.inbox.first?.id == 130 }
        XCTAssertEqual(state.inbox.count, 20)
        XCTAssertTrue(state.inboxHasMore)
    }

    func testFailedLiveRefreshKeepsTheListWithoutError() async {
        state.liveRefreshDelay = .milliseconds(20)
        backend.serverInbox = [message(1)]
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        backend.failure = ClientError.Failed(code: .unreachable, detail: "")
        backend.push(.fixture(unread: 1))
        await settle { self.backend.pagesRequested.count == 2 }
        await settle(timeout: 0.1)
        XCTAssertFalse(state.inboxFailed)
        XCTAssertEqual(state.inbox.map(\.id), [1])
    }

    func testNewUnreadMessagesDoNotLoadAnUnopenedList() async {
        backend.push(.fixture(unread: 3))
        await settle(timeout: 0.1)
        XCTAssertEqual(backend.pagesRequested, [])
    }

    func testInboxFailureKeepsTheList() async {
        backend.serverInbox = [message(1)]
        state.reloadInbox()
        await settle { self.state.inboxLoaded && !self.state.inboxLoading }
        backend.failure = ClientError.Failed(code: .unreachable, detail: "")
        state.reloadInbox()
        await settle { self.state.inboxFailed }
        XCTAssertEqual(state.inbox.map(\.id), [1])
        XCTAssertNil(state.presentedError)
    }

    func testReadAndUnread() async {
        backend.serverInbox = [message(2, link: "https://example.com/a"), message(1, read: true)]
        state.reloadInbox()
        await settle { self.state.inboxLoaded }
        state.markRead(state.inbox[1]) // already read: nothing to do
        state.open(state.inbox[0])
        XCTAssertTrue(state.inbox[0].read)
        XCTAssertEqual(state.opened, [URL(string: "https://example.com/a")!])
        state.markUnread(state.inbox[0])
        XCTAssertFalse(state.inbox[0].read)
        state.markAllRead()
        XCTAssertTrue(state.inbox.allSatisfy(\.read))
        await settle { self.backend.calls.contains("readAll") }
        XCTAssertEqual(backend.calls, ["read 2", "unread 2", "readAll"])
    }

    // MARK: ppvpn://push/<id>

    private func push(_ id: UInt64, link: String? = nil, message: UInt64? = nil) -> PushMessage {
        let push = PushMessage(id: id, messageId: message, title: "t\(id)", body: "b\(id)", severity: .important,
                               category: .route, eventKey: "proxy.chain_unhealthy", deepLink: link,
                               createdAt: "2026-09-30T00:00:00Z")
        backend.shownPushes[id] = push
        return push
    }

    private var requestedPush: InboxMessage? {
        if case .push(let message) = state.detailRequest { return message }
        return nil
    }

    private var requestedMessage: InboxMessage? {
        if case .message(let message) = state.detailRequest { return message }
        return nil
    }

    /// (a) A web link opens and its inbox message is marked read.
    func testLinkOpensAndMarksItsMessageRead() async {
        _ = push(8, link: "https://example.com/p", message: 40)
        state.activatePush(id: 8)
        XCTAssertEqual(state.opened, [URL(string: "https://example.com/p")!])
        XCTAssertFalse(state.messagesWindowRequested)
        await settle { self.backend.calls.contains("read 40") }
        XCTAssertEqual(backend.calls, ["read 40"])
    }

    /// Only http(s) links are followed.
    func testNonWebLinkIsNotOpened() {
        _ = push(9, link: "file:///etc/passwd")
        state.activatePush(id: 9)
        XCTAssertTrue(state.opened.isEmpty)
        XCTAssertEqual(requestedPush?.id, 9)
    }

    /// (b) The push's inbox message opens (searched beyond the loaded pages).
    func testInboxMessageOpensAndIsMarkedRead() async {
        backend.serverInbox = (1...30).reversed().map { message(UInt64($0)) }
        _ = push(7, message: 3)
        state.activatePush(id: 7)
        XCTAssertTrue(state.messagesWindowRequested)
        await settle { self.state.detailRequest != nil }
        XCTAssertEqual(requestedMessage?.id, 3)
        XCTAssertEqual(requestedMessage?.read, true)
        await settle { self.backend.calls.contains("read 3") }
        XCTAssertEqual(backend.calls, ["read 3"])
        XCTAssertEqual(backend.pagesRequested, [1, 2])
    }

    /// (b→c) A message the server does not have falls back to the push.
    func testMissingInboxMessageShowsThePush() async {
        backend.serverInbox = [message(1)]
        _ = push(7, message: 99)
        state.activatePush(id: 7)
        await settle { self.state.detailRequest != nil }
        XCTAssertEqual(requestedPush?.title, "t7")
        XCTAssertEqual(backend.calls, [])
    }

    /// (c) A push with neither shows read-only, touching no inbox API.
    func testPlainPushShowsReadOnly() throws {
        _ = push(7)
        state.activatePush(id: 7)
        XCTAssertTrue(state.messagesWindowRequested)
        let shown = try XCTUnwrap(requestedPush)
        XCTAssertEqual(shown.content, "b7")
        XCTAssertEqual(shown.category, .route)
        XCTAssertEqual(shown.severity, .important)
        XCTAssertTrue(shown.read)
        XCTAssertEqual(backend.calls, [])
    }

    /// (d) An unknown push only brings up the message centre.
    func testUnknownPushOnlyOpensTheMessageCentre() {
        state.activatePush(id: 99)
        XCTAssertTrue(state.messagesWindowRequested)
        XCTAssertNil(state.detailRequest)
        XCTAssertTrue(state.opened.isEmpty)
    }

    /// (e) Clicked before sign-in finished: the push shows now, the message
    /// replaces it once signed in.
    func testBeforeSignInShowsThePushThenTheMessage() async {
        backend.push(.fixture(auth: .restoring))
        backend.serverInbox = [message(3)]
        _ = push(7, message: 3)
        state.activatePush(id: 7)
        XCTAssertEqual(requestedPush?.id, 7)
        XCTAssertEqual(backend.pagesRequested, [])
        state.detailRequest = nil // the window showed it
        backend.push(.fixture())
        await settle { self.state.detailRequest != nil }
        XCTAssertEqual(requestedMessage?.id, 3)
        await settle { self.backend.calls.contains("read 3") }
    }

    /// (e) The push was left before sign-in finished: only mark it read.
    func testBeforeSignInLeftPushOnlyMarksRead() async {
        backend.push(.fixture(auth: .restoring))
        _ = push(7, message: 3)
        state.activatePush(id: 7)
        state.detailRequest = nil
        state.pushDetailClosed()
        backend.push(.fixture())
        await settle { self.backend.calls.contains("read 3") }
        XCTAssertNil(state.detailRequest)
        XCTAssertEqual(backend.pagesRequested, [])
    }

    /// (e) A link clicked before sign-in marks its message read afterwards.
    func testBeforeSignInLinkMarksReadAfterwards() async {
        backend.push(.fixture(auth: .restoring))
        _ = push(8, link: "https://example.com/p", message: 40)
        state.activatePush(id: 8)
        XCTAssertEqual(state.opened.count, 1)
        XCTAssertEqual(backend.calls, [])
        backend.push(.fixture())
        await settle { self.backend.calls.contains("read 40") }
    }

    /// (e) Ending up signed out drops the pending activation.
    func testSignedOutDropsThePendingActivation() async {
        backend.push(.fixture(auth: .restoring))
        _ = push(7, message: 3)
        state.activatePush(id: 7)
        backend.push(.fixture(auth: .signedOut))
        backend.push(.fixture())
        await settle(timeout: 0.2)
        XCTAssertEqual(backend.calls.filter { $0.hasPrefix("read") }, [])
    }
}
