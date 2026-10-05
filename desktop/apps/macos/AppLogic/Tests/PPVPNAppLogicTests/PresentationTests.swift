import PPVPNAppLogic
import PPVPNClient
import XCTest

@MainActor
final class PresentationTests: LogicTestCase {
    private var backend: StubBackend!
    private var state: RecordingState!

    override func setUp() async throws {
        backend = StubBackend()
        backend.current = .fixture(profileStatus: .loading)
        state = RecordingState(backend: backend)
        backend.push(.fixture())
    }

    private func show(_ snapshot: ClientSnapshot) -> ConnectionPresentation {
        backend.push(snapshot)
        return state.presentation
    }

    func testIdle() {
        let p = show(.fixture())
        XCTAssertEqual(p.tone, .idle)
        XCTAssertEqual(p.tray, .off)
        XCTAssertEqual(p.connectSwitch, .off)
        XCTAssertTrue(p.connectSwitchEnabled)
        XCTAssertEqual(p.headline, tr("h_idle"))
        XCTAssertEqual(p.statusLine, tr("h_idle"))
        XCTAssertTrue(p.notices.isEmpty)
    }

    func testConnectedShowsLiveRouteAndLatency() {
        let p = show(.fixture(phase: .on, endpoint: "hk-1-1", latency: 38))
        XCTAssertEqual(p.tone, .ok)
        XCTAssertEqual(p.tray, .on)
        XCTAssertEqual(p.connectSwitch, .on)
        XCTAssertEqual(p.detail, tr("d_on", ["r": "HKG-B", "ms": "38 ms"]))
    }

    func testRouteFallsBackToFirstReplica() {
        XCTAssertEqual(show(.fixture(phase: .connecting)).detail, tr("d_connecting", ["r": "HKG-A"]))
    }

    // MARK: Routes never show endpoint keys

    func testLiveEndpointShowsItsLabelNeverItsKey() {
        XCTAssertEqual(show(.fixture(phase: .on, endpoint: "hk-1-1", latency: 38)).detail,
                       tr("d_on", ["r": "HKG-B", "ms": "38 ms"]))
        XCTAssertEqual(show(.fixture(phase: .on, endpoint: "hk-1-9", endpointLabel: "Live", latency: 38)).detail,
                       tr("d_on", ["r": "Live", "ms": "38 ms"]))
    }

    func testUnlabelledRouteIsLeftOut() {
        backend.catalog = [node("hk-1", "香港 01", replicas: [nil, nil])]
        backend.push(.fixture(profileStatus: .loading))
        backend.push(.fixture())
        XCTAssertEqual(show(.fixture(phase: .on, endpoint: "hk-1-0", latency: 38)).detail, "38 ms")
        XCTAssertEqual(show(.fixture(mode: .compatible, phase: .on, endpoint: "hk-1-0", latency: 38)).detail, "38 ms")
        XCTAssertEqual(show(.fixture(phase: .connecting)).detail, "")
        XCTAssertEqual(show(.fixture(phase: .reconnecting, endpoint: "hk-1-1", previousEndpoint: "hk-1-0")).detail, "")
        XCTAssertFalse(show(.fixture(phase: .on, endpoint: "hk-1-0", latency: 38)).detail.contains("hk-1"))
    }

    func testReconnectingNamesBothLabelledLines() {
        XCTAssertEqual(show(.fixture(phase: .reconnecting, endpoint: "hk-1-1", previousEndpoint: "hk-1-0")).detail,
                       tr("d_reconnecting", ["r0": "HKG-A", "r": "HKG-B"]))
        // An unknown (or unlabelled) previous line: only the new one.
        XCTAssertEqual(show(.fixture(phase: .reconnecting, endpoint: "hk-1-1", previousEndpoint: "gone")).detail,
                       tr("d_connecting", ["r": "HKG-B"]))
    }

    // MARK: Line (ingress) pinning

    func testLineNamesUseLabelsThenPositions() {
        let n = node("n", "N", replicas: ["HKG-A", nil])
        XCTAssertTrue(n.hasLineChoice)
        XCTAssertEqual(n.lineName("n-0"), "HKG-A")
        XCTAssertEqual(n.lineName("n-1"), tr("routeN", ["n": 2]))
        XCTAssertNil(n.lineName("gone"))
        XCTAssertFalse(node("n", "N", replicas: ["Only"]).hasLineChoice)
    }

    func testAutomaticFailoverSaysSwitched() {
        XCTAssertEqual(show(.fixture(phase: .on, endpoint: "hk-1-1", previousEndpoint: "hk-1-0", latency: 38)).detail,
                       tr("d_switched", ["r": "HKG-B", "ms": "38 ms"]))
        // A pinned node never switches: the plain line.
        let pinned = show(.fixture(phase: .on, endpoint: "hk-1-1", previousEndpoint: "hk-1-0", latency: 38,
                                   pins: [IngressPin(nodeId: "hk-1", endpointKey: "hk-1-1")]))
        XCTAssertEqual(pinned.detail, tr("d_on", ["r": "HKG-B", "ms": "38 ms"]))
    }

    func testPinnedLineDownOffersAutomatic() {
        let pin = IngressPin(nodeId: "hk-1", endpointKey: "hk-1-1")
        func reported(_ healthy: Bool?) -> [NodeIngresses] {
            [NodeIngresses(nodeId: "hk-1", pinnedEndpointKey: "hk-1-1", ingresses: [
                IngressHealth(endpointKey: "hk-1-0", role: "primary", label: "HKG-A", healthy: true, active: false),
                IngressHealth(endpointKey: "hk-1-1", role: "backup", label: "HKG-B", healthy: healthy, active: true),
            ])]
        }
        XCTAssertNil(show(.fixture(pins: [pin], ingresses: reported(nil))).notices.first { $0.id == "ingressDown" })
        XCTAssertNil(show(.fixture(pins: [pin], ingresses: reported(true))).notices.first { $0.id == "ingressDown" })
        let notice = show(.fixture(pins: [pin], ingresses: reported(false))).notices.first { $0.id == "ingressDown" }
        XCTAssertEqual(notice?.title, tr("ingressDownT"))
        XCTAssertEqual(notice?.message, tr("ingressDownD", ["n": state.selectedNode!.name, "r": "HKG-B"]))
        XCTAssertEqual(notice?.actionTitle, tr("backToAuto"))
        XCTAssertEqual(notice?.action, .backToAuto)
    }

    func testClearedPinsNoticeDismisses() {
        let cleared = [IngressPin(nodeId: "hk-1", endpointKey: "old"), IngressPin(nodeId: "hk-1", endpointKey: "older")]
        let notice = show(.fixture(clearedPins: cleared)).notices.first { $0.id == "pinCleared" }
        XCTAssertEqual(notice?.title, tr("pinClearedT"))
        XCTAssertEqual(notice?.message, tr("pinClearedD", ["n": state.selectedNode!.name]))
        XCTAssertEqual(notice?.actionTitle, tr("ok"))
        XCTAssertEqual(notice?.action, .dismissClearedPins)
    }

    func testRebuiltProxyCredentialsNoticeDismisses() {
        XCTAssertNil(show(.fixture()).notices.first { $0.id == "proxyReset" })
        let notice = show(.fixture(proxyReset: true)).notices.first { $0.id == "proxyReset" }
        XCTAssertEqual(notice?.tone, .warn)
        XCTAssertEqual(notice?.title, tr("proxyResetT"))
        XCTAssertEqual(notice?.message, tr("proxyResetD"))
        XCTAssertEqual(notice?.actionTitle, tr("ok"))
        XCTAssertEqual(notice?.action, .dismissProxyReset)
        XCTAssertTrue(show(.fixture(profileStatus: .noSubscription, proxyReset: true)).notices.isEmpty)
    }

    func testRoutesTextUsesLabelsThenPositions() {
        XCTAssertEqual(node("n", "N", replicas: ["HKG-A", "HKG-B"]).routesText, "HKG-A → HKG-B")
        XCTAssertEqual(node("n", "N", replicas: ["HKG-A", nil, ""]).routesText,
                       "HKG-A → \(tr("routeN", ["n": 2])) → \(tr("routeN", ["n": 3]))")
        XCTAssertEqual(node("n", "N", replicas: [nil]).routesText, "")
        XCTAssertEqual(node("n", "N", replicas: ["Only"]).routesText, "Only")
        XCTAssertFalse(node("n", "N", replicas: [nil, nil]).routesText.contains("n-"))
    }

    // MARK: Routed local proxy

    private let nodeProxy = LocalProxy(nodeId: "hk-1", host: "127.0.0.1", port: 17890, username: "u-hk1", password: "p")
    private let routed = LocalProxy(nodeId: "", host: "127.0.0.1", port: 17890, username: "u", password: "p")

    func testRoutedUserIsShownByDefault() async {
        backend.proxyList = [nodeProxy]
        backend.routedProxy = routed
        backend.push(.fixture(standard: .ready(revision: "r1")))
        await settle { self.state.routedProxy != nil }
        XCTAssertTrue(state.offersProxyScope)
        XCTAssertEqual(state.proxyScope, .routed)
        XCTAssertEqual(state.shownProxy, routed)
        XCTAssertEqual(state.proxyNote, tr("proxyNoteRouted"))
        XCTAssertEqual(routed.socksURL, "socks5h://u:p@127.0.0.1:17890")
        XCTAssertEqual(routed.socksAddress, "socks5h://127.0.0.1:17890")
        XCTAssertEqual(routed.httpURL, "http://u:p@127.0.0.1:17890")

        state.proxyScope = .node
        XCTAssertEqual(state.shownProxy, nodeProxy)
        XCTAssertEqual(state.proxyNote, tr("proxyNote"))
        XCTAssertEqual(nodeProxy.socksURL, "socks5://u-hk1:p@127.0.0.1:17890")
    }

    func testWithoutRoutedUserTheNodeUserIsShown() async {
        backend.proxyList = [nodeProxy]
        backend.routedFailure = ClientError.StandardNotReady
        backend.push(.fixture(standard: .ready(revision: "r1")))
        await settle { self.state.currentProxy != nil }
        XCTAssertFalse(state.offersProxyScope)
        XCTAssertNil(state.routedProxy)
        XCTAssertEqual(state.shownProxy, nodeProxy)
        XCTAssertEqual(state.proxyNote, tr("proxyNote"))
    }

    func testFailedStandardCoreHidesTheRoutedUser() async {
        backend.proxyList = [nodeProxy]
        backend.routedProxy = routed
        backend.push(.fixture(standard: .ready(revision: "r1")))
        await settle { self.state.routedProxy != nil }
        backend.push(.fixture(standard: .failed(error: ClientErrorInfo(code: .standardCoreFailed, detail: "bind"))))
        XCTAssertFalse(state.offersProxyScope)
        XCTAssertNil(state.shownProxy)
    }

    func testProxyScopeTitles() {
        XCTAssertEqual(LocalProxyScope.allCases.map(\.title), [tr("proxyRouted"), tr("proxyNode")])
        XCTAssertEqual(LocalProxyScope.allCases.map(\.detail), [tr("proxyRoutedD"), tr("proxyNodeD")])
    }

    // MARK: Failed standard core

    func testFailedStandardCoreShowsTheLocalProxyUnavailable() async {
        let failure = ClientErrorInfo(code: .standardCoreFailed, detail: "bind")
        backend.proxyList = [LocalProxy(nodeId: "hk-1", host: "127.0.0.1", port: 17890, username: "u", password: "p")]
        backend.push(.fixture(standard: .ready(revision: "r1")))
        await settle { self.state.currentProxy != nil }
        XCTAssertNil(state.localProxyUnavailableText)

        for mode in [ConnectionMode.enhanced, .compatible] {
            let p = show(.fixture(mode: mode, standard: .failed(error: failure)))
            XCTAssertEqual(p.detail, tr("d_idleProxyFailed"), "\(mode)")
            let notice = p.notices.first { $0.id == "proxyFailed" }
            XCTAssertEqual(notice?.tone, .err)
            XCTAssertEqual(notice?.title, tr("proxyFailedT"))
            XCTAssertEqual(notice?.message, failure.message)
            XCTAssertEqual(notice?.actionTitle, tr("retry"))
            XCTAssertEqual(notice?.action, .retryLocalProxy)
        }
        XCTAssertNil(state.currentProxy)
        XCTAssertEqual(state.localProxyUnavailableText, tr("proxyFailed", ["reason": failure.message]))

        backend.push(.fixture(standard: .ready(revision: "r1")))
        XCTAssertTrue(show(.fixture(standard: .ready(revision: "r1"))).notices.isEmpty)
        XCTAssertEqual(show(.fixture(standard: .ready(revision: "r1"))).detail, tr("d_idle"))
    }

    func testTransitionsArePendingAndSomeLockTheSwitch() {
        for phase in [ConnectionPhase.preparing, .waitingPermission, .connecting, .reconnecting, .disconnecting] {
            let p = show(.fixture(phase: phase))
            XCTAssertEqual(p.connectSwitch, .pending, "\(phase)")
            XCTAssertEqual(p.tray, .busy, "\(phase)")
            XCTAssertEqual(p.connectSwitchEnabled, [.connecting, .reconnecting].contains(phase), "\(phase)")
        }
    }

    func testNoNodesLocksTheSwitch() {
        backend.catalog = []
        backend.push(.fixture(profileStatus: .loading))
        XCTAssertFalse(show(.fixture()).connectSwitchEnabled)
    }

    func testFailureNoticeOffersRetryAndCompatible() {
        let p = show(.fixture(phase: .error, reason: .init(code: .serviceInstallCancelled, detail: "x"),
                              retryable: true, suggestCompatible: true))
        XCTAssertEqual(p.tray, .error)
        XCTAssertEqual(p.notices.count, 1)
        let notice = p.notices[0]
        XCTAssertEqual(notice.tone, .err)
        XCTAssertEqual(notice.title, "\(tr("tunMode")) · \(tr("st_failed"))")
        XCTAssertEqual(notice.message, tr("fr_auth"))
        XCTAssertEqual(notice.action, .retry)
        XCTAssertEqual(notice.secondary, .useCompatible)
    }

    func testHealthCheckFailureShowsItsOwnTextAndSuggestsCompatible() {
        let p = show(.fixture(phase: .error, reason: .init(code: .connectHealthCheckFailed, detail: ""),
                              retryable: true, suggestCompatible: true))
        XCTAssertEqual(p.notices.first?.message, zh.texts["Error_ConnectHealthCheckFailed"])
        XCTAssertEqual(p.notices.first?.secondary, .useCompatible)
        for code in [ErrorCode.connectFailed, .timeout, .unreachable] {
            XCTAssertEqual(show(.fixture(phase: .error, reason: .init(code: code, detail: ""))).notices.first?.message,
                           tr("fr_timeout"), "\(code)")
        }
    }

    /// Surge's enhanced mode holding the path: one conflict notice naming it.
    func testCompetitorTurnsTheFailureIntoOneConflictNotice() {
        let p = show(.fixture(phase: .error, reason: .init(code: .connectHealthCheckFailed, detail: ""),
                              retryable: true, suggestCompatible: true, competitor: "Surge"))
        XCTAssertEqual(p.headline, tr("h_pathContended"))
        XCTAssertEqual(p.systemImage, "arrow.triangle.branch")
        XCTAssertEqual(p.notices.count, 1)
        let notice = p.notices[0]
        XCTAssertEqual(notice.id, "conflict")
        XCTAssertEqual(notice.tone, .err)
        XCTAssertEqual(notice.title, tr("conflictT"))
        XCTAssertEqual(notice.message, tr("conflictD", ["app": "Surge"]))
        XCTAssertTrue(notice.message.contains("Surge"))
        XCTAssertEqual(notice.action, .retry)
        XCTAssertEqual(notice.secondary, .useCompatible)

        let contended = show(.fixture(phase: .contended, reason: .init(code: .networkPathContended, detail: ""),
                                      retryable: true, competitor: "Clash"))
        XCTAssertEqual(contended.notices.map(\.id), ["conflict"])
        XCTAssertNil(contended.notices[0].secondary)
    }

    func testNoCompetitorOrCompatibleModeKeepsTheOldNotices() {
        let failed = show(.fixture(phase: .error, reason: .init(code: .connectFailed, detail: ""), retryable: true))
        XCTAssertEqual(failed.notices.map(\.id), ["failed"])
        XCTAssertEqual(failed.headline, tr("h_failed"))
        // Compatible mode: a replaced foreign proxy is not a failure.
        let compatible = show(.fixture(mode: .compatible, phase: .on, competitor: "Surge"))
        XCTAssertTrue(compatible.notices.isEmpty)
        XCTAssertTrue(show(.fixture(competitor: "Surge")).notices.isEmpty)
    }

    /// NetworkPathContended with nobody named is an ordinary failure, never
    /// "taken over" nor "in use on another device".
    func testPathContendedWithoutCompetitorIsAnOrdinaryFailure() {
        let p = show(.fixture(phase: .contended, reason: .init(code: .networkPathContended, detail: ""),
                              retryable: true, suggestCompatible: true))
        XCTAssertEqual(p.headline, tr("h_failed"))
        XCTAssertEqual(p.tone, .err)
        XCTAssertEqual(p.notices.map(\.id), ["failed"])
        XCTAssertEqual(p.notices.first?.message, tr("fr_timeout"))
        XCTAssertEqual(p.notices.first?.action, .retry)
        XCTAssertEqual(p.notices.first?.secondary, .useCompatible)
        XCTAssertFalse(p.notices.contains { $0.title == tr("st_occupied") })
        XCTAssertEqual(show(.fixture(phase: .error, reason: .init(code: .networkPathContended, detail: ""))).headline,
                       tr("h_failed"))
        // Another device of this account still reads as occupied.
        let busy = show(.fixture(phase: .contended, reason: .init(code: .serviceBusy, detail: ""), canTakeOver: true))
        XCTAssertEqual(busy.notices.map(\.id), ["occupied"])
    }

    func testSystemProxyUnavailableInCompatibleMode() {
        let p = show(.fixture(mode: .compatible, phase: .error, reason: .init(code: .systemProxyUnavailable, detail: "")))
        XCTAssertEqual(p.detail, tr("sysproxyUnavailable"))
        XCTAssertEqual(p.notices.first?.message, tr("sysproxyUnavailable"))
    }

    func testUnavailableRuleSetsShowAWarning() {
        let p = show(.fixture(ruleSetsUnavailable: ["geosite-cn"]))
        let notice = p.notices.first { $0.id == "rulesUnavailable" }
        XCTAssertEqual(notice?.tone, .warn)
        XCTAssertEqual(notice?.title, tr("rulesT"))
        XCTAssertEqual(notice?.message, tr("rulesUnavailableD"))
        XCTAssertNil(notice?.action)
        XCTAssertTrue(show(.fixture()).notices.isEmpty)
        XCTAssertTrue(show(.fixture(profileStatus: .noSubscription, ruleSetsUnavailable: ["x"])).notices.isEmpty)
    }

    func testCompatibleFailureReasons() {
        let admin = show(.fixture(mode: .compatible, phase: .error,
                                  reason: .init(code: .systemProxyFailed, detail: "ADMIN_REQUIRED: networksetup")))
        XCTAssertEqual(admin.headline, tr("h_stdFail"))
        XCTAssertEqual(admin.detail, tr("x_adminRequired"))
        XCTAssertEqual(admin.notices.first?.title, "\(tr("stdMode")) · \(tr("std_failed"))")
        XCTAssertNil(admin.notices.first?.action)

        let port = show(.fixture(mode: .compatible, phase: .error,
                                 reason: .init(code: .systemProxyFailed, detail: "port in use")))
        XCTAssertEqual(port.detail, tr("fr_port"))

        let other = show(.fixture(mode: .compatible, phase: .error, reason: .init(code: .noSubscription, detail: "")))
        XCTAssertEqual(other.detail, ErrorCode.noSubscription.message)
        XCTAssertEqual(show(.fixture(mode: .compatible, phase: .error)).detail, tr("fr_timeout"))
    }

    func testContendedByOtherDeviceOffersTakeOver() {
        let p = show(.fixture(phase: .contended, reason: .init(code: .serviceBusy, detail: ""), canTakeOver: true))
        XCTAssertEqual(p.tone, .warn)
        XCTAssertEqual(p.headline, tr("h_occupied"))
        XCTAssertEqual(p.notices.first?.action, .takeOver)
        XCTAssertEqual(p.notices.first?.actionTitle, tr("takeOver"))
    }

    func testContendedByAnotherUserOffersNothing() {
        let p = show(.fixture(phase: .contended, reason: .init(code: .serviceOwnedByAnotherUser, detail: "")))
        XCTAssertEqual(p.headline, tr("h_occupiedByUser"))
        XCTAssertEqual(p.systemImage, "person.2")
        XCTAssertNil(p.notices.first?.action)
        XCTAssertNil(p.notices.first?.actionTitle)
    }

    func testRestrictionWinsOverConnection() {
        let p = show(.fixture(profileStatus: .noSubscription, phase: .error))
        XCTAssertEqual(p.tone, .idle)
        XCTAssertEqual(p.tray, .off)
        XCTAssertEqual(p.statusLine, tr("noSubT"))
        XCTAssertTrue(p.notices.isEmpty)
        XCTAssertEqual(state.restriction?.offersPurchase, true)
        XCTAssertEqual(state.accessBlock, .noSubscription)
    }

    func testExpiredRestrictionDates() {
        backend.push(.fixture(profileStatus: .subscriptionExpired(expiredAt: nil)))
        XCTAssertEqual(state.restriction?.message, tr("x_expiredGenericD"))
        backend.push(.fixture(profileStatus: .subscriptionExpired(expiredAt: "2026-09-01T00:00:00Z")))
        let message = state.restriction?.message ?? ""
        XCTAssertNotEqual(message, tr("x_expiredGenericD"))
        XCTAssertTrue(message.contains("2026"), message)
    }

    func testTeamDisabledNamesTheTeam() {
        backend.push(.fixture(profileStatus: .teamDisabled))
        XCTAssertEqual(state.restriction?.message, tr("teamOffD", ["team": "Acme"]))
        XCTAssertEqual(state.restriction?.offersPurchase, false)
    }

    /// The node line carries no method when failed or occupied: the notice
    /// says why and the caption already names the method.
    func testFailedOrOccupiedLeaveTheDetailEmpty() {
        XCTAssertEqual(show(.fixture(phase: .error, reason: .init(code: .connectFailed, detail: ""))).detail, "")
        XCTAssertEqual(show(.fixture(phase: .contended, reason: .init(code: .serviceBusy, detail: ""),
                                     canTakeOver: true)).detail, "")
        XCTAssertEqual(ConnectionMode.enhanced.caption, tr("captionEnhanced"))
    }

    func testCompatibleTransitionsAsAppCore() {
        let starting = show(.fixture(mode: .compatible, phase: .connecting, endpoint: "hk-1-0", latency: 9))
        XCTAssertEqual(starting.headline, tr("h_stdStarting"))
        XCTAssertEqual(starting.detail, "")
        let stopping = show(.fixture(mode: .compatible, phase: .disconnecting))
        XCTAssertEqual(stopping.headline, tr("st_disconnecting"))
        XCTAssertEqual(stopping.detail, tr("d_disconnecting"))
        let again = show(.fixture(mode: .compatible, phase: .reconnecting, endpoint: "hk-1-1", previousEndpoint: "hk-1-0"))
        XCTAssertEqual(again.headline, tr("st_reconnecting"))
        XCTAssertEqual(again.detail, tr("d_reconnecting", ["r0": "HKG-A", "r": "HKG-B"]))
    }

    func testStatusLineIsOneShortState() {
        XCTAssertEqual(show(.fixture(phase: .connecting)).statusLine, tr("st_connecting"))
        XCTAssertEqual(show(.fixture(phase: .waitingPermission)).statusLine, tr("st_connecting"))
        XCTAssertEqual(show(.fixture(phase: .reconnecting)).statusLine, tr("st_reconnecting"))
        XCTAssertEqual(show(.fixture(phase: .on, endpoint: "hk-1-0", latency: 20)).statusLine, tr("h_on"))
        XCTAssertEqual(show(.fixture(phase: .disconnecting)).statusLine, tr("st_disconnecting"))
        XCTAssertEqual(show(.fixture(phase: .error, reason: .init(code: .connectFailed, detail: ""))).statusLine,
                       tr("h_failed"))
        XCTAssertEqual(show(.fixture(phase: .error, reason: .init(code: .connectHealthCheckFailed, detail: ""),
                                     competitor: "Surge")).statusLine, tr("h_pathContended"))
        XCTAssertFalse(show(.fixture(phase: .on)).statusLine.contains("香港"))
    }

    func testRestoringSaysNothingRatherThanSignedOut() {
        let p = show(.fixture(auth: .restoring))
        XCTAssertEqual(p.statusLine, "")
        XCTAssertEqual(p.tray, .off)
    }

    func testSignedOutStatusLine() {
        let p = show(.fixture(auth: .signedOut, phase: .on))
        XCTAssertEqual(p.statusLine, tr("notSignedIn"))
        XCTAssertEqual(p.tray, .off)
        XCTAssertTrue(p.notices.isEmpty)
    }

    func testLatencyFallsBackToProbe() async {
        state.probeMethod = .tcp
        state.clientDidProbe(ProbeResult(nodeId: "hk-1", method: .tcp, success: true, latencyMs: 42,
                                         endpointKey: nil, error: nil))
        XCTAssertEqual(state.currentLatencyText, "42 ms")
        // Results for another method are ignored.
        state.clientDidProbe(ProbeResult(nodeId: "hk-1", method: .icmp, success: true, latencyMs: 7,
                                         endpointKey: nil, error: nil))
        XCTAssertEqual(state.currentLatencyText, "42 ms")
        state.clientDidProbe(ProbeResult(nodeId: "hk-1", method: .tcp, success: false, latencyMs: nil,
                                         endpointKey: nil, error: nil))
        XCTAssertEqual(state.probes["hk-1"], .failed(.probeFailed))
        XCTAssertEqual(state.currentLatencyText, "—")
    }

    func testPhaseKeysExistForEveryPhase() {
        let phases: [ConnectionPhase] = [.off, .preparing, .waitingPermission, .connecting, .on, .reconnecting,
                                         .contended, .disconnecting, .error]
        for phase in phases {
            XCTAssertNotNil(zh.texts["st_" + phase.designKey], "\(phase)")
        }
        for tab in MainTab.allCases { XCTAssertNotNil(zh.texts[tab.rawValue], "\(tab)") }
    }
}
