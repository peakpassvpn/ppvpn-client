// Dedicated push file: no service/window/packaging edits, agent cleanup or
// implicit registration. AppDelegate integration belongs to its owner.
import Cocoa
import Security
@preconcurrency import UserNotifications

@MainActor
final class MacApplePushReceiver: NSObject, UNUserNotificationCenterDelegate {
  let state: ApplePushState
  let configured: Bool
  var registrationFailed: ((String, Int) -> Void)?

  init(bundle: Bundle = .main) {
    var entitlement: String?
    if let task = SecTaskCreateFromSelf(nil) {
      entitlement = SecTaskCopyValueForEntitlement(task,
        "com.apple.developer.aps-environment" as CFString, nil) as? String
    }
    let app = ApplePushApp(platform: "macos", bundleID: bundle.bundleIdentifier, entitlement: entitlement)
    configured = bundle.object(forInfoDictionaryKey: "PPVPNApplePushEnabled") as? Bool == true && app != nil
    state = ApplePushState(app: configured ? app : nil)
  }

  @discardableResult
  func start() -> Bool {
    guard configured else { return false }
    UNUserNotificationCenter.current().delegate = self
    NSApplication.shared.registerForRemoteNotifications()
    return true
  }

  func requestPermission() {
    guard configured else { return }
    UNUserNotificationCenter.current().getNotificationSettings { settings in
      guard settings.authorizationStatus == .notDetermined else { return }
      UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { _, _ in }
    }
  }

  func didRegister(token: Data) { if configured { state.receiveToken(token) } }

  func didFailToRegister(_ error: Error) {
    guard configured else { return }
    let failure = error as NSError
    // No token, notification body, localized error text or bearer is logged.
    registrationFailed?(failure.domain, failure.code)
  }

  // Forward NSApplicationDelegate's didReceiveRemoteNotification for a
  // refresh hint. This does not claim to keep the application resident.
  func receiveBackground(_ userInfo: [AnyHashable: Any]) {
    guard configured, let notice = ApplePushNotice(userInfo: userInfo, fallbackID: UUID().uuidString) else { return }
    state.receive(notice, opened: false)
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
    let notice = ApplePushNotice(userInfo: notification.request.content.userInfo, fallbackID: notification.request.identifier)
    DispatchQueue.main.async { [weak self] in
      guard let self, self.configured, let notice else { completionHandler([]); return }
      let accepted = self.state.receive(notice, opened: false)
      completionHandler(accepted && notice.accountID != nil ? [.banner, .list, .sound] : [])
    }
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void) {
    if response.actionIdentifier == UNNotificationDefaultActionIdentifier {
      let notice = ApplePushNotice(userInfo: response.notification.request.content.userInfo,
        fallbackID: response.notification.request.identifier)
      DispatchQueue.main.async { [weak self] in
        guard let self, self.configured, let notice else { return }
        self.state.receive(notice, opened: true)
      }
    }
    completionHandler()
  }
}
