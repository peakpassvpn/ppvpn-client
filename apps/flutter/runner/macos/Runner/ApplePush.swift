// AppDelegate installs this receiver before Flutter initialization. System
// effects stay injectable for controlled tests; the existing Agent is retained.
import Cocoa
import Security
@preconcurrency import UserNotifications

@MainActor
struct MacApplePushSystem {
  let installDelegate: (MacApplePushReceiver) -> Void
  let register: () -> Void
  let permission: (@escaping @MainActor () -> Bool, @escaping @MainActor (Bool) -> Void) -> Void

  static var live: Self {
    Self(installDelegate: { $0.installNativeDelegate() }, register: {
      NSApplication.shared.registerForRemoteNotifications()
    }, permission: { isCurrent, done in
      let center = UNUserNotificationCenter.current()
      center.getNotificationSettings { settings in
        DispatchQueue.main.async {
          guard isCurrent(), NSApp.isActive else { done(false); return }
          guard settings.authorizationStatus == .notDetermined else {
            let allowed = settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional
            ApplePushTrace.emit(ApplePushTrace.Receipt(.permissionResult, accepted: allowed))
            done(true)
            return
          }
          center.requestAuthorization(options: [.alert, .badge, .sound]) { granted, error in
            DispatchQueue.main.async {
              ApplePushTrace.emit(ApplePushTrace.Receipt(.permissionResult, accepted: granted, opened: true, error: error as NSError?))
              done(isCurrent())
            }
          }
        }
      }
    })
  }
}

@MainActor
final class MacApplePushReceiver: NSObject, UNUserNotificationCenterDelegate {
  let app: ApplePushApp?
  let state: ApplePushState
  let configured: Bool
  private let system: MacApplePushSystem
  private weak var previousDelegate: UNUserNotificationCenterDelegate?
  private let trace: @MainActor (ApplePushTrace.Receipt) -> Void
  var registrationFailed: ((String, Int) -> Void)?

  convenience init(bundle: Bundle = .main) {
    var entitlement: String?
    if let task = SecTaskCreateFromSelf(nil) {
      entitlement = SecTaskCopyValueForEntitlement(task,
        "com.apple.developer.aps-environment" as CFString, nil) as? String
    }
    let app = ApplePushApp(platform: "macos", bundleID: bundle.bundleIdentifier, entitlement: entitlement)
    self.init(app: app, enabled: bundle.object(forInfoDictionaryKey: "PPVPNApplePushEnabled") as? Bool == true,
              system: .live)
  }

  init(app: ApplePushApp?, enabled: Bool, system: MacApplePushSystem, trace: @escaping @MainActor (ApplePushTrace.Receipt) -> Void = ApplePushTrace.emit) {
    self.app = app
    configured = enabled && app != nil
    state = ApplePushState(app: configured ? app : nil)
    self.system = system
    self.trace = trace
    super.init()
  }

  // May run before the window/Flutter exists. OFF does not even obtain the
  // notification center. Recheck after plugin registration to retain any local
  // delegate without allowing it to silently displace remote-push handling.
  func prepareForLaunch() {
    guard configured else { return }
    system.installDelegate(self)
  }

  fileprivate func installNativeDelegate() {
    let center = UNUserNotificationCenter.current()
    if center.delegate !== self {
      previousDelegate = center.delegate
      center.delegate = self
    }
  }

  @discardableResult
  func start() -> Bool {
    guard configured else { return false }
    prepareForLaunch()
    record(.registerRequested)
    system.register()
    return true
  }

  func requestPermission(whileCurrent: @escaping @MainActor () -> Bool = { true },
    completion: @escaping @MainActor (Bool) -> Void = { _ in }) {
    guard configured, whileCurrent() else { completion(false); return }
    system.permission(whileCurrent, completion)
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

  // Forward NSApplicationDelegate's didReceiveRemoteNotification for a
  // refresh hint. This does not claim to keep the application resident.
  func receiveBackground(_ userInfo: [AnyHashable: Any]) {
    guard configured else { return }
    record(.backgroundReceived, remote: true)
    guard let notice = ApplePushNotice(userInfo: userInfo, fallbackID: UUID().uuidString) else { return }
    state.receive(notice, opened: false)
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, willPresent notification: UNNotification,
    withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void) {
    DispatchQueue.main.async { [weak self] in
      guard let self else { completionHandler([]); return }
      if !(notification.request.trigger is UNPushNotificationTrigger) {
        if let handler = self.previousDelegate?.userNotificationCenter(_:willPresent:withCompletionHandler:) {
          handler(center, notification, completionHandler)
        } else { completionHandler([]) }
        return
      }
      self.record(.foregroundReceived, remote: true)
      guard self.configured, let notice = ApplePushNotice(userInfo: notification.request.content.userInfo,
        fallbackID: notification.request.identifier) else { completionHandler([]); return }
      let accepted = self.state.receive(notice, opened: false)
      completionHandler(accepted && notice.accountID != nil ? [.banner, .list, .sound] : [])
    }
  }

  nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter, didReceive response: UNNotificationResponse,
    withCompletionHandler completionHandler: @escaping () -> Void) {
    DispatchQueue.main.async { [weak self] in
      guard let self else { completionHandler(); return }
      if !(response.notification.request.trigger is UNPushNotificationTrigger) {
        if let handler = self.previousDelegate?.userNotificationCenter(_:didReceive:withCompletionHandler:) {
          handler(center, response, completionHandler)
        } else { completionHandler() }
        return
      }
      self.record(.tapReceived, opened: response.actionIdentifier == UNNotificationDefaultActionIdentifier, remote: true)
      if self.configured, response.actionIdentifier == UNNotificationDefaultActionIdentifier,
        let notice = ApplePushNotice(userInfo: response.notification.request.content.userInfo,
          fallbackID: response.notification.request.identifier) {
        self.state.receive(notice, opened: true)
      }
      completionHandler()
    }
  }
}
