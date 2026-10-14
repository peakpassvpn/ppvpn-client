import AppKit
import PPVPNAppLogic

/// Before anything else starts: a copy of PPVPN that is not where the
/// privileged service accepts it (``AppLocation``) offers to move itself to
/// /Applications or to quit. There is no "keep running here": enhanced mode
/// and the service would refuse it anyway.
///
/// The copy is made as Finder makes one (`ditto`, extended attributes kept as
/// they are): quarantine is neither added nor removed, so the first-open
/// approval the user gave carries over and nothing is stripped (removing it
/// leaves the app hanging at launch on macOS 26).
enum AppMover {
    /// Returns when the app may start; otherwise moves or quits, and exits.
    @MainActor static func moveIfNeeded() {
        #if DEBUG
        return
        #else
        let bundle = Bundle.main.bundleURL
        let original = AppLocation.isTranslocated(bundle.path) ? originalURL(ofTranslocated: bundle) : nil
        let decision = AppLocation.decide(
            bundlePath: bundle.path, originalPath: original?.path,
            readOnlyVolume: readOnlyVolume(of: original ?? bundle))
        guard case let .move(source, removeSource, volume) = decision else { return }

        let app = NSApplication.shared
        app.setActivationPolicy(.regular)
        app.activate(ignoringOtherApps: true)
        let alert = NSAlert()
        alert.messageText = tr("x_moveT")
        alert.informativeText = tr("x_moveD")
        alert.addButton(withTitle: tr("x_moveB"))
        alert.addButton(withTitle: tr("x_moveQuit"))
        guard alert.runModal() == .alertFirstButtonReturn else { exit(0) }

        do {
            try move(from: URL(fileURLWithPath: source), removeSource: removeSource)
            relaunch(ejecting: volume)
            exit(0)
        } catch {
            let failed = NSAlert()
            failed.alertStyle = .warning
            failed.messageText = tr("x_moveFailedT")
            failed.informativeText = tr("x_moveFailedD", ["error": error.localizedDescription])
            failed.addButton(withTitle: tr("x_moveQuit"))
            failed.runModal()
            exit(1)
        }
        #endif
    }

    private static let destination = URL(fileURLWithPath: AppLocation.installed)

    private struct Failure: LocalizedError {
        let errorDescription: String?
    }

    /// Quits a copy already running from /Applications, replaces it (asking
    /// for an administrator only when the plain copy is refused) and removes
    /// the source unless it lies on a read-only volume.
    private static func move(from source: URL, removeSource: Bool) throws {
        quitInstalledCopy()
        let fm = FileManager.default
        do {
            if fm.fileExists(atPath: destination.path) {
                try fm.trashItem(at: destination, resultingItemURL: nil)
            }
            try ditto(source, destination)
        } catch {
            try dittoAsAdmin(source, destination)
        }
        if removeSource {
            try? fm.trashItem(at: source, resultingItemURL: nil)
        }
    }

    private static func quitInstalledCopy() {
        let me = ProcessInfo.processInfo.processIdentifier
        let running = NSRunningApplication.runningApplications(
            withBundleIdentifier: Bundle.main.bundleIdentifier ?? "com.peakpassvpn.ppvpn.desktop")
            .filter { $0.processIdentifier != me && $0.bundleURL?.path == destination.path }
        for app in running { app.terminate() }
        let deadline = Date().addingTimeInterval(10)
        while running.contains(where: { !$0.isTerminated }), Date() < deadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.2))
        }
        for app in running where !app.isTerminated { app.forceTerminate() }
    }

    private static func ditto(_ source: URL, _ destination: URL) throws {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/ditto")
        process.arguments = [source.path, destination.path]
        let stderr = Pipe()
        process.standardError = stderr
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            try? FileManager.default.removeItem(at: destination)
            let message = String(decoding: stderr.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            throw Failure(errorDescription: message.trimmingCharacters(in: .whitespacesAndNewlines))
        }
    }

    /// The same copy behind the standard administrator prompt, for an
    /// /Applications the user may not write to (or an older copy owned by root).
    private static func dittoAsAdmin(_ source: URL, _ destination: URL) throws {
        func quoted(_ path: String) -> String { "'" + path.replacingOccurrences(of: "'", with: "'\\''") + "'" }
        let script = "/bin/rm -rf \(quoted(destination.path)) && /usr/bin/ditto \(quoted(source.path)) \(quoted(destination.path))"
        let literal = "\"" + script.replacingOccurrences(of: "\\", with: "\\\\").replacingOccurrences(of: "\"", with: "\\\"") + "\""
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", "do shell script \(literal) with administrator privileges"]
        let stderr = Pipe()
        process.standardError = stderr
        process.standardOutput = FileHandle.nullDevice
        try process.run()
        process.waitUntilExit()
        guard process.terminationStatus == 0 else {
            let message = String(decoding: stderr.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
            throw Failure(errorDescription: message.trimmingCharacters(in: .whitespacesAndNewlines))
        }
    }

    /// Opens the moved copy once this process has exited, then ejects the
    /// disk image it ran from (only a mounted image: `hdiutil info` lists it).
    private static func relaunch(ejecting volume: String?) {
        var script = "while /bin/kill -0 \(ProcessInfo.processInfo.processIdentifier) 2>/dev/null; do /bin/sleep 0.2; done; "
            + "/usr/bin/open \"$1\""
        if volume != nil {
            script += "; if /usr/bin/hdiutil info | /usr/bin/grep -qF \"$2\"; then /usr/bin/hdiutil detach \"$2\" -quiet; fi"
        }
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/sh")
        process.arguments = ["-c", script, "sh", destination.path, volume ?? ""]
        try? process.run()
    }

    /// The mount point of the volume `url` lies on when it is read-only.
    private static func readOnlyVolume(of url: URL) -> String? {
        guard let values = try? url.resourceValues(forKeys: [.volumeIsReadOnlyKey, .volumeURLKey]),
              values.volumeIsReadOnly == true else { return nil }
        return values.volume?.path
    }

    /// Where a translocated bundle was opened from: Security's
    /// SecTranslocateCreateOriginalPathForURL, looked up at run time (it is
    /// not in the public headers).
    private static func originalURL(ofTranslocated url: URL) -> URL? {
        typealias Original = @convention(c) (CFURL, UnsafeMutablePointer<Unmanaged<CFError>?>?) -> Unmanaged<CFURL>?
        guard let handle = dlopen("/System/Library/Frameworks/Security.framework/Security", RTLD_LAZY),
              let symbol = dlsym(handle, "SecTranslocateCreateOriginalPathForURL") else { return nil }
        let original = unsafeBitCast(symbol, to: Original.self)
        return original(url as CFURL, nil)?.takeRetainedValue() as URL?
    }
}
