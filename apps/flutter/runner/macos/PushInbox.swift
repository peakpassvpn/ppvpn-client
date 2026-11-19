// Native callbacks can arrive before Dart starts. Main-queue only; no UI or
// network dependencies, so cold launches and rotation can be tested directly.
import Foundation

final class MacPushInbox {
  private var token: String?
  private var pending: [String: String]?
  private var deliver: ((String, [String: String]) -> Void)?

  func start(deliver: @escaping (String, [String: String]) -> Void) {
    self.deliver = deliver
    if let token { deliver("token", ["token": token]) }
    if let pending {
      self.pending = nil
      deliver("opened", pending)
    }
  }

  func receiveToken(_ token: String) {
    self.token = token
    deliver?("token", ["token": token])
  }

  func receiveTap(_ info: [AnyHashable: Any]) {
    receiveTapPayload(Self.tapPayload(info))
  }

  static func tapPayload(_ info: [AnyHashable: Any]) -> [String: String] {
    let extra = info["extra"] as? [AnyHashable: Any] ?? info
    var payload: [String: String] = [:]
    // Only delivery_id / message_id cross into Dart. Never forward a payload's
    // account_id, body or URL as trusted identity or inbox content (#539).
    if let delivery = extra["delivery_id"] as? String, !delivery.isEmpty {
      payload["delivery_id"] = delivery
    }
    if let id = extra["message_id"] as? String { payload["message_id"] = id }
    else if let id = extra["message_id"] as? NSNumber { payload["message_id"] = id.stringValue }
    return payload
  }

  func receiveTapPayload(_ payload: [String: String]) {
    if let deliver { deliver("opened", payload) }
    else { pending = payload }
  }
}
