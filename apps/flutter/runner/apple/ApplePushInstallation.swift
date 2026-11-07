// Installation proof only, never an Apple provider key. No UserDefaults,
// shared app group, iCloud sync, bearer, OS token or diagnostics in this store.
import Foundation
import Security
import LocalAuthentication

struct ApplePushInstallation: Codable {
  let installationID: String
  let secret: String

  static func load(app: ApplePushApp, attempts: Int = 2) throws -> ApplePushInstallation {
    var query: [String: Any] = [
      kSecClass as String: kSecClassGenericPassword,
      kSecAttrService as String: app.bundleID + ".apple-push.installation",
      kSecAttrAccount as String: app.platform + "/" + app.environment.rawValue,
      kSecAttrSynchronizable as String: false,
    ]
    #if os(macOS)
    query[kSecUseDataProtectionKeychain as String] = true
    #endif
    var read = query
    read[kSecReturnData as String] = true
    read[kSecMatchLimit as String] = kSecMatchLimitOne
    let context = LAContext()
    context.interactionNotAllowed = true
    read[kSecUseAuthenticationContext as String] = context
    var found: CFTypeRef?
    let status = SecItemCopyMatching(read as CFDictionary, &found)
    if status == errSecSuccess {
      guard let data = found as? Data, let saved = try? JSONDecoder().decode(Self.self, from: data),
        UUID(uuidString: saved.installationID) != nil, saved.secret.utf8.count == 43,
        let decoded = Data(base64Encoded: saved.secret.replacingOccurrences(of: "-", with: "+").replacingOccurrences(of: "_", with: "/") + "="), decoded.count == 32
      else { throw Failure.invalid }
      return saved
    }
    // Locked/denied/corrupt stores must never silently rotate the installation.
    guard status == errSecItemNotFound else { throw Failure.unavailable }
    var bytes = [UInt8](repeating: 0, count: 32)
    guard SecRandomCopyBytes(kSecRandomDefault, bytes.count, &bytes) == errSecSuccess else { throw Failure.unavailable }
    let secret = Data(bytes).base64EncodedString().replacingOccurrences(of: "+", with: "-").replacingOccurrences(of: "/", with: "_").replacingOccurrences(of: "=", with: "")
    let fresh = Self(installationID: UUID().uuidString.lowercased(), secret: secret)
    query[kSecAttrAccessible as String] = kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly
    query[kSecValueData as String] = try JSONEncoder().encode(fresh)
    let added = SecItemAdd(query as CFDictionary, nil)
    if added == errSecDuplicateItem, attempts > 0 { return try load(app: app, attempts: attempts - 1) }
    guard added == errSecSuccess else { throw Failure.unavailable }
    return fresh
  }

  enum Failure: Error { case unavailable, invalid }
}
