// Shared APNs lifecycle. No credentials, token persistence, payload content,
// networking or notification presentation belongs in this state machine.
import Foundation
import CoreFoundation
import os

// Typed, content-free native callback receipts. Kept in the already included
// common compilation unit; the state machine itself does not emit logs.
@MainActor
enum ApplePushTrace {
  enum Stage: String, CaseIterable {
    case registerRequested = "register_requested"
    case tokenReceived = "token_received"
    case registrationFailed = "registration_failed"
    case permissionResult = "permission_result"
    case foregroundReceived = "foreground_received"
    case backgroundReceived = "background_received"
    case tapReceived = "tap_received"
    case bridgeRegisterReply = "bridge_register_reply"
    case bridgeUnregisterReply = "bridge_unregister_reply"
    case bridgeNoticeReply = "bridge_notice_reply"
  }

  enum FailureDomain: String {
    case none, cocoa, url, osStatus = "os_status", other
    init(_ error: NSError?) {
      guard let error else { self = .none; return }
      switch error.domain {
      case NSCocoaErrorDomain: self = .cocoa
      case NSURLErrorDomain: self = .url
      case NSOSStatusErrorDomain: self = .osStatus
      default: self = .other // Never log arbitrary domain/description/userInfo.
      }
    }
  }

  struct Receipt {
    let stage: Stage
    let accepted: Bool
    let current: Bool
    let opened: Bool
    let remote: Bool
    let failureDomain: FailureDomain
    let code: Int

    init(_ stage: Stage, accepted: Bool = false, current: Bool = true,
      opened: Bool = false, remote: Bool = false, error: NSError? = nil) {
      self.stage = stage; self.accepted = accepted; self.current = current
      self.opened = opened; self.remote = remote
      failureDomain = FailureDomain(error); code = error?.code ?? 0
    }

    // No string parameter can carry a token, owner, delivery/message id,
    // nonce, URL, payload or error description into this serialization.
    var line: String {
      "apns stage=\(stage.rawValue) accepted=\(accepted ? 1 : 0) current=\(current ? 1 : 0) opened=\(opened ? 1 : 0) remote=\(remote ? 1 : 0) domain=\(failureDomain.rawValue) code=\(code)"
    }
  }

  private static let logger = Logger(subsystem: "com.peakpassvpn.ppvpn.apple-push", category: "lifecycle")
  static func emit(_ receipt: Receipt) {
    logger.info("\(receipt.line, privacy: .public)")
  }
}

enum ApplePushEnvironment: String, Codable, Sendable {
  case sandbox, production

  init?(entitlement: String?) {
    switch entitlement {
    case "development": self = .sandbox
    case "production": self = .production
    default: return nil
    }
  }
}

struct ApplePushApp: Equatable, Sendable {
  let platform: String
  let bundleID: String
  let environment: ApplePushEnvironment

  // The builder must derive this value from the signed entitlement/profile
  // and verify it after export. DEBUG and the receipt environment are not
  // APNs environment selectors (TestFlight uses production APNs).
  init?(platform: String, bundleID: String?, entitlement: String?) {
    guard ["ios", "macos"].contains(platform), let bundleID, !bundleID.isEmpty,
      let environment = ApplePushEnvironment(entitlement: entitlement) else { return nil }
    self.platform = platform
    self.bundleID = bundleID
    self.environment = environment
  }
}

struct ApplePushRegistration: Equatable, Sendable {
  let request: UInt64
  let accountGeneration: UInt64
  let accountID: String
  let token: String
  let app: ApplePushApp
}

struct ApplePushNotice: Equatable, Sendable {
  let deliveryID: String
  let accountID: String?
  let messageID: Int64?

  init?(userInfo: [AnyHashable: Any], fallbackID: String) {
    let extra = userInfo["extra"] as? [AnyHashable: Any] ?? userInfo
    let delivery = extra["delivery_id"] as? String ?? fallbackID
    guard !delivery.isEmpty, delivery.utf8.count <= 256 else { return nil }
    deliveryID = delivery
    // A present, malformed owner must not become an unscoped notification.
    if let owner = extra["account_id"] {
      guard let owner = owner as? String, !owner.isEmpty, owner.utf8.count <= 256 else { return nil }
      accountID = owner
    } else {
      accountID = nil
    }
    var text: String?
    if let number = extra["message_id"] as? NSNumber {
      if CFGetTypeID(number) != CFBooleanGetTypeID() { text = number.stringValue }
    } else {
      text = extra["message_id"] as? String
    }
    if let text, !text.isEmpty, text.utf8.allSatisfy({ (48...57).contains($0) }),
      let id = Int64(text), id > 0 { messageID = id }
    else { messageID = nil }
    // title, body and deep_link deliberately never cross this boundary.
  }
}

@MainActor
final class ApplePushState {
  private enum Account: Equatable { case restoring, signedOut, signedIn(String) }
  private struct Pending { let notice: ApplePushNotice; let opened: Bool }
  private let app: ApplePushApp?
  private var account: Account = .restoring
  private var accountGeneration: UInt64 = 0
  private var nextRequest: UInt64 = 0
  private var token: String?
  private var registering: ApplePushRegistration?
  private var registered: ApplePushRegistration?
  private var pending: [Pending] = []
  private var opened: [String] = []
  private var received: [String] = []
  private var register: ((ApplePushRegistration) -> Void)?
  private var unregister: ((ApplePushRegistration) -> Void)?
  private var deliver: ((ApplePushNotice, Bool) -> Void)?
  private let capacity: Int

  init(app: ApplePushApp?, capacity: Int = 64) {
    self.app = app
    self.capacity = max(1, capacity)
  }

  func attach(register: @escaping (ApplePushRegistration) -> Void,
              unregister: @escaping (ApplePushRegistration) -> Void,
              deliver: @escaping (ApplePushNotice, Bool) -> Void) {
    self.register = register
    self.unregister = unregister
    self.deliver = deliver
    requestRegistration()
    drain()
  }

  // Call with the stable authenticated account id, never its display name.
  func beginRestoration() {
    guard account != .restoring else { return }
    invalidateRegistration()
    accountGeneration &+= 1
    account = .restoring
    pending.removeAll()
    opened.removeAll()
    received.removeAll()
  }

  // Call nil on explicit logout or after restoration finds no signed-in user.
  // The transport must serialize these effects with auth logout, while the
  // old bearer is still usable; this class never possesses that bearer.
  func setAccount(_ id: String?) {
    let next: Account = id.flatMap { $0.isEmpty ? nil : $0 }.map(Account.signedIn) ?? .signedOut
    guard next != account else { return }
    let wasRestoring = account == .restoring
    invalidateRegistration()
    accountGeneration &+= 1
    account = next
    opened.removeAll()
    received.removeAll()
    if !wasRestoring || next == .signedOut { pending.removeAll() }
    requestRegistration()
    drain()
  }

  // Only accept a fresh OS callback in this process. No disk-cached token
  // is replayed at launch. APNs tokens have no fixed byte length.
  func receiveToken(_ bytes: Data) {
    guard !bytes.isEmpty else { return }
    let next = bytes.map { String(format: "%02x", $0) }.joined()
    if next != token {
      // Token rotation updates the same binding; only account invalidation
      // unbinds. Late old-token acknowledgements cannot settle this request.
      registered = nil
      registering = nil
      token = next
    }
    requestRegistration()
  }

  func requestRegistration() {
    guard registering == nil, registered == nil, let app, let token, let register,
      case .signedIn(let owner) = account else { return }
    nextRequest &+= 1
    let request = ApplePushRegistration(request: nextRequest, accountGeneration: accountGeneration,
                                        accountID: owner, token: token, app: app)
    registering = request
    register(request)
  }

  // An old account's/old token's asynchronous success cannot confirm a new
  // registration. A failed current request may be retried by the transport.
  @discardableResult
  func acknowledge(_ request: ApplePushRegistration, success: Bool) -> Bool {
    guard registering == request, request.accountGeneration == accountGeneration,
      account == .signedIn(request.accountID), request.token == token else { return false }
    registering = nil
    if success { registered = request }
    return true
  }

  private func invalidateRegistration() {
    let old = registered ?? registering
    registered = nil
    registering = nil
    if let old { unregister?(old) }
  }

  // Foreground receive and user tap have distinct dedupe histories: showing
  // a banner must not consume the later tap. Deduplication is bounded and
  // process-local; the backend must also deduplicate its delivery records.
  @discardableResult
  func receive(_ notice: ApplePushNotice, opened: Bool) -> Bool {
    if account == .signedOut { return false }
    guard case .signedIn(let owner) = account, let deliver else {
      if pending.count == capacity { pending.removeFirst() }
      pending.append(Pending(notice: notice, opened: opened))
      return false
    }
    guard notice.accountID == nil || notice.accountID == owner else { return false }
    var history = opened ? self.opened : received
    guard !history.contains(notice.deliveryID) else { return false }
    if history.count == capacity { history.removeFirst() }
    history.append(notice.deliveryID)
    if opened { self.opened = history } else { received = history }
    // An unscoped legacy payload can only trigger an authenticated inbox
    // lookup. It can never provide trusted content or a URL to navigate.
    deliver(notice, opened)
    return true
  }

  private func drain() {
    guard deliver != nil, case .signedIn = account else { return }
    let buffered = pending
    pending.removeAll()
    for item in buffered { receive(item.notice, opened: item.opened) }
  }
}

enum ApplePushFetchResult: Equatable, Sendable { case newData, noData, failed }

@MainActor
final class ApplePushCompletion {
  private var completion: ((ApplePushFetchResult) -> Void)?
  init(_ completion: @escaping (ApplePushFetchResult) -> Void) { self.completion = completion }
  @discardableResult
  func finish(_ result: ApplePushFetchResult) -> Bool {
    guard let completion else { return false }
    self.completion = nil
    completion(result)
    return true
  }
}
