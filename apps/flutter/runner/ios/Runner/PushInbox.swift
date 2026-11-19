// APNs events can precede both the Flutter engine and Dart's channel handler.
// Accessed on the main queue by PPVPNPush. Kept separate from UIKit so the
// cold-start, rotation and provisioning contracts can be tested with swiftc.
import Foundation

final class PushInbox {
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
    let extra = info["extra"] as? [AnyHashable: Any] ?? info
    var payload: [String: String] = [:]
    // Only delivery_id / message_id cross into Dart. The signed-in account's
    // backend endpoint resolves the message; payload account_id, body or URL
    // never become trusted identity or inbox content (#539).
    if let delivery = extra["delivery_id"] as? String, !delivery.isEmpty {
      payload["delivery_id"] = delivery
    }
    if let id = extra["message_id"] as? String { payload["message_id"] = id }
    else if let id = extra["message_id"] as? NSNumber { payload["message_id"] = id.stringValue }
    if let deliver { deliver("opened", payload) }
    else { pending = payload }
  }

  static func apnsEnvironment(provisionData: Data?, simulator: Bool = false) -> String {
    if let data = provisionData,
       let start = data.range(of: Data("<plist".utf8)),
       let end = data.range(of: Data("</plist>".utf8), in: start.lowerBound..<data.endIndex)
    {
      let xml = data.subdata(in: start.lowerBound..<end.upperBound)
      if let plist = try? PropertyListSerialization.propertyList(from: xml, format: nil) as? [String: Any],
         let entitlements = plist["Entitlements"] as? [String: Any],
         let aps = entitlements["aps-environment"] as? String
      {
        if aps == "development" { return "sandbox" }
        if aps == "production" { return "production" }
      }
    }
    return simulator ? "sandbox" : "production"
  }
}
