import Foundation

/// Incremental tail of one log file: the first read takes the last
/// `initialBytes` (from the first line starting in them), later reads only
/// what was appended; truncation or rotation starts over.
public final class LogTail {
    public private(set) var url: URL?
    private var offset: UInt64 = 0
    /// Enough for `LogBuffer.maxEntries` lines (App.Core's TailBytes).
    private let initialBytes: UInt64 = 1024 * 1024

    public enum Change { case none, replace(String), append(String) }

    public init() {}

    public func read(_ url: URL?) -> Change {
        guard let url, let handle = try? FileHandle(forReadingFrom: url) else {
            defer { self.url = nil; offset = 0 }
            return self.url == nil ? .none : .replace("")
        }
        defer { try? handle.close() }
        let size = (try? handle.seekToEnd()) ?? 0
        if url != self.url || size < offset {
            self.url = url
            offset = size > initialBytes ? size - initialBytes : 0
            try? handle.seek(toOffset: offset)
            var data = (try? handle.readToEnd()) ?? Data()
            // Started mid-file: drop the cut-off first line.
            if offset > 0, let newline = data.firstIndex(of: UInt8(ascii: "\n")) {
                data = data[data.index(after: newline)...]
            }
            offset = size
            return .replace(String(decoding: data, as: UTF8.self))
        }
        guard size > offset else { return .none }
        try? handle.seek(toOffset: offset)
        let data = (try? handle.readToEnd()) ?? Data()
        offset += UInt64(data.count)
        return .append(String(decoding: data, as: UTF8.self))
    }
}

/// A daily log file: client or core, dated by its (UTC) file name.
public struct LogFile: Identifiable, Hashable, Comparable, Sendable {
    public enum Kind: String, Sendable { case client = "ppvpn-client", core = "ppvpn-core" }

    public let url: URL
    public let kind: Kind
    public let day: String

    public var id: URL { url }

    public init?(url: URL) {
        guard url.pathExtension == "log" else { return nil }
        let parts = url.deletingPathExtension().lastPathComponent.split(separator: ".", maxSplits: 1)
        guard parts.count == 2, let kind = Kind(rawValue: String(parts[0])) else { return nil }
        self.url = url
        self.kind = kind
        self.day = String(parts[1])
    }

    /// "客户端 · 今天" / "核心 · 昨天" / "客户端 · 2026-09-27".
    public var title: String {
        let kindTitle = kind == .client ? tr("client") : tr("core")
        return "\(kindTitle) · \(dayTitle)"
    }

    private var dayTitle: String {
        var utc = Calendar(identifier: .gregorian)
        utc.timeZone = TimeZone(identifier: "UTC")!
        let formatter = DateFormatter()
        formatter.calendar = utc
        formatter.timeZone = utc.timeZone
        formatter.dateFormat = "yyyy-MM-dd"
        guard let date = formatter.date(from: day) else { return day }
        if utc.isDateInToday(date) { return tr("today") }
        if utc.isDateInYesterday(date) { return tr("yesterday") }
        return day
    }

    /// Newest day first; client before core on the same day.
    public static func < (lhs: LogFile, rhs: LogFile) -> Bool {
        lhs.day != rhs.day ? lhs.day > rhs.day : lhs.kind == .client && rhs.kind == .core
    }
}
