import Foundation

/// Severity of a log line, lowest first. `unknown`: a line no format matched.
public enum LogSeverity: Int, Comparable, Sendable {
    case unknown, trace, debug, info, warn, error

    public static func < (lhs: LogSeverity, rhs: LogSeverity) -> Bool { lhs.rawValue < rhs.rawValue }

    /// "INFO", "WARN", …; empty for an unparsed line (log levels read the same in every language).
    public var text: String {
        switch self {
        case .trace: "TRACE"
        case .debug: "DEBUG"
        case .info: "INFO"
        case .warn: "WARN"
        case .error: "ERROR"
        case .unknown: ""
        }
    }

    /// Debug / trace lines read dimmed.
    public var isDim: Bool { self == .debug || self == .trace }

    init(level: String) {
        self = switch level.trimmingCharacters(in: .whitespaces).uppercased() {
        case "TRACE": .trace
        case "DEBUG": .debug
        case "INFO": .info
        case "WARN", "WARNING": .warn
        case "ERROR", "ERR", "FATAL", "CRITICAL", "PANIC": .error
        default: .unknown
        }
    }
}

/// A `key=value` of a log line, the value unquoted.
public struct LogField: Equatable, Sendable {
    public let key: String
    public let value: String

    public init(key: String, value: String) {
        self.key = key
        self.value = value
    }
}

/// One parsed log line (plus the lines that continue it, e.g. a panic message
/// spanning lines). `raw` is the original text; `message` is `raw` when no
/// format matched. App.Core's LogEntry.
public struct LogEntry: Identifiable, Equatable, Sendable {
    /// Order within its `LogBuffer`; a continued entry keeps its id.
    public var id: Int
    public var time: Date?
    public var severity: LogSeverity
    public var source: String?
    public var message: String
    public var fields: [LogField]
    public var raw: String
    /// "HH:mm:ss.SSS" in the zone the entry was parsed for; empty without a time.
    public var timeText: String

    /// What the search looks in, lowercased: message, source, then each field's
    /// key and value, apart (a query never spans two of them).
    private(set) var searchText: String

    init(id: Int, time: Date?, severity: LogSeverity, source: String?, message: String, fields: [LogField],
         raw: String, timeText: String) {
        self.id = id
        self.time = time
        self.severity = severity
        self.source = source
        self.message = message
        self.fields = fields
        self.raw = raw
        self.timeText = timeText
        searchText = ""
        indexForSearch()
    }

    /// The fields as one line, `key=value` separated by spaces (values quoted
    /// when they contain spaces).
    public var fieldsText: String {
        fields.map { "\($0.key)=\(LogParser.quote($0.value))" }.joined(separator: " ")
    }

    /// Longest message (and fields line) shown; a core may log a whole
    /// configuration on one line, which lays out slowly. Copies keep `raw`.
    public static let displayLimit = 4000

    public var displayMessage: String { Self.truncated(message) }
    public var displayFields: String { Self.truncated(fieldsText) }

    /// Identifies what the Logs view shows of this entry: a continued entry
    /// keeps its id but grows.
    public var stamp: Stamp { Stamp(id: id, rawLength: raw.utf8.count) }

    public struct Stamp: Equatable, Sendable {
        public let id: Int
        public let rawLength: Int
    }

    /// Appends a line of a multi-line message.
    mutating func `continue`(with line: String) {
        message += "\n" + line
        raw += "\n" + line
        indexForSearch()
    }

    /// Case-insensitive search over message, source, field keys and values;
    /// `query` is already lowercased.
    func matches(lowercased query: String) -> Bool { searchText.contains(query) }

    private mutating func indexForSearch() {
        var parts = [message]
        if let source { parts.append(source) }
        for field in fields {
            parts.append(field.key)
            parts.append(field.value)
        }
        searchText = parts.joined(separator: "\u{1}").lowercased()
    }

    private static func truncated(_ text: String) -> String {
        text.utf16.count <= displayLimit ? text : String(text.prefix(displayLimit)) + "…"
    }
}

/// Parses the lines of the log files the Logs page shows (App.Core's LogParser):
///
/// * client (Rust `tracing` fmt): `2026-09-30T16:42:45.634276Z  INFO ppvpn_client::enhanced: message key=value key="quoted"`
///   — RFC 3339 UTC time, level right-aligned to 5, optional spans
///   `name{fields}:`, the target, then the message with its fields appended
///   as trailing `key=value` tokens.
/// * core (logfmt): `2026-09-30T18:27:12.661737Z level=info msg="apply timing" outcome=ok total_ms=1`
///   (a leading `time=` key is accepted too).
/// * service (log4rs): `[2026-09-30 23:04:45.362][INFO] message`, local time.
///
/// Anything else continues the previous entry (see `append`) or stands as an
/// `unknown` entry.
public enum LogParser {
    private static let timePattern = #"\d{4}-\d{2}-\d{2}T\d{2}:\d{2}:\d{2}(?:\.\d+)?(?:Z|[+-]\d{2}:\d{2})"#

    private static let tracing = pattern(
        #"^(?<time>\#(timePattern))\s+(?<level>TRACE|DEBUG|INFO|WARN|ERROR)\s(?<rest>.*)$"#)

    /// Spans (`name{…}:`, any number) then the target (`crate::module`) and ": ".
    private static let tracingTarget = pattern(
        #"^\s*(?<spans>(?:[A-Za-z_][\w:]*(?:\{[^}]*\})?:\s?)*?)(?<target>[A-Za-z_]\w*(?:::\w+)*): (?<msg>.*)$"#)

    private static let logfmtStart = pattern(
        #"^(?:(?<time>\#(timePattern))\s+)?(?<rest>(?:time|level)=.*)$"#)

    private static let log4rs = pattern(
        #"^\[(?<time>\d{4}-\d{2}-\d{2} \d{2}:\d{2}:\d{2}(?:\.\d+)?)\]\[(?<level>[A-Za-z]+)\]\s?(?<msg>.*)$"#)

    /// A trailing ` key=value` (value quoted or a run without spaces) of a tracing message.
    private static let trailingField = pattern(
        #"\s(?<key>[A-Za-z_][\w.]*)=(?<value>"(?:[^"\\]|\\.)*"|[^\s"]+)$"#)

    /// `line` as an entry of its own, or nil when it matches no format (it
    /// then continues the previous entry). Times are shown in `zone`.
    public static func parse(_ line: String, id: Int = 0, zone: TimeZone = .current) -> LogEntry? {
        if let match = tracing.groups(in: line) {
            let time = parseUTC(match.text("time"))
            let rest = match.text("rest")
            var source: String?
            var message = String(rest.drop { $0 == " " })
            if let target = tracingTarget.groups(in: rest) {
                source = target.text("target")
                message = target.text("msg")
            }
            let (text, fields) = splitTrailingFields(message)
            return LogEntry(id: id, time: time, severity: LogSeverity(level: match.text("level")), source: source,
                            message: text, fields: fields, raw: line, timeText: timeText(time, zone))
        }
        if let match = logfmtStart.groups(in: line) {
            var time = match.optionalText("time").flatMap(parseUTC)
            var severity = LogSeverity.unknown
            var message = ""
            var fields: [LogField] = []
            for (key, value) in parseLogfmt(match.text("rest")) {
                switch key {
                case "time" where time == nil: time = parseUTC(value)
                case "level": severity = LogSeverity(level: value)
                case "msg" where message.isEmpty, "message" where message.isEmpty: message = value
                default: fields.append(LogField(key: key, value: value))
                }
            }
            if severity == .unknown, time == nil { return nil }
            return LogEntry(id: id, time: time, severity: severity, source: nil, message: message, fields: fields,
                            raw: line, timeText: timeText(time, zone))
        }
        if let match = log4rs.groups(in: line) {
            let time = parseLocal(match.text("time"), zone)
            return LogEntry(id: id, time: time, severity: LogSeverity(level: match.text("level")), source: nil,
                            message: match.text("msg"), fields: [], raw: line, timeText: timeText(time, zone))
        }
        return nil
    }

    /// Adds `line` to `entries`: a new entry (with id `id`), or appended to the
    /// last one when it matches no format (a line of a multi-line message).
    /// Returns true when the last entry was continued rather than one added.
    @discardableResult
    public static func append(_ line: String, to entries: inout [LogEntry], id: Int, zone: TimeZone = .current) -> Bool {
        if let entry = parse(line, id: id, zone: zone) {
            entries.append(entry)
            return false
        }
        if !entries.isEmpty {
            entries[entries.count - 1].continue(with: line)
            return true
        }
        entries.append(LogEntry(id: id, time: nil, severity: .unknown, source: nil, message: line, fields: [],
                                raw: line, timeText: ""))
        return false
    }

    /// The message without its trailing `key=value` fields, and the fields in order.
    static func splitTrailingFields(_ message: String) -> (String, [LogField]) {
        var fields: [LogField] = []
        // The leading space lets a message made only of fields (tracing without one) match too.
        var text = " " + message
        while let match = trailingField.groups(in: text) {
            fields.insert(LogField(key: match.text("key"), value: unquote(match.text("value"))), at: 0)
            text = String(text.utf16.prefix(match.start))!
        }
        return (text.trimmingCharacters(in: .whitespaces), fields)
    }

    /// logfmt pairs in order; a bare word becomes a key with an empty value.
    static func parseLogfmt(_ text: String) -> [(String, String)] {
        let chars = Array(text)
        var pairs: [(String, String)] = []
        var i = 0
        while i < chars.count {
            while i < chars.count, chars[i] == " " { i += 1 }
            if i >= chars.count { break }
            let keyStart = i
            while i < chars.count, chars[i] != "=", chars[i] != " " { i += 1 }
            let key = String(chars[keyStart..<i])
            if i >= chars.count || chars[i] == " " {
                pairs.append((key, ""))
                continue
            }
            i += 1 // "="
            let start = i
            if i < chars.count, chars[i] == "\"" {
                i += 1
                while i < chars.count, chars[i] != "\"" {
                    if chars[i] == "\\" { i += 1 }
                    i += 1
                }
                i = min(i + 1, chars.count)
                pairs.append((key, unquote(String(chars[start..<i]))))
            } else {
                while i < chars.count, chars[i] != " " { i += 1 }
                pairs.append((key, String(chars[start..<i])))
            }
        }
        return pairs
    }

    /// `"a \"b\""` → `a "b"` (Go / Rust Debug escapes); unquoted text unchanged.
    static func unquote(_ value: String) -> String {
        guard value.count >= 2, value.first == "\"", value.last == "\"" else { return value }
        let inner = Array(value.dropFirst().dropLast())
        guard inner.contains("\\") else { return String(inner) }
        var result = ""
        var i = 0
        while i < inner.count {
            if inner[i] != "\\" || i + 1 == inner.count {
                result.append(inner[i])
            } else {
                i += 1
                switch inner[i] {
                case "n": result.append("\n")
                case "t": result.append("\t")
                case "r": result.append("\r")
                default: result.append(inner[i])
                }
            }
            i += 1
        }
        return result
    }

    /// A value as logfmt writes it: quoted (with escapes) when empty or
    /// containing spaces, quotes or '='.
    static func quote(_ value: String) -> String {
        if !value.isEmpty, !value.contains(where: { " \"=\n\t".contains($0) }) { return value }
        let escaped = value.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"")
            .replacingOccurrences(of: "\n", with: "\\n").replacingOccurrences(of: "\t", with: "\\t")
        return "\"\(escaped)\""
    }

    // Formatters are costly to make and safe to share once set up.
    private static let formatterLock = NSLock()
    nonisolated(unsafe) private static var formatters: [String: DateFormatter] = [:]

    private static func formatter(_ format: String, _ zone: TimeZone) -> DateFormatter {
        formatterLock.lock()
        defer { formatterLock.unlock() }
        let key = format + "|" + zone.identifier
        if let cached = formatters[key] { return cached }
        let formatter = DateFormatter()
        formatter.locale = Locale(identifier: "en_US_POSIX")
        formatter.timeZone = zone
        formatter.dateFormat = format
        formatters[key] = formatter
        return formatter
    }

    /// `YYYY-MM-DDTHH:MM:SS[.fraction](Z|±HH:MM)`, the shape the patterns
    /// matched, read by hand: ISO8601DateFormatter costs ~40 µs a line.
    static func parseUTC(_ text: String) -> Date? {
        let c = Array(text.utf8)
        func number(_ from: Int, _ count: Int) -> Int? {
            guard from + count <= c.count else { return nil }
            var value = 0
            for byte in c[from..<(from + count)] {
                guard byte >= 48, byte <= 57 else { return nil }
                value = value * 10 + Int(byte - 48)
            }
            return value
        }
        guard let year = number(0, 4), let month = number(5, 2), let day = number(8, 2),
              let hour = number(11, 2), let minute = number(14, 2), let second = number(17, 2),
              (1...12).contains(month), (1...31).contains(day) else { return nil }
        var i = 19
        var fraction = 0.0
        if i < c.count, c[i] == UInt8(ascii: ".") {
            i += 1
            var scale = 0.1
            while i < c.count, c[i] >= 48, c[i] <= 57 {
                fraction += Double(c[i] - 48) * scale
                scale /= 10
                i += 1
            }
        }
        var offset = 0
        if i < c.count, c[i] == UInt8(ascii: "+") || c[i] == UInt8(ascii: "-") {
            guard let h = number(i + 1, 2), let m = number(i + 4, 2) else { return nil }
            offset = (h * 60 + m) * 60 * (c[i] == UInt8(ascii: "-") ? -1 : 1)
        } else if !(i < c.count && c[i] == UInt8(ascii: "Z")) {
            return nil
        }
        // Days since 1970-01-01 in the proleptic Gregorian calendar (Howard Hinnant's days_from_civil).
        let y = month <= 2 ? year - 1 : year
        let era = (y >= 0 ? y : y - 399) / 400
        let yoe = y - era * 400
        let doy = (153 * (month + (month > 2 ? -3 : 9)) + 2) / 5 + day - 1
        let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy
        let days = era * 146_097 + doe - 719_468
        let seconds = days * 86_400 + hour * 3_600 + minute * 60 + second - offset
        return Date(timeIntervalSince1970: Double(seconds) + fraction)
    }

    private static func parseLocal(_ text: String, _ zone: TimeZone) -> Date? {
        formatter(text.contains(".") ? "yyyy-MM-dd HH:mm:ss.SSS" : "yyyy-MM-dd HH:mm:ss", zone).date(from: text)
    }

    /// "HH:mm:ss.SSS" in `zone`; empty without a time.
    private static func timeText(_ time: Date?, _ zone: TimeZone) -> String {
        guard let time else { return "" }
        // Truncated to the millisecond like App.Core's "fff" (DateFormatter rounds).
        let milliseconds = (time.timeIntervalSince1970 * 1000 + 1e-3).rounded(.down) / 1000
        return formatter("HH:mm:ss.SSS", zone).string(from: Date(timeIntervalSince1970: milliseconds))
    }
}

/// Named groups of one match (NSRegularExpression: much faster than Swift
/// Regex over thousands of lines).
private struct Groups {
    let string: NSString
    let result: NSTextCheckingResult

    /// UTF-16 offset where the match starts.
    var start: Int { result.range.location }

    func text(_ name: String) -> String { optionalText(name) ?? "" }

    func optionalText(_ name: String) -> String? {
        let range = result.range(withName: name)
        return range.location == NSNotFound ? nil : string.substring(with: range)
    }
}

private extension NSRegularExpression {
    /// The first match in `text` (patterns anchor themselves).
    func groups(in text: String) -> Groups? {
        let string = text as NSString
        guard let result = firstMatch(in: text, range: NSRange(location: 0, length: string.length)) else { return nil }
        return Groups(string: string, result: result)
    }
}

private func pattern(_ pattern: String) -> NSRegularExpression {
    try! NSRegularExpression(pattern: pattern)
}

/// Lowest severity shown by the Logs page's level filter.
public enum LogLevelFilter: CaseIterable, Identifiable, Sendable {
    case all, debug, info, warn, error

    public var id: Self { self }

    public var title: String {
        switch self {
        case .all: tr("logAllLevels")
        case .debug: tr("logDebugUp")
        case .info: tr("logInfoUp")
        case .warn: tr("logWarnUp")
        case .error: tr("logErrorOnly")
        }
    }

    var minimum: LogSeverity {
        switch self {
        case .all: .trace
        case .debug: .debug
        case .info: .info
        case .warn: .warn
        case .error: .error
        }
    }
}

/// The parsed entries of the followed log file, fed with `LogTail`'s changes:
/// only complete lines are parsed (a partial last line waits for its end), and
/// at most `maxEntries` are kept.
public struct LogBuffer {
    public static let maxEntries = 2000

    public private(set) var entries: [LogEntry] = []
    /// Bumped when the entries are replaced (another file, truncation).
    public private(set) var generation = 0
    public var zone: TimeZone = .current
    private var partial = ""
    private var nextID = 0

    public init() {}

    public mutating func apply(_ change: LogTail.Change) {
        switch change {
        case .none:
            return
        case .replace(let text):
            entries = []
            partial = ""
            generation += 1
            // Only the lines that can be kept are parsed (App.Core's InitialLines).
            add(text, keepingLast: Self.maxEntries)
        case .append(let text):
            add(text)
        }
    }

    private mutating func add(_ text: String, keepingLast limit: Int = .max) {
        var lines = (partial + text).split(separator: "\n", omittingEmptySubsequences: false).map(String.init)
        partial = lines.removeLast()
        for line in lines.suffix(limit) where !line.isEmpty {
            let line = line.hasSuffix("\r") ? String(line.dropLast()) : line
            if !LogParser.append(line, to: &entries, id: nextID, zone: zone) { nextID += 1 }
        }
        if entries.count > Self.maxEntries { entries.removeFirst(entries.count - Self.maxEntries) }
    }

    /// Entries at `level` or above (unparsed lines count as info) that contain
    /// `search` in their message, source or a field (case-insensitive).
    public func visible(level: LogLevelFilter, search: String) -> [LogEntry] {
        let query = search.trimmingCharacters(in: .whitespaces).lowercased()
        return entries.filter { entry in
            (entry.severity == .unknown ? .info : entry.severity) >= level.minimum
                && (query.isEmpty || entry.matches(lowercased: query))
        }
    }
}
