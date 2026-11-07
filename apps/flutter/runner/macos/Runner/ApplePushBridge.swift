import FlutterMacOS
import Cocoa

/// Installed at native launch; Flutter may attach much later. No bearer or
/// notification content crosses this channel. Only the authenticated Rust
/// transport may acknowledge registration, after the actual HTTP result.
@MainActor
final class MacApplePushBridge {
  static let shared = MacApplePushBridge()
  let receiver = MacApplePushReceiver()
  var showWindow: (() -> Void)?
  private var channel: FlutterMethodChannel?
  private var clientEpoch = "0"
  private var pendingEffects = 0
  private var drains: [FlutterResult] = []

  func start() { receiver.start() }

  func attach(_ registrar: FlutterPluginRegistrar) {
    let channel = FlutterMethodChannel(name: "com.peakpassvpn.ppvpn/apple-push", binaryMessenger: registrar.messenger)
    self.channel = channel
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self else { return result(nil) }
      switch call.method {
      case "configured": result(self.receiver.configured)
      case "installation":
        guard self.receiver.configured, let app = self.receiver.app,
          let scope = call.arguments as? [String: Any],
          scope["platform"] as? String == app.platform,
          scope["bundleID"] as? String == app.bundleID,
          scope["environment"] as? String == app.environment.rawValue else {
          return result(FlutterError(code: "push-scope", message: nil, details: nil))
        }
        do {
          let proof = try ApplePushInstallation.load(app: app)
          result(["installation_id": proof.installationID, "installation_secret": proof.secret])
        } catch {
          result(FlutterError(code: "push-store-unavailable", message: nil, details: nil))
        }
      case "drain":
        if self.pendingEffects == 0 { result(nil) }
        else { self.drains.append(result) }
      case "account":
        guard let args = call.arguments as? [String: Any], let state = args["state"] as? String,
          let epoch = args["clientEpoch"] as? String, !epoch.isEmpty, epoch.utf8.count <= 32 else {
          return result(FlutterError(code: "invalid-account", message: nil, details: nil))
        }
        self.clientEpoch = epoch
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
      self?.channel?.invokeMethod("register", arguments: Self.arguments(request, clientEpoch: self?.clientEpoch ?? "")) { [weak self] reply in
        self?.receiver.state.acknowledge(request, success: reply as? Bool == true)
        self?.finishEffect()
      }
    }, unregister: { [weak self] request in
      self?.pendingEffects += 1
      self?.channel?.invokeMethod("unregister", arguments: Self.arguments(request, clientEpoch: self?.clientEpoch ?? "")) { [weak self] _ in
        self?.finishEffect()
      }
    }, deliver: { [weak self] notice, opened in
      guard let self else { return }
      self.deliver(notice, opened: opened) { _ in }
    })

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
    channel.invokeMethod("notice", arguments: args) { [weak self] reply in
      let accepted = reply as? Bool == true
      if opened && accepted { self?.showWindow?() }
      completion(accepted)
    }
  }

  private static func arguments(_ r: ApplePushRegistration, clientEpoch: String) -> [String: Any] {
    ["clientEpoch": clientEpoch, "request": String(r.request), "accountGeneration": String(r.accountGeneration),
     "accountID": r.accountID, "token": r.token, "platform": r.app.platform,
     "bundleID": r.app.bundleID, "environment": r.app.environment.rawValue]
  }
}
