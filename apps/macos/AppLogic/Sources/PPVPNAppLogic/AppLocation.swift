import Foundation

/// Where the app must run from. The privileged service accepts only a client
/// at these paths (crates/service/src/ipc.rs, `macos_client_path_allowed`),
/// so a copy anywhere else (the mounted disk image, ~/Downloads,
/// ~/Applications, a Gatekeeper-translocated path) is moved before it starts.
public enum AppLocation {
    /// Where a release build is moved to.
    public static let installed = "/Applications/PPVPN.app"
    /// The paths the service accepts, the development build's included.
    public static let allowed: Set<String> = [installed, "/Applications/PPVPN（开发版）.app"]

    public enum Decision: Equatable, Sendable {
        /// Already where the service accepts it.
        case stay
        /// Move `source` to ``installed``. `removeSource`: delete the copy that
        /// ran (not on a read-only volume such as a disk image). `volume`: the
        /// read-only volume it ran from, ejected after the move when it is a
        /// disk image.
        case move(source: String, removeSource: Bool, volume: String?)
    }

    /// - Parameters:
    ///   - bundlePath: the running bundle's path (`Bundle.main.bundlePath`).
    ///   - originalPath: for a translocated bundle, the path it was opened
    ///     from (nil when unknown or not translocated).
    ///   - readOnlyVolume: the mount point of the volume the original lies on
    ///     when that volume is read-only, else nil.
    public static func decide(bundlePath: String, originalPath: String?, readOnlyVolume: String?) -> Decision {
        let running = normalized(bundlePath)
        if allowed.contains(running) { return .stay }
        let source = originalPath.map(normalized) ?? running
        // Opened from /Applications but translocated: moving cannot help and
        // opening it again would translocate it again. The service's
        // rejection message covers it.
        if allowed.contains(source) { return .stay }
        return .move(source: source, removeSource: readOnlyVolume == nil, volume: readOnlyVolume.map(normalized))
    }

    /// Whether `path` is under App Translocation's randomized mount.
    public static func isTranslocated(_ path: String) -> Bool {
        path.contains("/AppTranslocation/")
    }

    private static func normalized(_ path: String) -> String {
        var path = (path as NSString).standardizingPath
        while path.count > 1, path.hasSuffix("/") { path.removeLast() }
        return path
    }
}
