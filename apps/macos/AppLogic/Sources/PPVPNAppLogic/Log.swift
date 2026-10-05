import Foundation
#if canImport(os)
import os
#endif

/// The app's client log: the unified log on Apple platforms, stderr
/// elsewhere. Every message is public; the callers keep secrets out.
public struct AppLog: Sendable {
    public enum Level: Sendable { case debug, info, error }

    /// Replaceable sink, e.g. to capture lines in tests.
    nonisolated(unsafe) public static var sink: (@Sendable (Level, String) -> Void)?

    #if canImport(os)
    private let logger: Logger
    #endif

    public init(subsystem: String, category: String) {
        #if canImport(os)
        logger = Logger(subsystem: subsystem, category: category)
        #endif
    }

    public func debug(_ message: String) { write(.debug, message) }
    public func info(_ message: String) { write(.info, message) }
    public func error(_ message: String) { write(.error, message) }

    private func write(_ level: Level, _ message: String) {
        if let sink = Self.sink { return sink(level, message) }
        #if canImport(os)
        switch level {
        case .debug: logger.debug("\(message, privacy: .public)")
        case .info: logger.info("\(message, privacy: .public)")
        case .error: logger.error("\(message, privacy: .public)")
        }
        #else
        FileHandle.standardError.write(Data("[\(level)] \(message)\n".utf8))
        #endif
    }
}
