import Foundation

/// Source of the UI strings; the app bundle's string catalogs by default.
public protocol Localizer: Sendable {
    /// The text for `key` in `table` (nil = Localizable), or `key` itself
    /// when there is none.
    func string(_ key: String, table: String?) -> String
}

/// Looks strings up in a bundle's compiled string catalogs.
public struct BundleLocalizer: Localizer {
    private let bundle: Bundle

    public init(bundle: Bundle = .main) { self.bundle = bundle }

    public func string(_ key: String, table: String?) -> String {
        bundle.localizedString(forKey: key, value: nil, table: table)
    }
}

public enum Localization {
    /// Set once at launch or in tests, before any lookup.
    nonisolated(unsafe) public static var localizer: Localizer = BundleLocalizer()
}

/// Looks up a design-handoff string (Localizable.xcstrings, keys straight from
/// strings.json) and fills `{name}` placeholders.
public func tr(_ key: String, _ args: [String: CustomStringConvertible] = [:]) -> String {
    var text = Localization.localizer.string(key, table: nil)
    for (name, value) in args {
        text = text.replacingOccurrences(of: "{\(name)}", with: value.description)
    }
    return text
}

/// Relative times for the message centre: 刚刚 · N 分钟前 · 今天 14:20 ·
/// 昨天 14:20 · 9月21日 (year added across years).
public enum RelativeTime {
    /// Days and clock times are in `calendar`'s time zone, relative to `now`.
    public static func format(_ date: Date, now: Date = .now, calendar: Calendar = .current) -> String {
        let seconds = now.timeIntervalSince(date)
        if seconds < 60 { return tr("justNow") }
        if seconds < 3_600 { return tr("minAgo", ["n": Int(seconds / 60)]) }
        var style = Date.FormatStyle.dateTime
        style.calendar = calendar
        style.timeZone = calendar.timeZone
        // Always 24-hour: a 12-hour locale would otherwise drop the AM/PM.
        let time = date.formatted(Date.VerbatimFormatStyle(
            format: "\(hour: .twoDigits(clock: .twentyFourHour, hourCycle: .zeroBased)):\(minute: .twoDigits)",
            timeZone: calendar.timeZone, calendar: calendar))
        if calendar.isDate(date, inSameDayAs: now) { return tr("timeToday", ["t": time]) }
        if let yesterday = calendar.date(byAdding: .day, value: -1, to: now),
           calendar.isDate(date, inSameDayAs: yesterday) {
            return tr("timeYesterday", ["t": time])
        }
        if calendar.isDate(date, equalTo: now, toGranularity: .year) {
            return date.formatted(style.month(.abbreviated).day())
        }
        return date.formatted(style.year().month(.abbreviated).day())
    }

    /// "2026年9月29日 14:29 · 3 分钟前"
    public static func full(_ date: Date, now: Date = .now) -> String {
        let absolute = date.formatted(.dateTime.year().month(.abbreviated).day().hour().minute())
        return "\(absolute) · \(format(date, now: now))"
    }
}

extension ISO8601DateFormatter {
    /// RFC 3339 with or without fractional seconds.
    public static func parse(_ string: String) -> Date? {
        let formatter = ISO8601DateFormatter()
        formatter.formatOptions = [.withInternetDateTime, .withFractionalSeconds]
        if let date = formatter.date(from: string) { return date }
        formatter.formatOptions = [.withInternetDateTime]
        return formatter.date(from: string)
    }
}
