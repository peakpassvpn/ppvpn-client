import Foundation
@testable import PPVPNAppLogic
import XCTest

private func entry(_ id: Int, _ message: String = "m") -> LogEntry {
    LogEntry(id: id, time: nil, severity: .info, source: nil, message: message, fields: [],
             raw: "level=info msg=\(message)", timeText: "")
}

final class LogTextPlanTests: XCTestCase {
    private func plan(_ previous: [LogEntry], _ next: [LogEntry]) -> LogTextPlan {
        LogTextPlan.make(previous: previous.map(\.stamp), next: next)
    }

    func testFirstShowRebuilds() {
        XCTAssertEqual(plan([], []), .unchanged)
        XCTAssertEqual(plan([], [entry(0)]), .rebuild)
    }

    func testSameEntriesAreUnchanged() {
        XCTAssertEqual(plan([entry(0), entry(1)], [entry(0), entry(1)]), .unchanged)
    }

    func testNewEntriesAppend() {
        XCTAssertEqual(plan([entry(0), entry(1)], [entry(0), entry(1), entry(2), entry(3)]),
                       .update(dropFirst: 0, replaceLast: false, appendFrom: 2))
    }

    func testEntriesTheBufferDroppedLeaveTheFront() {
        XCTAssertEqual(plan([entry(0), entry(1), entry(2)], [entry(2), entry(3)]),
                       .update(dropFirst: 2, replaceLast: false, appendFrom: 1))
        XCTAssertEqual(plan([entry(0), entry(1), entry(2)], [entry(1), entry(2)]),
                       .update(dropFirst: 1, replaceLast: false, appendFrom: 2))
    }

    func testContinuedLastEntryIsRedrawn() {
        var grown = entry(1)
        grown.continue(with: "  at lib.rs:3")
        XCTAssertEqual(plan([entry(0), entry(1)], [entry(0), grown]),
                       .update(dropFirst: 0, replaceLast: true, appendFrom: 2))
        XCTAssertEqual(plan([entry(0), entry(1)], [entry(0), grown, entry(2)]),
                       .update(dropFirst: 0, replaceLast: true, appendFrom: 2))
    }

    func testAnythingElseRebuilds() {
        // Another filter: a gap, a new first entry, an earlier entry changed, or nothing left.
        XCTAssertEqual(plan([entry(0), entry(1), entry(2)], [entry(0), entry(2)]), .rebuild)
        XCTAssertEqual(plan([entry(1), entry(2)], [entry(0), entry(1), entry(2)]), .rebuild)
        var grown = entry(0)
        grown.continue(with: "more")
        XCTAssertEqual(plan([entry(0), entry(1)], [grown, entry(1)]), .rebuild)
        XCTAssertEqual(plan([entry(0)], []), .rebuild)
        XCTAssertEqual(plan([entry(0), entry(1), entry(2)], [entry(1)]), .rebuild)
    }
}

@MainActor
final class LogsStateTests: LogicTestCase {
    private var directory: URL!
    private var state: LogsState!

    override func setUp() async throws {
        directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        state = LogsState(directory: directory)
    }

    override func tearDown() async throws {
        try? FileManager.default.removeItem(at: directory)
    }

    private func write(_ name: String, _ text: String, append: Bool = false) throws {
        let file = directory.appendingPathComponent(name)
        if append, let handle = try? FileHandle(forWritingTo: file) {
            handle.seekToEndOfFile()
            handle.write(Data(text.utf8))
            try handle.close()
        } else {
            try Data(text.utf8).write(to: file)
        }
    }

    private let lines = """
        2026-10-01T00:00:00Z  INFO ppvpn_client::enhanced: connected node=hk-1
        2026-10-01T00:00:01Z DEBUG ppvpn_client::monitor: tick n=1
        2026-10-01T00:00:02Z level=warn msg="dns timeout" server=192.0.2.3
        2026-10-01T00:00:03Z level=error msg="core died"

        """

    func testPollFollowsTheClientLogAndAppends() async throws {
        try write("ppvpn-core.2026-10-01.log", "level=info msg=core\n")
        try write("ppvpn-client.2026-10-01.log", lines)
        state.poll()
        await settle { self.state.shown.count == 4 }
        XCTAssertEqual(state.selected?.kind, .client)
        XCTAssertEqual(state.files.map(\.kind), [.client, .core])
        XCTAssertTrue(state.hasEntries)

        try write("ppvpn-client.2026-10-01.log", "2026-10-01T00:00:04Z  INFO ppvpn_client::x: later\n", append: true)
        state.poll()
        await settle { self.state.shown.count == 5 }
        XCTAssertEqual(state.shown.last?.message, "later")
        XCTAssertEqual(state.shownText.split(separator: "\n").count, 5)
    }

    func testLevelAppliesAtOnceAndSearchAfterTyping() async throws {
        try write("ppvpn-client.2026-10-01.log", lines)
        state.searchDelay = .milliseconds(150)
        state.poll()
        await settle { self.state.shown.count == 4 }

        state.level = .warn
        await settle { self.state.shown.count == 2 }
        XCTAssertEqual(state.shown.map(\.severity), [.warn, .error])

        state.level = .all
        await settle { self.state.shown.count == 4 }
        let runs = state.filterRuns
        for prefix in ["d", "di", "die", "died"] { state.search = prefix }
        await settle(timeout: 0.05)
        XCTAssertEqual(state.filterRuns, runs, "waits for typing to pause")
        await settle { self.state.shown.count == 1 }
        XCTAssertEqual(state.filterRuns, runs + 1, "one pass for the whole word")
        XCTAssertEqual(state.shown.first?.message, "core died")

        // Field keys and values, case-insensitive.
        state.search = "SERVER"
        await settle { self.state.shown.first?.message == "dns timeout" }
        state.search = "10.10.0"
        await settle { self.state.shown.first?.message == "dns timeout" }
        state.search = "nothing"
        await settle { self.state.shown.isEmpty }
        XCTAssertTrue(state.hasEntries, "no match, not an empty file")
    }

    func testSwitchingFilesReplacesTheEntries() async throws {
        try write("ppvpn-client.2026-10-01.log", lines)
        try write("ppvpn-core.2026-10-01.log", "level=info msg=core\n")
        state.poll()
        await settle { self.state.shown.count == 4 }
        state.selected = state.files.first { $0.kind == .core }
        await settle { self.state.shown.count == 1 }
        XCTAssertEqual(state.shown.first?.message, "core")
    }

    func testEmptyDirectoryShowsNothing() async {
        state.poll()
        await settle(timeout: 0.2)
        XCTAssertNil(state.selected)
        XCTAssertTrue(state.shown.isEmpty)
        XCTAssertFalse(state.hasEntries)
    }

    func testSearchIndexesMessageSourceAndFieldsApart() throws {
        let line = "2026-10-01T00:00:00Z  WARN ppvpn_client::cores: Retry Later key=Value other=x"
        let parsed = try XCTUnwrap(LogParser.parse(line))
        for query in ["retry", "later", "cores", "key", "value", "other"] {
            XCTAssertTrue(parsed.matches(lowercased: query), query)
        }
        // A query never spans two parts.
        XCTAssertFalse(parsed.matches(lowercased: "key=value"))
        XCTAssertFalse(parsed.matches(lowercased: "valueother"))
    }

    func testLongEntriesAreTruncatedForDisplayOnly() {
        let long = String(repeating: "y", count: LogEntry.displayLimit + 10)
        let e = entry(0, long)
        XCTAssertEqual(e.displayMessage.count, LogEntry.displayLimit + 1)
        XCTAssertTrue(e.displayMessage.hasSuffix("…"))
        XCTAssertTrue(e.raw.hasSuffix(long))
        XCTAssertEqual(entry(1, "short").displayMessage, "short")
    }
}

#if canImport(AppKit)
import AppKit

@MainActor
final class LogTextRendererTests: XCTestCase {
    private func textView() -> NSTextView {
        NSTextView.scrollableTextView().documentView as! NSTextView
    }

    private func lines(_ view: NSTextView) -> [String] {
        view.string.split(separator: "\n").map(String.init)
    }

    func testFollowingDropsWhatTheBufferLetGo() {
        let view = textView()
        let renderer = LogTextRenderer()
        renderer.trimSlack = 0
        XCTAssertEqual(renderer.update(view, to: [entry(0, "a"), entry(1, "b"), entry(2, "c")]), .rebuild)
        XCTAssertEqual(lines(view), ["INFO   a", "INFO   b", "INFO   c"])

        XCTAssertEqual(renderer.update(view, to: [entry(1, "b"), entry(2, "c"), entry(3, "d")]),
                       .update(dropFirst: 1, replaceLast: false, appendFrom: 2))
        XCTAssertEqual(lines(view), ["INFO   b", "INFO   c", "INFO   d"])

        var grown = entry(3, "d")
        grown.continue(with: "  more")
        XCTAssertEqual(renderer.update(view, to: [entry(2, "c"), grown, entry(4, "e")]),
                       .update(dropFirst: 1, replaceLast: true, appendFrom: 2))
        XCTAssertEqual(lines(view), ["INFO   c", "INFO   d", "  more", "INFO   e"])
        XCTAssertEqual(renderer.update(view, to: [entry(2, "c"), grown, entry(4, "e")]), .unchanged)

        XCTAssertEqual(renderer.update(view, to: [entry(4, "e")]),
                       .update(dropFirst: 2, replaceLast: false, appendFrom: 1))
        XCTAssertEqual(lines(view), ["INFO   e"])
        XCTAssertEqual(renderer.update(view, to: [entry(9, "z")]), .rebuild)
        XCTAssertEqual(lines(view), ["INFO   z"])
        XCTAssertEqual(renderer.update(view, to: []), .rebuild)
        XCTAssertEqual(view.string, "")
    }

    func testTextStaysWithinTheBufferAndTheSlack() {
        let view = textView()
        let renderer = LogTextRenderer()
        renderer.trimSlack = 100
        var buffer = LogBuffer()
        buffer.apply(.replace(""))
        var most = 0
        for second in 0..<1000 {
            buffer.apply(.append((0..<5).map { "level=info msg=s\(second)-\($0)" }.joined(separator: "\n") + "\n"))
            renderer.update(view, to: buffer.visible(level: .all, search: ""))
            most = max(most, renderer.stamps.count)
            if second % 100 == 0 { XCTAssertEqual(lines(view).count, renderer.stamps.count) }
        }
        XCTAssertEqual(buffer.entries.count, LogBuffer.maxEntries)
        XCTAssertLessThanOrEqual(most, LogBuffer.maxEntries + 101)
        XCTAssertGreaterThan(most, LogBuffer.maxEntries + 90, "trims in batches, not every second")
        XCTAssertEqual(lines(view).last, "INFO   s999-4")
        // The newest entries are all there, in order, after whatever stale ones remain.
        XCTAssertEqual(Array(lines(view).suffix(LogBuffer.maxEntries)), buffer.entries.map { "INFO   \($0.message)" })

        // Leaving the follow (a filter) rebuilds to exactly the shown entries.
        renderer.update(view, to: buffer.visible(level: .warn, search: ""))
        XCTAssertEqual(view.string, "")
    }
}
#endif
