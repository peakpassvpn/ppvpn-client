import Foundation

/// When the main app should re-register a push agent that stopped running.
///
/// The agent touches `push-agent.heartbeat` every 60 s. A stale heartbeat is
/// only a hint (the Mac may just have slept), so healing also needs the agent
/// process to be gone, a full window since the app started or woke, and no
/// heal of this version in the last 24 h (`push-agent.heal`).
public enum PushAgentHealth {
    public static let heartbeatFile = "push-agent.heartbeat"
    public static let healFile = "push-agent.heal"
    /// Three missed heartbeats.
    public static let staleAfter: TimeInterval = 180
    public static let healInterval: TimeInterval = 24 * 60 * 60

    public struct HealRecord: Codable, Equatable, Sendable {
        public var version: String
        public var at: Date

        public init(version: String, at: Date) {
            self.version = version
            self.at = at
        }
    }

    /// - Parameters:
    ///   - heartbeat: modification time of the heartbeat file, nil if missing.
    ///   - watchingSince: when this launch started, or the last wake.
    public static func shouldHeal(heartbeat: Date?, watchingSince: Date, now: Date, agentRunning: Bool,
                                  lastHeal: HealRecord?, version: String) -> Bool {
        guard isDown(heartbeat: heartbeat, watchingSince: watchingSince, now: now, agentRunning: agentRunning)
        else { return false }
        guard let lastHeal, lastHeal.version == version else { return true }
        return now.timeIntervalSince(lastHeal.at) >= healInterval
    }

    /// The agent is not running and has missed its heartbeats for a full
    /// window: worth a plain restart (not rate-limited) before any heal.
    public static func isDown(heartbeat: Date?, watchingSince: Date, now: Date, agentRunning: Bool) -> Bool {
        guard now.timeIntervalSince(watchingSince) >= staleAfter else { return false }
        if let heartbeat, now.timeIntervalSince(heartbeat) < staleAfter { return false }
        return !agentRunning
    }

    public static func readHeal(in directory: URL) -> HealRecord? {
        guard let data = try? Data(contentsOf: directory.appendingPathComponent(healFile)) else { return nil }
        let decoder = JSONDecoder()
        decoder.dateDecodingStrategy = .iso8601
        return try? decoder.decode(HealRecord.self, from: data)
    }

    public static func writeHeal(_ record: HealRecord, in directory: URL) {
        let encoder = JSONEncoder()
        encoder.dateEncodingStrategy = .iso8601
        guard let data = try? encoder.encode(record) else { return }
        try? data.write(to: directory.appendingPathComponent(healFile), options: .atomic)
    }

    public static func heartbeatDate(in directory: URL) -> Date? {
        let path = directory.appendingPathComponent(heartbeatFile).path
        return (try? FileManager.default.attributesOfItem(atPath: path))?[.modificationDate] as? Date
    }
}

/// Where the app and its push agent keep data and logs; both processes must
/// agree, so neither derives them from its own bundle.
public enum AppDirectories {
    public static var data: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("PPVPN", isDirectory: true)
    }

    public static var logs: URL {
        FileManager.default.urls(for: .libraryDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("Logs/PPVPN", isDirectory: true)
    }

    /// Both directories, created if missing. The data directory holds the
    /// sign-in and the push token, so it is private to the user (0700).
    public static func prepare() -> (data: URL, logs: URL) {
        let (data, logs) = (data, logs)
        let files = FileManager.default
        try? files.createDirectory(at: data, withIntermediateDirectories: true, attributes: [.posixPermissions: 0o700])
        try? files.setAttributes([.posixPermissions: 0o700], ofItemAtPath: data.path)
        try? files.createDirectory(at: logs, withIntermediateDirectories: true)
        return (data, logs)
    }
}
