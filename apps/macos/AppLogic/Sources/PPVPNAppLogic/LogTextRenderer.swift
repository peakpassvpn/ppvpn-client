#if canImport(AppKit)
import AppKit

/// Keeps an `NSTextView` showing log entries, one line each: time
/// (monospaced, dim), level (error red, warning orange; debug / trace lines
/// dimmed), source (small), message and fields.
///
/// Updates follow `LogTextPlan`: new entries are appended, a continued last
/// entry is redrawn, and entries the buffer let go leave the front once
/// `trimSlack` of them gathered (deleting at the front lays the whole text
/// out again, so not every second). Only another file, filter or search
/// rebuilds. macOS only (AppKit); the rest of AppLogic builds on Linux.
@MainActor
public final class LogTextRenderer {
    /// What the view shows, oldest first, and each entry's length in the
    /// text (UTF-16, with its newline).
    public private(set) var stamps: [LogEntry.Stamp] = []
    private var lengths: [Int] = []
    /// Entries the buffer let go that may stay at the front before they are
    /// removed at once.
    public var trimSlack = 500

    public init() {}

    /// Brings `textView` to `entries`; returns what it did.
    @discardableResult
    public func update(_ textView: NSTextView, to entries: [LogEntry]) -> LogTextPlan {
        guard let storage = textView.textStorage else { return .unchanged }
        let plan = LogTextPlan.make(previous: stamps, next: entries)
        switch plan {
        case .unchanged:
            return plan
        case .rebuild:
            let text = NSMutableAttributedString()
            lengths = entries.map { entry in
                let line = Self.render(entry)
                text.append(line)
                return line.length
            }
            storage.setAttributedString(text)
        case let .update(dropFirst, replaceLast, appendFrom):
            // Gone from the buffer but still shown, until enough gathered.
            var stale: [LogEntry.Stamp] = []
            storage.beginEditing()
            if dropFirst > trimSlack {
                let removed = lengths[..<dropFirst].reduce(0, +)
                storage.deleteCharacters(in: NSRange(location: 0, length: removed))
                lengths.removeFirst(dropFirst)
            } else {
                stale = Array(stamps[..<dropFirst])
            }
            if replaceLast, let last = lengths.last {
                let line = Self.render(entries[appendFrom - 1])
                storage.replaceCharacters(in: NSRange(location: storage.length - last, length: last), with: line)
                lengths[lengths.count - 1] = line.length
            }
            for entry in entries[appendFrom...] {
                let line = Self.render(entry)
                storage.append(line)
                lengths.append(line.length)
            }
            storage.endEditing()
            stamps = stale + entries.map(\.stamp)
            return plan
        }
        stamps = entries.map(\.stamp)
        return plan
    }

    /// One entry's line, with its newline.
    public static func render(_ entry: LogEntry) -> NSAttributedString {
        let line = NSMutableAttributedString()
        let dim = entry.severity.isDim
        func add(_ string: String, _ font: NSFont, _ color: NSColor) {
            line.append(NSAttributedString(string: string, attributes: [
                .font: font, .foregroundColor: color, .paragraphStyle: paragraph,
            ]))
        }
        if !entry.timeText.isEmpty {
            add(entry.timeText + "  ", mono, .tertiaryLabelColor)
        }
        if entry.severity != .unknown {
            let color: NSColor = switch entry.severity {
            case .error: .systemRed
            case .warn: .systemOrange
            default: dim ? .tertiaryLabelColor : .secondaryLabelColor
            }
            add(entry.severity.text.padding(toLength: 5, withPad: " ", startingAt: 0) + "  ", monoBold, color)
        }
        if let source = entry.source {
            add(source + "  ", small, .tertiaryLabelColor)
        }
        add(entry.displayMessage, mono, dim ? .secondaryLabelColor : .labelColor)
        if !entry.fields.isEmpty {
            add((entry.message.isEmpty ? "" : "  ") + entry.displayFields, mono, .secondaryLabelColor)
        }
        add("\n", mono, .labelColor)
        return line
    }

    public static let mono = NSFont.monospacedSystemFont(ofSize: 11.5, weight: .regular)
    static let monoBold = NSFont.monospacedSystemFont(ofSize: 11, weight: .semibold)
    static let small = NSFont.systemFont(ofSize: 10.5)
    public static let paragraph: NSParagraphStyle = {
        let style = NSMutableParagraphStyle()
        style.lineHeightMultiple = 1.35
        return style
    }()
}
#endif
