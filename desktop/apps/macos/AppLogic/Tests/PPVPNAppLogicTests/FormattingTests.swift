import Foundation
import PPVPNAppLogic
import PPVPNClient
import XCTest

final class FormattingTests: LogicTestCase {
    private var utc: Calendar {
        var calendar = Calendar(identifier: .gregorian)
        calendar.locale = Locale(identifier: "en_US_POSIX")
        calendar.timeZone = TimeZone(identifier: "UTC")!
        return calendar
    }

    func testISO8601WithAndWithoutFraction() {
        XCTAssertEqual(ISO8601DateFormatter.parse("2026-09-30T12:00:00Z")?.timeIntervalSince1970, 1_790_769_600)
        XCTAssertEqual(ISO8601DateFormatter.parse("2026-09-30T12:00:00.250Z")?.timeIntervalSince1970,
                       1_790_769_600.25)
        XCTAssertEqual(ISO8601DateFormatter.parse("2026-09-30T20:00:00+08:00")?.timeIntervalSince1970, 1_790_769_600)
        XCTAssertNil(ISO8601DateFormatter.parse("30/09/2026"))
    }

    /// Chinese relative times: asserted by structure, not by formatter output.
    func testRelativeTimes() throws {
        let now = try XCTUnwrap(ISO8601DateFormatter.parse("2026-09-30T12:00:00Z"))
        XCTAssertEqual(RelativeTime.format(now.addingTimeInterval(-30), now: now, calendar: utc), tr("justNow"))
        XCTAssertEqual(RelativeTime.format(now.addingTimeInterval(-5 * 60), now: now, calendar: utc),
                       tr("minAgo", ["n": 5]))

        let clock = #"\d{1,2}:\d{2}"#
        func matches(_ text: String, _ key: String) -> Bool {
            let pattern = "^" + NSRegularExpression.escapedPattern(for: tr(key))
                .replacingOccurrences(of: #"\{t\}"#, with: clock) + "$"
            return text.range(of: pattern, options: .regularExpression) != nil
        }
        // Today and yesterday go by `now` in the calendar's zone (UTC here).
        let today = RelativeTime.format(now.addingTimeInterval(-3 * 3_600), now: now, calendar: utc)
        XCTAssertTrue(matches(today, "timeToday"), today)
        XCTAssertTrue(today.contains("09:00"), today)
        let yesterday = RelativeTime.format(now.addingTimeInterval(-13 * 3_600), now: now, calendar: utc)
        XCTAssertTrue(matches(yesterday, "timeYesterday"), yesterday)
        XCTAssertTrue(yesterday.contains("23:00"), yesterday)
        let twoDaysAgo = RelativeTime.format(now.addingTimeInterval(-50 * 3_600), now: now, calendar: utc)
        XCTAssertFalse(twoDaysAgo.contains(":"), twoDaysAgo)

        let lastMonth = RelativeTime.format(now.addingTimeInterval(-40 * 86_400), now: now, calendar: utc)
        XCTAssertFalse(lastMonth.contains(":"), lastMonth)
        XCTAssertFalse(lastMonth.contains("2026"), lastMonth)
        let lastYear = RelativeTime.format(now.addingTimeInterval(-400 * 86_400), now: now, calendar: utc)
        XCTAssertTrue(lastYear.contains("2025"), lastYear)

        let full = RelativeTime.full(now.addingTimeInterval(-30), now: now)
        XCTAssertTrue(full.hasSuffix(" · " + tr("justNow")), full)
    }

    func testErrorTexts() {
        XCTAssertEqual(ErrorCode.networkPathContended.key, "NetworkPathContended")
        XCTAssertEqual(ErrorCode.networkPathContended.message, zh.texts["Error_NetworkPathContended"])
        XCTAssertEqual(ClientError.Failed(code: .serviceBusy, detail: "d").userMessage, ErrorCode.serviceBusy.message)
        XCTAssertEqual(ClientError.NotSignedIn.userMessage, zh.texts["Error_NotSignedIn"])
        XCTAssertEqual(ClientError.Cancelled.userMessage, "")
    }

    func testUnknownErrorCodeFallsBack() {
        let saved = Localization.localizer
        defer { Localization.localizer = saved }
        Localization.localizer = SharedStrings(texts: zh.texts.filter { $0.key != "Error_ServiceBusy" })
        XCTAssertEqual(ErrorCode.serviceBusy.message, tr("x_unknownError", ["c": "ServiceBusy"]))
    }

    func testEveryErrorCodeHasText() {
        let missing = zh.texts.keys.filter { $0.hasPrefix("Error_") }.isEmpty
        XCTAssertFalse(missing)
        let codes: [ErrorCode] = [.networkPathContended, .serviceOwnedByAnotherUser, .systemProxyFailed, .serviceBusy,
                                  .noSubscription, .subscriptionExpired, .teamDisabled, .probeFailed, .timeout]
        for code in codes {
            XCTAssertNotNil(zh.texts["Error_" + code.key], code.key)
        }
    }

    func testFlagsAndNames() {
        XCTAssertEqual("hk".flagEmoji, "🇭🇰")
        let hk = node("hk-1", "香港 01", replicas: ["B", "A"])
        XCTAssertEqual(hk.displayName, "🇭🇰 香港 01")
        XCTAssertEqual(hk.orderedReplicas.map(\.replicaOrdinal), [0, 1])
        XCTAssertNil(Replica(endpointKey: "k", replicaOrdinal: 0, protocol: "p", label: nil).routeLabel)
        XCTAssertNil(Replica(endpointKey: "k", replicaOrdinal: 0, protocol: "p", label: "").routeLabel)
    }

    func testLocalProxyForms() {
        let proxy = LocalProxy(nodeId: "n", host: "127.0.0.1", port: 17890, username: "u", password: "p")
        XCTAssertEqual(proxy.httpURL, "http://u:p@127.0.0.1:17890")
        XCTAssertEqual(proxy.socksAddress, "socks5://127.0.0.1:17890")
    }

    func testMessageKinds() {
        var inbox = message(1, link: "https://example.com/x")
        XCTAssertEqual(inbox.kindTitle, tr("t_broadcast"))
        XCTAssertEqual(inbox.link?.host, "example.com")
        XCTAssertEqual(inbox.createdDate?.timeIntervalSince1970, 1_790_726_400)
        XCTAssertNil(inbox.severityTitle)
        inbox.severity = .critical
        XCTAssertEqual(inbox.severityTitle, tr("sev_critical"))
    }
}
