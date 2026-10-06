import Foundation
import PPVPNClient

extension UInt64 {
    private static let sizeUnits = ["KB", "MB", "GB", "TB"]

    /// A byte count (traffic, sizes) in DECIMAL units, as the web and store: 1 KB = 1000 B,
    /// 1 MB = 10^6, 1 GB = 10^9, 1 TB = 10^12; labels KB/MB/GB/TB, never KiB-style. The unit is the
    /// largest one the value reaches (KB at least: "0 KB" floor, no bytes). KB is whole; MB and up
    /// have one decimal below 100 ("8.6 MB") and are whole from 100 ("400 GB"). Rounding is half up
    /// on the exact integer, and a value that rounds to 1000 moves to the next unit ("1.0 MB").
    /// Locale-independent, the same rule as the Windows and Linux apps.
    public var byteCount: String {
        let units = Self.sizeUnits
        var divisor: UInt64 = 1_000
        var unit = 0
        while unit < units.count - 1, self >= divisor * 1_000 {
            divisor *= 1_000
            unit += 1
        }
        while true {
            if unit > 0 {
                let tenths = roundedDivided(by: divisor / 10)
                if tenths < 1_000 { return "\(tenths / 10).\(tenths % 10) \(units[unit])" }
            }
            let whole = roundedDivided(by: divisor)
            if whole < 1_000 || unit == units.count - 1 { return "\(whole) \(units[unit])" }
            divisor *= 1_000
            unit += 1
        }
    }

    /// A rate in bytes per second: `byteCount` + "/s" ("22 KB/s", "8.6 MB/s", "0 KB/s" floor).
    public var byteRate: String { byteCount + "/s" }

    /// `self / divisor`, rounded half up, without overflow.
    private func roundedDivided(by divisor: UInt64) -> UInt64 {
        self / divisor + (2 * (self % divisor) >= divisor ? 1 : 0)
    }
}

extension String {
    /// Regional-indicator flag emoji for an ISO 3166-1 alpha-2 code.
    public var flagEmoji: String {
        uppercased().unicodeScalars
            .compactMap { UnicodeScalar(127_397 + $0.value) }
            .map(String.init)
            .joined()
    }
}

extension Node {
    public var displayName: String {
        guard let flag = exitCountryCode?.flagEmoji, !flag.isEmpty else { return name }
        return "\(flag) \(name)"
    }

    /// Replicas in failover order.
    public var orderedReplicas: [Replica] {
        replicas.sorted { $0.replicaOrdinal < $1.replicaOrdinal }
    }
}
