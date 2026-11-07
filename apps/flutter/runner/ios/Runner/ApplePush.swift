// Explicit host integration is required; this file never starts itself.
// Keep the receiver alive in the AppDelegate/plugin and install at launch.
import UIKit
@preconcurrency import UserNotifications
import os

// Read-only, finite enums. No delegate class name, user data, token, payload,
// URL or other free-form value can enter this snapshot or its public OSLog.
@MainActor
struct IOSApplePushDiagnostics {
  enum Phase: String { case explicit, afterPermission = "after_permission" }
  enum Authorization: String { case notDetermined = "not_determined", denied, authorized, provisional, ephemeral, unknown }
  enum Ownership: String { case receiver, appDelegate = "app_delegate", other, none }
  enum Setting: String { case unsupported, disabled, enabled, unknown }
  enum Style: String { case none, banner, alert, unknown }
  let phase: Phase
  let authorization: Authorization
  let delegate: Ownership
  let alert: Setting
  let style: Style
  let center: Setting
  let lock: Setting
  let sound: Setting
  let badge: Setting

  static func setting(_ value: UNNotificationSetting) -> Setting {
    switch value {
    case .notSupported: return .unsupported
    case .disabled: return .disabled
    case .enabled: return .enabled
    @unknown default: return .unknown
    }
  }

  init(settings: UNNotificationSettings, delegate: Ownership, phase: Phase) {
    self.phase = phase; self.delegate = delegate
    switch settings.authorizationStatus {
    case .notDetermined: authorization = .notDetermined
    case .denied: authorization = .denied
    case .authorized: authorization = .authorized
    case .provisional: authorization = .provisional
    case .ephemeral: authorization = .ephemeral
    @unknown default: authorization = .unknown
    }
    switch settings.alertStyle {
    case .none: style = .none
    case .banner: style = .banner
    case .alert: style = .alert
    @unknown default: style = .unknown
    }
    alert = Self.setting(settings.alertSetting); center = Self.setting(settings.notificationCenterSetting)
    lock = Self.setting(settings.lockScreenSetting); sound = Self.setting(settings.soundSetting)
    badge = Self.setting(settings.badgeSetting)
  }

  // Typed fixture constructor, also ensures no arbitrary strings can be logged.
  init(phase: Phase, authorization: Authorization, delegate: Ownership, alert: Setting,
    style: Style, center: Setting, lock: Setting, sound: Setting, badge: Setting) {
    self.phase = phase; self.authorization = authorization; self.delegate = delegate
    self.alert = alert; self.style = style; self.center = center; self.lock = lock
    self.sound = sound; self.badge = badge
  }

  var arguments: [String: Any] {
    ["configured": true, "phase": phase.rawValue, "authorization": authorization.rawValue,
     "delegate": delegate.rawValue, "alert": alert.rawValue, "style": style.rawValue,
     "center": center.rawValue, "lock": lock.rawValue, "sound": sound.rawValue, "badge": badge.rawValue]
  }
  var line: String {
    "apns_settings phase=\(phase.rawValue) authorization=\(authorization.rawValue) delegate=\(delegate.rawValue) alert=\(alert.rawValue) style=\(style.rawValue) center=\(center.rawValue) lock=\(lock.rawValue) sound=\(sound.rawValue) badge=\(badge.rawValue)"
  }
  private static let logger = Logger(subsystem: "com.peakpassvpn.ppvpn.apple-push", category: "settings")
  static func emit(_ value: Self) { logger.info("\(value.line, privacy: .public)") }
}

@MainActor
struct IOSApplePushSystem {
  var installDelegate: (IOSApplePushReceiver) -> Void
  var register: () -> Void
  var permission: (@escaping @MainActor () -> Bool, @escaping @MainActor (Bool, Bool, NSError?) -> Void) -> Void
  var diagnostics: (IOSApplePushReceiver, IOSApplePushDiagnostics.Phase,
    @escaping @MainActor (IOSApplePushDiagnostics) -> Void) -> Void = { _, _, _ in }

  static var live: Self {
    Self(installDelegate: { UNUserNotificationCenter.current().delegate = $0 },
      register: { UIApplication.shared.registerForRemoteNotifications() }, permission: { isCurrent, done in
        let center = UNUserNotificationCenter.current()
        center.getNotificationSettings { settings in
          DispatchQueue.main.async {
            guard isCurrent(), UIApplication.shared.applicationState == .active else { done(false, false, nil); return }
            guard settings.authorizationStatus == .notDetermined else {
              let allowed = settings.authorizationStatus == .authorized || settings.authorizationStatus == .provisional || settings.authorizationStatus == .ephemeral
              done(false, allowed, nil)
              return
            }
            center.requestAuthorization(options: [.alert, .badge, .sound]) { granted, error in
              DispatchQueue.main.async { done(true, granted, error as NSError?) }
            }
          }
        }
      }, diagnostics: { receiver, phase, done in
        let center = UNUserNotificationCenter.current()
        center.getNotificationSettings { [weak receiver] settings in
          DispatchQueue.main.async {
            let ownership: IOSApplePushDiagnostics.Ownership
            if center.delegate == nil { ownership = .none }
            else if let receiver, center.delegate === receiver { ownership = .receiver }
            else if center.delegate === UIApplication.shared.delegate { ownership = .appDelegate }
            else { ownership = .other }
            done(IOSApplePushDiagnostics(settings: settings, delegate: ownership, phase: phase))
          }
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
  private let diagnosticTrace: @MainActor (IOSApplePushDiagnostics) -> Void
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
    trace: @escaping @MainActor (ApplePushTrace.Receipt) -> Void = ApplePushTrace.emit,
    diagnosticTrace: @escaping @MainActor (IOSApplePushDiagnostics) -> Void = IOSApplePushDiagnostics.emit) {
    self.app = app
    configured = enabled && app != nil
    state = ApplePushState(app: configured ? app : nil)
    self.trace = trace
    self.diagnosticTrace = diagnosticTrace
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

  func requestPermission(whileCurrent: @escaping @MainActor () -> Bool = { true },
    completion: @escaping @MainActor (Bool) -> Void = { _ in }) {
    guard configured, whileCurrent() else { completion(false); return }
    system.permission(whileCurrent) { [weak self] requested, granted, error in
      guard let self else { return }
      self.record(.permissionResult, accepted: granted, opened: requested, error: error)
      self.readDiagnostics(phase: .afterPermission) { _ in }
      completion(whileCurrent())
    }
  }

  func readDiagnostics(phase: IOSApplePushDiagnostics.Phase = .explicit,
    completion: @escaping @MainActor ([String: Any]) -> Void) {
    guard configured else { completion(["configured": false]); return }
    system.diagnostics(self, phase) { [weak self] value in
      guard let self else { return }
      self.diagnosticTrace(value)
      completion(value.arguments)
    }
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
