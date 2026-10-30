import AppKit
import Foundation
import Security
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
        try runHelperAsAdmin("ppvpn-service-install", alongside: ["ppvpn-service"])
    }

    func uninstallPrivilegedService() throws {
        try runHelperAsAdmin("ppvpn-service-uninstall")
    }

    /// Runs a bundled helper from Contents/MacOS as root behind the standard
    /// administrator password prompt.
    ///
    /// The bundle belongs to the user, so in a release the root shell first
    /// copies the helper (and the files it installs) into a directory only
    /// root can write, checks the copy with /usr/bin/codesign against the
    /// helper's identifier and a Developer ID signature of the team that
    /// signed this app, and runs the copy: a helper changed on disk is never
    /// run as root.
    /// ppvpn-service-install checks the service binary the same way
    /// (crates/service/src/install.rs).
    private func runHelperAsAdmin(_ name: String, alongside: [String] = []) throws {
        guard let directory = Bundle.main.executableURL?.deletingLastPathComponent() else {
            throw PlatformError.Failed(message: "MACOS_PRIVILEGED_HELPER_MISSING:\(name)")
        }
        for file in [name] + alongside where !FileManager.default.isExecutableFile(atPath: directory.appending(path: file).path) {
            throw PlatformError.Failed(message: "MACOS_PRIVILEGED_HELPER_MISSING:\(file)")
        }
        func quoted(_ text: String) -> String { "'" + text.replacingOccurrences(of: "'", with: "'\\''") + "'" }

        #if DEBUG
        // Ad-hoc signed: nothing to check against.
        let script = quoted(directory.appending(path: name).path)
        #else
        guard let team = Self.signingTeamIdentifier() else {
            throw PlatformError.Failed(message: "MACOS_PRIVILEGED_HELPER_UNSIGNED")
        }
        let identifier = "com.peakpassvpn.ppvpn." + name.replacingOccurrences(of: "ppvpn-", with: "")
        // The service's requirement (crates/service/src/macsign.rs).
        let requirement = "=anchor apple generic and identifier \"\(identifier)\""
            + " and certificate 1[field.1.2.840.113635.100.6.2.6]"
            + " and certificate leaf[field.1.2.840.113635.100.6.1.13]"
            + " and certificate leaf[subject.OU] = \"\(team)\""
        let copies = ([name] + alongside)
            .map { "/bin/cp \(quoted(directory.appending(path: $0).path)) \"$d/\"" }
            .joined(separator: " && ")
        let script = "d=$(/usr/bin/mktemp -d /tmp/ppvpn-helper.XXXXXX) || exit 1; "
            + "trap '/bin/rm -rf \"$d\"' EXIT; "
            + "\(copies) && /usr/bin/codesign --verify --strict --test-requirement \(quoted(requirement)) \"$d/\(name)\" && \"$d/\(name)\""
        #endif
        let appleLiteral = "\"" + script
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

    /// The Team ID of this app's Developer ID signature (#471): ten
    /// upper-case letters and digits, nil without one.
    private static func signingTeamIdentifier() -> String? {
        var code: SecCode?
        var staticCode: SecStaticCode?
        var information: CFDictionary?
        guard SecCodeCopySelf([], &code) == errSecSuccess, let code,
              SecCodeCopyStaticCode(code, [], &staticCode) == errSecSuccess, let staticCode,
              SecCodeCopySigningInformation(staticCode, SecCSFlags(rawValue: kSecCSSigningInformation), &information)
                == errSecSuccess,
              let team = (information as? [String: Any])?[kSecCodeInfoTeamIdentifier as String] as? String,
              team.range(of: "^[A-Z0-9]{10}$", options: .regularExpression) != nil
        else { return nil }
        return team
    }
}
