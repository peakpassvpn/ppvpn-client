import Foundation
import PPVPNAppLogic
import Security

/// Backs ppvpn-client's credential_load/save/delete hooks: one opaque blob,
/// one storage entry.
///
/// Builds are signed without an Apple Team ID (self-signed or ad-hoc), and
/// the login keychain partitions such an app's items by its cdhash, which
/// changes with every build: each update would ask for the keychain password
/// again. So the sign-in lives in a 0600 file in the 0700 data directory
/// instead, excluded from Time Machine and deleted on sign-out (the client
/// calls `delete()`). Debug builds use their own file.
/// TODO: back to `KeychainCredentialStore` once builds carry a Developer ID.
protocol CredentialStore: Sendable {
    func load() throws -> Data?
    func save(_ data: Data) throws
    func delete() throws
}

enum CredentialStores {
    static func `default`() -> CredentialStore {
        #if DEBUG
        let name = "credentials.debug"
        #else
        let name = "credentials"
        #endif
        return FileCredentialStore(url: AppDirectories.data.appendingPathComponent(name))
    }
}

struct KeychainError: LocalizedError {
    let status: OSStatus
    var errorDescription: String? {
        (SecCopyErrorMessageString(status, nil) as String?) ?? "Keychain error \(status)"
    }
}

struct KeychainCredentialStore: CredentialStore {
    let service: String
    let account: String

    private var query: [String: Any] {
        [kSecClass as String: kSecClassGenericPassword,
         kSecAttrService as String: service,
         kSecAttrAccount as String: account]
    }

    func load() throws -> Data? {
        var request = query
        request[kSecReturnData as String] = true
        request[kSecMatchLimit as String] = kSecMatchLimitOne
        var result: CFTypeRef?
        let status = SecItemCopyMatching(request as CFDictionary, &result)
        switch status {
        case errSecSuccess: return result as? Data
        case errSecItemNotFound: return nil
        default: throw KeychainError(status: status)
        }
    }

    func save(_ data: Data) throws {
        let update = SecItemUpdate(query as CFDictionary, [kSecValueData as String: data] as CFDictionary)
        switch update {
        case errSecSuccess:
            return
        case errSecItemNotFound:
            var item = query
            item[kSecValueData as String] = data
            item[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
            let status = SecItemAdd(item as CFDictionary, nil)
            guard status == errSecSuccess else { throw KeychainError(status: status) }
        default:
            throw KeychainError(status: update)
        }
    }

    func delete() throws {
        let status = SecItemDelete(query as CFDictionary)
        guard status == errSecSuccess || status == errSecItemNotFound else { throw KeychainError(status: status) }
    }
}

struct FileCredentialStore: CredentialStore {
    let url: URL

    func load() throws -> Data? {
        FileManager.default.fileExists(atPath: url.path) ? try Data(contentsOf: url) : nil
    }

    /// Written to a 0600 temporary file and renamed over the old one, so the
    /// credentials are never readable by others, not even briefly. No
    /// file-protection class: .complete makes the file unreadable while the
    /// screen is locked, which breaks restore at login.
    func save(_ data: Data) throws {
        let directory = url.deletingLastPathComponent()
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let temporary = directory.appendingPathComponent(".\(url.lastPathComponent).\(UUID().uuidString)")
        guard FileManager.default.createFile(atPath: temporary.path, contents: data,
                                             attributes: [.posixPermissions: 0o600]) else {
            throw CocoaError(.fileWriteUnknown, userInfo: [NSFilePathErrorKey: temporary.path])
        }
        guard rename(temporary.path, url.path) == 0 else {
            let error = POSIXError(POSIXErrorCode(rawValue: errno) ?? .EIO)
            try? FileManager.default.removeItem(at: temporary)
            throw error
        }
        // Keep the sign-in out of Time Machine backups.
        var stored = url
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try? stored.setResourceValues(values)
    }

    func delete() throws {
        if FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.removeItem(at: url)
        }
    }
}
