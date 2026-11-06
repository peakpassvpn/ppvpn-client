// Explicit host integration is required; this file never starts itself.
// Keep the receiver alive in the AppDelegate/plugin and install at launch.
import UIKit
@preconcurrency import UserNotifications

@MainActor
final class IOSApplePushReceiver: NSObject, UNUserNotificationCenterDelegate {
  let state: ApplePushState
  let configured: Bool
  var registrationFailed: ((String, Int) -> Void)?
  var fetchBackground: ((ApplePushNotice, @escaping (ApplePushFetchResult) -> Void) -> Void)?

  init(bundle: Bundle = .main) {
    // There is no supported SecTask entitlement reader in the iOS SDK.
    // Packaging must copy the actual signed/profile environment here and
    // verify equality after archive export; never infer it from DEBUG or
    // an App Store receipt (TestFlight receipts are not an APNs selector).
    let app = ApplePushApp(platform: "ios", bundleID: bundle.bundleIdentifier,
      entitlement: bundle.object(forInfoDictionaryKey: "PPVPNAPNsEntitlementEnvironment") as? String)
    configured = bundle.object(forInfoDictionaryKey: "PPVPNApplePushEnabled") as? Bool == true && app != nil
    state = ApplePushState(app: configured ? app : nil)
  }

  @discardableResult
  func start() -> Bool {
    guard configured else { return false }
    UNUserNotificationCenter.current().delegate = self
    // Ask the OS each launch/retry. Do not restore a token from a file.
    UIApplication.shared.registerForRemoteNotifications()
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

  // Forward from didReceiveRemoteNotification:fetchCompletionHandler:.
  // A native/Dart authenticated fetch hook must complete; a deadline is scheduled
  // at 25s, below Apple's 30s budget (not a real-time scheduling guarantee). Missing auth/handler returns noData.
  func receiveBackground(_ userInfo: [AnyHashable: Any], completion: @escaping (UIBackgroundFetchResult) -> Void) {
    guard configured, let notice = ApplePushNotice(userInfo: userInfo, fallbackID: UUID().uuidString),
      state.receive(notice, opened: false), let fetchBackground else { completion(.noData); return }
    let once = ApplePushCompletion { result in
      switch result {
      case .newData: completion(.newData)
      case .noData: completion(.noData)
      case .failed: completion(.failed)
      }
    }
    DispatchQueue.main.asyncAfter(deadline: .now() + 25) { once.finish(.failed) }
    fetchBackground(notice) { result in
      DispatchQueue.main.async { once.finish(result) }
    }
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
    let notice = ApplePushNotice(userInfo: notification.request.content.userInfo, fallbackID: notification.request.identifier)
    DispatchQueue.main.async { [weak self] in
      guard let self, self.configured, let notice else { completionHandler([]); return }
      let accepted = self.state.receive(notice, opened: false)
      // Unscoped legacy data is only an authenticated refresh hint. Its
      // payload body is not shown by the foreground app.
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
