import Foundation
import PPVPNAppLogic
import PPVPNClient
import XCTest

final class AgentSupportTests: XCTestCase {
    override func tearDown() {
        AppLog.sink = nil
        super.tearDown()
    }

    /// The Flutter app's directories (apps/flutter/rust/src/dirs.rs): the
    /// agent reads the sign-in the app wrote.
    func testDirectoriesAreTheApps() {
        XCTAssertEqual(AppDirectories.data.lastPathComponent, "PPVPN")
        #if os(macOS)
        // Foundation on Linux has no user Library directory; the agent runs
        // only on macOS.
        XCTAssertEqual(Array(AppDirectories.logs.pathComponents.suffix(2)), ["Logs", "PPVPN"])
        #endif
    }

    func testLogGoesToTheSink() {
        let lines = Lines()
        AppLog.sink = { level, message in lines.append("\(level) \(message)") }
        let log = AppLog(subsystem: "com.peakpassvpn.ppvpn.desktop", category: "test")
        log.info("one")
        log.error("two")
        XCTAssertEqual(lines.all, ["info one", "error two"])
    }

    /// The agent's crate API compiles against these bindings.
    func testBindingsHaveThePushAgent() {
        let config = PushAgentConfig(dataDir: "/nonexistent", logDir: "/nonexistent", platform: "macos", appVersion: "0")
        XCTAssertEqual(config.platform, "macos")
    }

    private final class Lines: @unchecked Sendable {
        private let lock = NSLock()
        private var lines: [String] = []
        func append(_ line: String) { lock.lock(); lines.append(line); lock.unlock() }
        var all: [String] { lock.lock(); defer { lock.unlock() }; return lines }
    }
}
