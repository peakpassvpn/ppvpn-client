// Explicit host integration is required; this file never starts itself.
// Keep the receiver alive in the AppDelegate/plugin and install at launch.
import UIKit
@preconcurrency import UserNotifications

@MainActor
struct IOSApplePushSystem {
  var installDelegate: (IOSApplePushReceiver) -> Void
  var register: () -> Void
  var permission: () -> Void

  static var live: Self {
    Self(installDelegate: { UNUserNotificationCenter.current().delegate = $0 },
      register: { UIApplication.shared.registerForRemoteNotifications() }, permission: {
        UNUserNotificationCenter.current().getNotificationSettings { settings in
          guard settings.authorizationStatus == .notDetermined else { return }
          UNUserNotificationCenter.current().requestAuthorization(options: [.alert, .badge, .sound]) { _, _ in }
        }
      })
  }
}

@MainActor
final class IOSApplePushReceiver: NSObject, UNUserNotificationCenterDelegate {
  let app: ApplePushApp?
  let state: ApplePushState
  let configured: Bool
  private let system: IOSApplePushSystem
  private let trace: @MainActor (ApplePushTrace.Receipt) -> Void
  var registrationFailed: ((String, Int) -> Void)?
  var fetchBackground: ((ApplePushNotice, @escaping (ApplePushFetchResult) -> Void) -> Void)?
  private(set) var receivingBackground = false

  convenience init(bundle: Bundle = .main) {
    // There is no supported SecTask entitlement reader in the iOS SDK.
    // Packaging must copy the actual signed/profile environment here and
    // verify equality after archive export; never infer it from DEBUG or
    // an App Store receipt (TestFlight receipts are not an APNs selector).
    let app = ApplePushApp(platform: "ios", bundleID: bundle.bundleIdentifier,
      entitlement: bundle.object(forInfoDictionaryKey: "PPVPNAPNsEntitlementEnvironment") as? String)
    self.init(app: app, enabled: bundle.object(forInfoDictionaryKey: "PPVPNApplePushEnabled") as? Bool == true, system: .live)
  }

  init(app: ApplePushApp?, enabled: Bool, system: IOSApplePushSystem,
    trace: @escaping @MainActor (ApplePushTrace.Receipt) -> Void = ApplePushTrace.emit) {
    self.app = app
    configured = enabled && app != nil
    state = ApplePushState(app: configured ? app : nil)
    self.trace = trace
    self.system = system
    super.init()
  }

  @discardableResult
  func start() -> Bool {
    guard configured else { return false }
    system.installDelegate(self)
    // Ask the OS each launch/retry. Do not restore a token from a file.
    record(.registerRequested)
    system.register()
    return true
  }

  func requestPermission() {
    guard configured else { return }
    system.permission()
  }

  func record(_ stage: ApplePushTrace.Stage, accepted: Bool = false, current: Bool = true,
    opened: Bool = false, remote: Bool = false, error: NSError? = nil) {
    guard configured else { return }
    trace(ApplePushTrace.Receipt(stage, accepted: accepted, current: current,
      opened: opened, remote: remote, error: error))
  }

  func didRegister(token: Data) {
    guard configured else { return }
    record(.tokenReceived, accepted: !token.isEmpty)
    state.receiveToken(token)
  }

  func didFailToRegister(_ error: Error) {
    guard configured else { return }
    let failure = error as NSError
    record(.registrationFailed, error: failure)
    // No token, notification body, localized error text or bearer is logged.
    registrationFailed?(failure.domain, failure.code)
  }

  // Forward from didReceiveRemoteNotification:fetchCompletionHandler:.
  // A native/Dart authenticated fetch hook must complete; a deadline is scheduled
  // at 25s, below Apple's 30s budget (not a real-time scheduling guarantee). Missing auth/handler returns noData.
  func receiveBackground(_ userInfo: [AnyHashable: Any], completion: @escaping (UIBackgroundFetchResult) -> Void) {
    guard configured else { completion(.noData); return }
    record(.backgroundReceived, remote: true)
    guard let notice = ApplePushNotice(userInfo: userInfo, fallbackID: UUID().uuidString) else { completion(.noData); return }
    receivingBackground = true
    let accepted = state.receive(notice, opened: false)
    receivingBackground = false
    guard accepted, let fetchBackground else { completion(.noData); return }
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
    let remote = notification.request.trigger is UNPushNotificationTrigger
    let notice = ApplePushNotice(userInfo: notification.request.content.userInfo, fallbackID: notification.request.identifier)
    DispatchQueue.main.async { [weak self] in
      guard let self, self.configured else { completionHandler([]); return }
      self.record(.foregroundReceived, remote: remote)
      guard let notice else { completionHandler([]); return }
      let accepted = self.state.receive(notice, opened: false)
      // Unscoped legacy data is only an authenticated refresh hint. Its
      // payload body is not shown by the foreground app.
      completionHandler(accepted && notice.accountID != nil ? [.banner, .list, .sound] : [])
    }
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void) {
    if response.actionIdentifier == UNNotificationDefaultActionIdentifier {
      let remote = response.notification.request.trigger is UNPushNotificationTrigger
      let notice = ApplePushNotice(userInfo: response.notification.request.content.userInfo,
        fallbackID: response.notification.request.identifier)
      DispatchQueue.main.async { [weak self] in
        guard let self, self.configured else { return }
        self.record(.tapReceived, opened: true, remote: remote)
        guard let notice else { return }
        self.state.receive(notice, opened: true)
      }
    }
    completionHandler()
  }
}
