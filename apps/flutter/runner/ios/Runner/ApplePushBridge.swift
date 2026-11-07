import Flutter
import UIKit

/// Installed at native launch; Flutter may attach much later. No bearer or
/// notification content crosses this channel. Only the authenticated Rust
/// transport may acknowledge registration, after the actual HTTP result.
@MainActor
final class IOSApplePushBridge {
  static let shared = IOSApplePushBridge()
  let receiver = IOSApplePushReceiver()
  private var channel: FlutterMethodChannel?
  private var pendingEffects = 0
  private var drains: [FlutterResult] = []

  func start() { receiver.start() }

  func attach(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(name: "com.peakpassvpn.ppvpn/apple-push", binaryMessenger: registrar.messenger())
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "configured": result(self.receiver.configured)
      case "drain":
        if self.pendingEffects == 0 { result(nil) }
        else { self.drains.append(result) }
      case "account":
        guard let args = call.arguments as? [String: Any], let state = args["state"] as? String else {
          return result(FlutterError(code: "invalid-account", message: nil, details: nil))
        }
        switch state {
        case "restoring": self.receiver.state.beginRestoration()
        case "signedOut": self.receiver.state.setAccount(nil)
        case "signedIn":
          guard let id = args["id"] as? String, !id.isEmpty else {
            return result(FlutterError(code: "invalid-account", message: nil, details: nil))
          }
          self.receiver.state.setAccount(id)
        default: return result(FlutterError(code: "invalid-account", message: nil, details: nil))
        }
        result(nil)
      case "retry": self.receiver.start(); self.receiver.state.requestRegistration(); result(nil)
      case "permission": self.receiver.requestPermission(); result(nil)
      default: result(FlutterMethodNotImplemented)
      }
    }
    receiver.state.attach(register: { [weak self] request in
      self?.pendingEffects += 1
      self?.channel?.invokeMethod("register", arguments: Self.arguments(request)) { [weak self] reply in
        self?.receiver.state.acknowledge(request, success: reply as? Bool == true)
        self?.finishEffect()
      }
    }, unregister: { [weak self] request in
      self?.pendingEffects += 1
      self?.channel?.invokeMethod("unregister", arguments: Self.arguments(request)) { [weak self] _ in
        self?.finishEffect()
      }
    }, deliver: { [weak self] notice, opened in
      guard let self, opened || !self.receiver.receivingBackground else { return }
      self.deliver(notice, opened: opened) { _ in }
    })
    receiver.fetchBackground = { [weak self] notice, completion in
      guard let self else { return completion(.noData) }
      self.deliver(notice, opened: false) { reply in completion(reply ? .newData : .noData) }
    }
  }

  private func finishEffect() {
    pendingEffects -= 1
    guard pendingEffects == 0 else { return }
    let waiting = drains
    drains.removeAll()
    for done in waiting { done(nil) }
  }

  private func deliver(_ notice: ApplePushNotice, opened: Bool, completion: @escaping (Bool) -> Void) {
    guard let channel else { return completion(false) }
    var args: [String: Any] = ["deliveryID": notice.deliveryID, "opened": opened]
    if let id = notice.accountID { args["accountID"] = id }
    // Decimal strings avoid JavaScript/Dart web integer precision surprises.
    if let id = notice.messageID { args["messageID"] = String(id) }
    channel.invokeMethod("notice", arguments: args) { reply in completion(reply as? Bool == true) }
  }

  private static func arguments(_ r: ApplePushRegistration) -> [String: Any] {
    ["request": String(r.request), "accountGeneration": String(r.accountGeneration),
     "accountID": r.accountID, "token": r.token, "platform": r.app.platform,
     "bundleID": r.app.bundleID, "environment": r.app.environment.rawValue]
  }
}
