import Foundation

/// Where the push agent keeps data and logs: the app's (the Flutter app's
/// apps/flutter/rust/src/dirs.rs), since both read the same sign-in. Neither
/// derives them from its own bundle.
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
