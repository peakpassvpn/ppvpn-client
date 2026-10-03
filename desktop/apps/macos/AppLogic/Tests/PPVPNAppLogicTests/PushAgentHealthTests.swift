import Foundation
import PPVPNAppLogic
import XCTest

final class PushAgentHealthTests: XCTestCase {
    private let start = Date(timeIntervalSince1970: 1_790_000_000)

    private func heal(heartbeat: TimeInterval? = nil, watching: TimeInterval = 600, running: Bool = false,
                      last: PushAgentHealth.HealRecord? = nil, version: String = "1.0") -> Bool {
        let now = start.addingTimeInterval(1_000)
        return PushAgentHealth.shouldHeal(
            heartbeat: heartbeat.map { now.addingTimeInterval(-$0) }, watchingSince: now.addingTimeInterval(-watching),
            now: now, agentRunning: running, lastHeal: last, version: version)
    }

    func testFreshHeartbeatIsHealthy() {
        XCTAssertFalse(heal(heartbeat: 60))
    }

    func testStaleHeartbeatWithoutProcessHeals() {
        XCTAssertTrue(heal(heartbeat: 400))
        XCTAssertTrue(heal(heartbeat: nil))
    }

    func testWaitsAFullWindowAfterLaunchOrWake() {
        XCTAssertFalse(heal(heartbeat: 4_000, watching: 100))
    }

    func testRunningProcessIsNotHealed() {
        XCTAssertFalse(heal(heartbeat: 400, running: true))
    }

    func testDownIgnoresTheHealRateLimit() {
        let now = start.addingTimeInterval(1_000)
        XCTAssertTrue(PushAgentHealth.isDown(heartbeat: now.addingTimeInterval(-400),
                                             watchingSince: now.addingTimeInterval(-600), now: now, agentRunning: false))
        XCTAssertFalse(PushAgentHealth.isDown(heartbeat: now.addingTimeInterval(-400),
                                              watchingSince: now.addingTimeInterval(-600), now: now, agentRunning: true))
        XCTAssertFalse(PushAgentHealth.isDown(heartbeat: now.addingTimeInterval(-60),
                                              watchingSince: now.addingTimeInterval(-600), now: now, agentRunning: false))
    }

    func testOncePerVersionPerDay() {
        let now = start.addingTimeInterval(1_000)
        let recent = PushAgentHealth.HealRecord(version: "1.0", at: now.addingTimeInterval(-3_600))
        XCTAssertFalse(heal(heartbeat: 400, last: recent))
        XCTAssertTrue(heal(heartbeat: 400, last: recent, version: "1.1"))
        let old = PushAgentHealth.HealRecord(version: "1.0", at: now.addingTimeInterval(-90_000))
        XCTAssertTrue(heal(heartbeat: 400, last: old))
    }

    func testHealRecordRoundTrips() throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        XCTAssertNil(PushAgentHealth.readHeal(in: directory))
        XCTAssertNil(PushAgentHealth.heartbeatDate(in: directory))
        let record = PushAgentHealth.HealRecord(version: "0.3.0", at: start)
        PushAgentHealth.writeHeal(record, in: directory)
        XCTAssertEqual(PushAgentHealth.readHeal(in: directory), record)
        try Data().write(to: directory.appendingPathComponent(PushAgentHealth.heartbeatFile))
        XCTAssertNotNil(PushAgentHealth.heartbeatDate(in: directory))
    }
}
