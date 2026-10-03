import AppKit
import Foundation
import PPVPNAppLogic
import PPVPNClient

/// macOS side of ppvpn-client's `PlatformHooks`. Called from Rust threads.
final class MacPlatformHooks: PlatformHooks, @unchecked Sendable {
    private let credentials: CredentialStore

    init(credentials: CredentialStore = CredentialStores.default()) {
        self.credentials = credentials
    }

    // MARK: Credentials

    func credentialLoad() throws -> Data? {
        do {
            let data = try credentials.load()
            clientLog.info("credential_load: \(data == nil ? "empty" : "ok")")
            return data
        } catch {
            clientLog.error("credential_load failed: \(error.localizedDescription)")
            throw PlatformError.Failed(message: error.localizedDescription)
        }
    }

    func credentialSave(blob: Data) throws {
        do { try credentials.save(blob) } catch { throw PlatformError.Failed(message: error.localizedDescription) }
    }

    func credentialDelete() throws {
        do { try credentials.delete() } catch { throw PlatformError.Failed(message: error.localizedDescription) }
    }

    // MARK: Browser

    func openUrl(url: String) -> Bool {
        guard let url = URL(string: url), ["https", "http"].contains(url.scheme) else { return false }
        return NSWorkspace.shared.open(url)
    }

    // MARK: Privileged service

    // Paths are owned by service/src/install.rs.
    private static let launchDaemonPlist = "/Library/LaunchDaemons/com.peakpassvpn.ppvpn.service.plist"
    private static let serviceBinary =
        "/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service"

    func privilegedServiceInstalled() -> Bool {
        FileManager.default.fileExists(atPath: Self.launchDaemonPlist)
            && FileManager.default.fileExists(atPath: Self.serviceBinary)
    }

    func installPrivilegedService() throws {
        try runHelperAsAdmin("ppvpn-service-install")
    }

    func uninstallPrivilegedService() throws {
        try runHelperAsAdmin("ppvpn-service-uninstall")
    }

    /// Runs a bundled helper from Contents/MacOS as root behind the standard
    /// administrator password prompt.
    private func runHelperAsAdmin(_ name: String) throws {
        guard let helper = Bundle.main.executableURL?.deletingLastPathComponent().appending(path: name),
              FileManager.default.isExecutableFile(atPath: helper.path)
        else { throw PlatformError.Failed(message: "MACOS_PRIVILEGED_HELPER_MISSING:\(name)") }

        let shellQuoted = "'" + helper.path.replacingOccurrences(of: "'", with: "'\\''") + "'"
        let appleLiteral = "\"" + shellQuoted
            .replacingOccurrences(of: "\\", with: "\\\\")
            .replacingOccurrences(of: "\"", with: "\\\"") + "\""

        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/usr/bin/osascript")
        process.arguments = ["-e", "do shell script \(appleLiteral) with administrator privileges"]
        let stderr = Pipe()
        process.standardError = stderr
        process.standardOutput = FileHandle.nullDevice
        do { try process.run() } catch { throw PlatformError.Failed(message: error.localizedDescription) }
        process.waitUntilExit()
        guard process.terminationStatus != 0 else { return }

        let message = String(decoding: stderr.fileHandleForReading.readDataToEndOfFile(), as: UTF8.self)
        // -128 is AppleScript's userCanceledErr.
        if message.contains("-128") { throw PlatformError.Cancelled }
        throw PlatformError.Failed(message: "MACOS_ADMIN_AUTHORIZATION_FAILED:\(message.trimmingCharacters(in: .whitespacesAndNewlines))")
    }
}
