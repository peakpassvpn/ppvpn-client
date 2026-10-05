@testable import PPVPNAppLogic
import XCTest

final class LogParserTests: LogicTestCase {
    private let utc = TimeZone(identifier: "UTC")!

    private func parse(_ line: String) -> LogEntry? { LogParser.parse(line, zone: utc) }

    // MARK: Client (tracing)

    func testClientLine() throws {
        let entry = try XCTUnwrap(parse(
            #"2026-09-30T18:49:02.356264Z  INFO ppvpn_client::enhanced: health check ok elapsed_ms=812 host="www.peakpassvpn.com""#))
        XCTAssertEqual(entry.severity, .info)
        XCTAssertEqual(entry.source, "ppvpn_client::enhanced")
        XCTAssertEqual(entry.message, "health check ok")
        XCTAssertEqual(entry.fields, [LogField(key: "elapsed_ms", value: "812"),
                                      LogField(key: "host", value: "www.peakpassvpn.com")])
        XCTAssertEqual(entry.timeText, "18:49:02.356")
        XCTAssertEqual(entry.time!.timeIntervalSince1970, 1790794142.356, accuracy: 0.001)
    }

    func testClientSpansAndEscapes() throws {
        let entry = try XCTUnwrap(parse(
            #"2026-09-30T18:49:02.356Z  WARN connect{mode=enhanced}:probe{n=1}: ppvpn_client::cores: retry err="dial \"a\"\ttimeout""#))
        XCTAssertEqual(entry.severity, .warn)
        XCTAssertEqual(entry.source, "ppvpn_client::cores")
        XCTAssertEqual(entry.message, "retry")
        XCTAssertEqual(entry.fields, [LogField(key: "err", value: "dial \"a\"\ttimeout")])
    }

    func testClientLevelsAndFieldsOnly() throws {
        XCTAssertEqual(parse("2026-09-30T18:49:02Z ERROR ppvpn_client: boom")?.severity, .error)
        let debug = try XCTUnwrap(parse("2026-09-30T18:49:02Z DEBUG ppvpn_client::monitor: tick=3"))
        XCTAssertEqual(debug.severity, .debug)
        XCTAssertTrue(debug.severity.isDim)
        XCTAssertEqual(debug.message, "")
        XCTAssertEqual(debug.fields, [LogField(key: "tick", value: "3")])
        XCTAssertEqual(debug.fieldsText, "tick=3")
    }

    // MARK: Core (logfmt)

    func testCoreLine() throws {
        let entry = try XCTUnwrap(parse(
            #"2026-09-30T18:27:12.661737Z level=info msg="apply timing" outcome=ok total_ms=1 note="a \"q\"\nb""#))
        XCTAssertEqual(entry.severity, .info)
        XCTAssertNil(entry.source)
        XCTAssertEqual(entry.message, "apply timing")
        XCTAssertEqual(entry.fields, [LogField(key: "outcome", value: "ok"), LogField(key: "total_ms", value: "1"),
                                      LogField(key: "note", value: "a \"q\"\nb")])
        XCTAssertEqual(entry.timeText, "18:27:12.661")
    }

    func testCoreTimeKey() throws {
        let entry = try XCTUnwrap(parse(#"time=2026-09-30T18:27:12Z level=error msg=failed dns=192.0.2.3"#))
        XCTAssertEqual(entry.severity, .error)
        XCTAssertEqual(entry.message, "failed")
        XCTAssertEqual(entry.timeText, "18:27:12.000")
        XCTAssertEqual(entry.fields, [LogField(key: "dns", value: "192.0.2.3")])
    }

    // MARK: Service (log4rs), continuations

    func testServiceLineInLocalTime() throws {
        let entry = try XCTUnwrap(LogParser.parse("[2026-09-30 23:04:45.362][INFO] core started", zone: utc))
        XCTAssertEqual(entry.severity, .info)
        XCTAssertEqual(entry.message, "core started")
        XCTAssertEqual(entry.timeText, "23:04:45.362")
    }

    func testUnmatchedLinesContinueTheLastEntry() {
        var entries: [LogEntry] = []
        LogParser.append("stray first", to: &entries, id: 0, zone: utc)
        XCTAssertEqual(entries.map(\.severity), [.unknown])
        XCTAssertEqual(entries[0].message, "stray first")

        LogParser.append("2026-09-30T18:49:02Z ERROR ppvpn_client: panicked", to: &entries, id: 1, zone: utc)
        XCTAssertTrue(LogParser.append("   at src/lib.rs:12", to: &entries, id: 2, zone: utc))
        XCTAssertEqual(entries.count, 2)
        XCTAssertEqual(entries[1].message, "panicked\n   at src/lib.rs:12")
        XCTAssertEqual(entries[1].raw, "2026-09-30T18:49:02Z ERROR ppvpn_client: panicked\n   at src/lib.rs:12")
    }

    func testTimestampsMatchFoundation() {
        let iso = ISO8601DateFormatter()
        for text in ["2026-09-30T18:49:02Z", "2024-02-29T23:59:59Z", "1999-12-31T00:00:00Z", "2026-01-01T08:00:00+08:00",
                     "2026-03-01T01:30:00-05:30", "2000-03-01T12:00:00Z"] {
            XCTAssertEqual(LogParser.parseUTC(text), iso.date(from: text), text)
        }
        XCTAssertEqual(LogParser.parseUTC("2026-09-30T18:49:02.5Z")?.timeIntervalSince1970 ?? 0,
                       (iso.date(from: "2026-09-30T18:49:02Z")?.timeIntervalSince1970 ?? 0) + 0.5, accuracy: 1e-6)
        XCTAssertNil(LogParser.parseUTC("2026-13-01T00:00:00Z"))
        XCTAssertNil(LogParser.parseUTC("2026-09-30T18:49:02"))
        XCTAssertNil(LogParser.parseUTC("not a time"))
    }

    func testQuoteRoundTrips() {
        XCTAssertEqual(LogParser.quote("plain"), "plain")
        XCTAssertEqual(LogParser.quote(""), "\"\"")
        XCTAssertEqual(LogParser.quote("a \"b\""), #""a \"b\"""#)
        XCTAssertEqual(LogParser.unquote(LogParser.quote("x=1\ty")), "x=1\ty")
    }

    // MARK: Buffer, filter, search

    func testBufferWaitsForWholeLinesAndFilters() {
        var buffer = LogBuffer()
        buffer.zone = utc
        buffer.apply(.replace("2026-09-30T18:49:02Z DEBUG ppvpn_client::monitor: tick\n2026-09-30T18:49:03Z  WA"))
        XCTAssertEqual(buffer.entries.count, 1)
        buffer.apply(.append("RN ppvpn_client::sysproxy: taken over app=Surge\nlevel=error msg=\"core died\"\n"))
        XCTAssertEqual(buffer.entries.map(\.severity), [.debug, .warn, .error])
        XCTAssertEqual(Set(buffer.entries.map(\.id)).count, 3)

        XCTAssertEqual(buffer.visible(level: .all, search: "").count, 3)
        XCTAssertEqual(buffer.visible(level: .info, search: "").map(\.severity), [.warn, .error])
        XCTAssertEqual(buffer.visible(level: .error, search: "").map(\.message), ["core died"])
        // Message, source, field key and value; case-insensitive.
        XCTAssertEqual(buffer.visible(level: .all, search: "SURGE").map(\.severity), [.warn])
        XCTAssertEqual(buffer.visible(level: .all, search: "app").map(\.severity), [.warn])
        XCTAssertEqual(buffer.visible(level: .all, search: "monitor").map(\.severity), [.debug])
        XCTAssertEqual(buffer.visible(level: .all, search: " died ").map(\.severity), [.error])
        XCTAssertTrue(buffer.visible(level: .all, search: "nothing").isEmpty)

        let generation = buffer.generation
        buffer.apply(.replace(""))
        XCTAssertTrue(buffer.entries.isEmpty)
        XCTAssertGreaterThan(buffer.generation, generation)
    }

    func testUnparsedLinesCountAsInfo() {
        var buffer = LogBuffer()
        buffer.apply(.replace("just text\n"))
        XCTAssertEqual(buffer.visible(level: .info, search: "").count, 1)
        XCTAssertTrue(buffer.visible(level: .warn, search: "").isEmpty)
    }

    func testBufferKeepsTheNewestEntries() {
        var buffer = LogBuffer()
        let lines = (0..<(LogBuffer.maxEntries + 5)).map { "level=info msg=m\($0)" }
        buffer.apply(.replace(lines.joined(separator: "\n") + "\n"))
        XCTAssertEqual(buffer.entries.count, LogBuffer.maxEntries)
        XCTAssertEqual(buffer.entries.first?.message, "m5")

        buffer.apply(.append("level=warn msg=late\n"))
        XCTAssertEqual(buffer.entries.count, LogBuffer.maxEntries)
        XCTAssertEqual(buffer.entries.first?.message, "m6")
        XCTAssertEqual(buffer.entries.last?.message, "late")
    }

    func testLevelFilterTitles() {
        XCTAssertEqual(LogLevelFilter.allCases.map(\.title),
                       ["logAllLevels", "logDebugUp", "logInfoUp", "logWarnUp", "logErrorOnly"].map { tr($0) })
    }
}
