import Foundation
import PPVPNClient

// ppvpn-client reports failures as a stable `ErrorCode` plus an upstream
// `detail`. Only the code is shown: its text comes from Errors.xcstrings,
// keyed by the Rust variant name (append-only upstream). The detail goes to
// the log for bug reports.

public let clientLog = AppLog(subsystem: "com.peakpassvpn.ppvpn.desktop", category: "client")

extension ErrorCode {
    /// Rust variant name, e.g. `NetworkUnreachable`.
    public var key: String {
        let name = String(describing: self)
        return name.prefix(1).uppercased() + name.dropFirst()
    }

    public var message: String {
        let text = Localization.localizer.string(key, table: "Errors")
        return text == key ? tr("x_unknownError", ["c": key]) : text
    }
}

extension ClientErrorInfo {
    public var message: String { code.message }

    public func log(_ context: StaticString) {
        clientLog.error("\(context): \(code.key) — \(detail)")
    }
}

extension ClientError {
    public var userMessage: String {
        switch self {
        case .NotSignedIn: Self.shared("NotSignedIn")
        case .StandardNotReady: Self.shared("StandardNotReady")
        case .Failed(let code, _): code.message
        case .NotImplemented: Self.shared("NotImplemented")
        // Never presented: `AppModel.perform` swallows cancellations.
        case .Cancelled: ""
        }
    }

    /// Texts for client-side errors live in Errors.xcstrings as well.
    private static func shared(_ key: String) -> String {
        Localization.localizer.string(key, table: "Errors")
    }

    public func log(_ context: StaticString) {
        if case .Failed(let code, let detail) = self {
            ClientErrorInfo(code: code, detail: detail).log(context)
        } else {
            clientLog.error("\(context): \(String(describing: self))")
        }
    }
}
