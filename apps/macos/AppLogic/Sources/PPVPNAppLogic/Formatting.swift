import Foundation
import PPVPNClient

extension UInt64 {
    public var byteCount: String {
        let formatter = ByteCountFormatter()
        formatter.countStyle = .binary
        formatter.allowsNonnumericFormatting = false // "0 KB", not "Zero KB"
        formatter.allowedUnits = [.useKB, .useMB, .useGB, .useTB] // never "字节"
        return formatter.string(fromByteCount: Int64(clamping: self))
    }

    public var byteRate: String { byteCount + "/s" }
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
