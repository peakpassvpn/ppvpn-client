import Foundation
@testable import PPVPNAppLogic
import XCTest
#if canImport(AppKit)
import AppKit
#endif

/// A long log (1 MiB, ~7800 lines, client and core formats mixed, some long
/// and multi-line entries) through the Logs page: open, follow, level switch,
/// search as typed and, on macOS, the text view. Prints medians; the bounds
/// only catch gross regressions (debug builds on a loaded CI runner are slow).
@MainActor
final class LogsPerformanceTests: LogicTestCase {
    static func bigLog(bytes: Int = 1024 * 1024) -> String {
        var lines: [String] = []
        var size = 0
        var i = 0
        let start = Date(timeIntervalSince1970: 1_790_794_142)
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        while size < bytes {
            let time = formatter.string(from: start.addingTimeInterval(Double(i) * 0.25))
            let line: String
            switch i % 10 {
            case 0, 1, 2, 3:
                line = "\(time)  INFO ppvpn_client::enhanced: health check ok elapsed_ms=\(i % 900) host=\"www.peakpassvpn.com\" probe=\(i)"
            case 4:
                line = "\(time) DEBUG ppvpn_client::monitor: traffic sample up_bps=\(i * 13) down_bps=\(i * 71) up_total=\(i * 1000) down_total=\(i * 9000)"
            case 5, 6:
                line = "\(time) level=info msg=\"apply timing\" outcome=ok total_ms=\(i % 40) rules=\(i % 300) node=hk-\(i % 7)"
            case 7:
                line = "\(time) level=warn msg=\"dns query timed out\" server=192.0.2.3 qname=example\(i % 50).com err=\"read udp 10.60.159.89:\(40000 + i % 20000)->192.0.2.3:53: i/o timeout\""
            case 8:
                line = "\(time)  WARN connect{mode=enhanced}:health{n=\(i)}: ppvpn_client::cores: retry err=\"dial tcp: lookup gstatic.com: timeout\" attempt=\(i % 5)"
            default:
                if i % 100 == 9 {
                    line = "\(time) ERROR ppvpn_client::service: request failed\n  caused by: connection reset\n  at service.rs:\(i % 400)"
                } else {
                    line = "\(time) level=error msg=\"core status\" detail=\"\(String(repeating: "x", count: 180))\" code=\(i % 9)"
                }
            }
            lines.append(line)
            size += line.utf8.count + 1
            i += 1
        }
        return lines.joined(separator: "\n") + "\n"
    }

    static func tick(_ second: Int) -> String {
        (0..<5).map { "2026-10-01T00:00:0\($0)Z  INFO ppvpn_client::enhanced: tick s=\(second) n=\($0)" }
            .joined(separator: "\n") + "\n"
    }

    /// Median of `runs` timings of `body`, in ms, printed as `PERF label`.
    @discardableResult
    static func median(_ label: String, runs: Int = 7, setUp: () -> Void = {}, _ body: () -> Void) -> Double {
        var samples: [Double] = []
        for _ in 0..<runs {
            setUp()
            let start = DispatchTime.now().uptimeNanoseconds
            body()
            samples.append(Double(DispatchTime.now().uptimeNanoseconds - start) / 1e6)
        }
        let median = samples.sorted()[runs / 2]
        print(String(format: "PERF %@: median %.1f ms (min %.1f, max %.1f)", label, median,
                     samples.min()!, samples.max()!))
        return median
    }

    func testLogic() throws {
        let text = Self.bigLog()
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        defer { try? FileManager.default.removeItem(at: directory) }
        let file = directory.appendingPathComponent("ppvpn-client.2026-10-01.log")
        try Data(text.utf8).write(to: file)
        print("PERF log: \(text.utf8.count) bytes, \(text.split(separator: "\n").count) lines")

        var buffer = LogBuffer()
        let open = Self.median("open: read 1 MiB + parse the last 2000 lines (off the main thread)") {
            buffer = LogBuffer()
            buffer.apply(LogTail().read(file))
        }
        XCTAssertGreaterThan(buffer.entries.count, 1900)
        XCTAssertLessThanOrEqual(buffer.entries.count, LogBuffer.maxEntries)

        let level = Self.median("level switch to warn+") { _ = buffer.visible(level: .warn, search: "") }
        var typing = 0.0
        for count in 1..."timeout".count {
            let query = String("timeout".prefix(count))
            typing = max(typing, Self.median("search filter '\(query)'") { _ = buffer.visible(level: .all, search: query) })
        }
        var second = 0
        let follow = Self.median("follow: parse 5 appended lines") {
            buffer.apply(.append(Self.tick(second)))
            second += 1
        }

        XCTAssertLessThan(open, 2000)
        XCTAssertLessThan(level, 500)
        XCTAssertLessThan(typing, 500)
        XCTAssertLessThan(follow, 200)
    }

    #if canImport(AppKit)
    func testTextView() {
        var buffer = LogBuffer()
        buffer.apply(.replace(Self.bigLog()))
        let all = buffer.visible(level: .all, search: "")
        var window: NSWindow!
        var view: NSTextView!
        var renderer: LogTextRenderer!
        func fresh() {
            window = NSWindow(contentRect: NSRect(x: 0, y: 0, width: 900, height: 600), styleMask: [.titled],
                              backing: .buffered, defer: false)
            let scroll = NSTextView.scrollableTextView()
            scroll.frame = window.contentView!.bounds
            window.contentView!.addSubview(scroll)
            view = scroll.documentView as? NSTextView
            view.isEditable = false
            renderer = LogTextRenderer()
        }
        func show(_ entries: [LogEntry]) {
            if renderer.update(view, to: entries) != .unchanged { view.scrollToEndOfDocument(nil) }
            window.displayIfNeeded()
        }

        let first = Self.median("text: open the page with 2000 entries (+display)", setUp: fresh) { show(all) }

        // Typing "time": one pass after the pause (debounced), from all entries.
        let narrowed = buffer.visible(level: .all, search: "time")
        let search = Self.median("text: search result \(narrowed.count) entries (+display)",
                                 setUp: { fresh(); show(all) }) { show(narrowed) }
        let level = buffer.visible(level: .warn, search: "")
        Self.median("text: level switch to warn+ \(level.count) entries (+display)",
                    setUp: { fresh(); show(all) }) { show(level) }

        fresh()
        show(all)
        var second = 0
        let follow = Self.median("text: follow 5 new lines (+display)", runs: 15) {
            buffer.apply(.append(Self.tick(second)))
            second += 1
            show(buffer.visible(level: .all, search: ""))
        }
        // A page left open: 2000 more seconds, then the same.
        for _ in 0..<2000 {
            buffer.apply(.append(Self.tick(second)))
            second += 1
            show(buffer.visible(level: .all, search: ""))
        }
        let long = Self.median("text: follow after 10000 more lines (+display)", runs: 15) {
            buffer.apply(.append(Self.tick(second)))
            second += 1
            show(buffer.visible(level: .all, search: ""))
        }
        print("PERF text: after the long session the view holds \(view.textStorage!.length) characters, "
              + "\(view.string.split(separator: "\n").count) lines")
        XCTAssertLessThanOrEqual(view.string.split(separator: "\n").count, LogBuffer.maxEntries + renderer.trimSlack + 1)

        XCTAssertLessThan(first, 3000)
        XCTAssertLessThan(search, 2000)
        XCTAssertLessThan(follow, 200)
        XCTAssertLessThan(long, 200)
    }
    #endif
}
