import Foundation

// UIKit boundary only, for executing the actual iOS receiver/bridge method
// bodies on a Mac. SDK compilation separately uses UIKit/Flutter unchanged.
enum UIBackgroundFetchResult { case newData, noData, failed }
@MainActor final class UIApplication {
  enum State { case active, inactive, background }
  var applicationState = State.active
  static let shared = UIApplication()
  func registerForRemoteNotifications() { preconditionFailure("unexpected live UIKit adapter") }
}

@main
struct IOSPushTraceTests {
  @MainActor static func main() async {
    var cases = 0
    func require(_ value: @autoclosure () -> Bool, _ message: String) {
      if !value() { FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2) }
    }
    let app = ApplePushApp(platform: "ios", bundleID: "test.fixture", entitlement: "production")!
    for validApp in [Optional(app), nil] {
      var effects = 0
      var receipts: [ApplePushTrace.Receipt] = []
      let receiver = IOSApplePushReceiver(app: validApp, enabled: validApp == nil,
        system: IOSApplePushSystem(installDelegate: { _ in effects += 1 },
          register: { effects += 1 }, permission: { _, _ in effects += 1 }, diagnostics: { _, _, _ in effects += 1 }), trace: { receipts.append($0) })
      let bridge = IOSApplePushBridge(receiver: receiver)
      bridge.start(); receiver.requestPermission(); bridge.attach(FlutterPluginRegistrar())
      let channel = FlutterMethodChannel.latest!
      receiver.record(.registerRequested)
      receiver.didRegister(token: Data([1]))
      receiver.didFailToRegister(NSError(domain: "SECRET_DOMAIN", code: 42))
      receiver.receiveBackground(["body": "SECRET_BODY"]) { require($0 == .noData, "OFF fetch") }
      channel.call("retry"); channel.call("permission")
      channel.call("diagnostics") { require(($0 as? [String: Any])?["configured"] as? Bool == false, "OFF diagnostics") }
      channel.call("installation", ["platform": "ios", "bundleID": "test.fixture", "environment": "production"])
      channel.call("account", ["state": "signedIn", "id": "SECRET_OWNER", "clientEpoch": "1"])
      require(effects == 0 && receipts.isEmpty && channel.sent.isEmpty && ApplePushInstallation.accesses == 0,
        "iOS OFF system/trace/store effects")
      cases += 1
    }

    var receipts: [ApplePushTrace.Receipt] = []
    var registers = 0
    var delegates = 0
    var permissions = 0
    var deferPermission = false
    var deferredPermission: ((@MainActor () -> Bool), (@MainActor (Bool, Bool, NSError?) -> Void))?
    var diagnosticReads = 0
    var diagnosticRows: [IOSApplePushDiagnostics] = []
    var fixtureAuthorization = IOSApplePushDiagnostics.Authorization.authorized
    var fixtureDelegate = IOSApplePushDiagnostics.Ownership.receiver
    let receiver = IOSApplePushReceiver(app: app, enabled: true,
      system: IOSApplePushSystem(installDelegate: { _ in delegates += 1 },
        register: { registers += 1 }, permission: { allowed, done in
          if deferPermission { deferredPermission = (allowed, done); return }
          permissions += 1; done(true, false, NSError(domain: "SECRET_DOMAIN", code: 7))
        },
        diagnostics: { _, phase, done in
          diagnosticReads += 1
          done(IOSApplePushDiagnostics(phase: phase, authorization: fixtureAuthorization, delegate: fixtureDelegate,
            alert: .enabled, style: .banner, center: .enabled, lock: .disabled, sound: .enabled, badge: .disabled))
        }), trace: { receipts.append($0) }, diagnosticTrace: { diagnosticRows.append($0) })
    let bridge = IOSApplePushBridge(receiver: receiver)
    bridge.start()
    require(registers == 1 && delegates == 1 && permissions == 0 && receipts.last?.stage == .registerRequested,
      "iOS OS request receipt/effects")
    receiver.didRegister(token: Data())
    require(receipts.last?.accepted == false, "empty token must not become successful receipt")
    receiver.didRegister(token: Data([0, 255]))
    require(receipts.last?.stage == .tokenReceived && receipts.last?.accepted == true, "fresh callback receipt")
    receiver.didFailToRegister(NSError(domain: "SECRET_DOMAIN", code: 42,
      userInfo: [NSLocalizedDescriptionKey: "SECRET_BODY SECRET_TOKEN https://SECRET_URL"]))
    require(receipts.last?.failureDomain == .other && receipts.last?.code == 42, "failure domain not sanitized")
    cases += 1
    bridge.attach(FlutterPluginRegistrar())
    let channel = FlutterMethodChannel.latest!
    channel.call("account", ["state": "signedIn", "id": "SECRET_OWNER", "clientEpoch": "1"])
    channel.reply(0, true)
    require(receipts.last?.stage == .bridgeRegisterReply && receipts.last?.accepted == true, "register ack trace")
    cases += 1

    let payload: [AnyHashable: Any] = ["extra": ["delivery_id": "SECRET_DELIVERY", "account_id": "SECRET_OWNER", "message_id": "42"],
      "body": "SECRET_BODY", "url": "https://SECRET_URL"]
    var fetch: UIBackgroundFetchResult?
    receiver.receiveBackground(payload) { fetch = $0 }
    require(fetch == nil && receipts.last?.stage == .backgroundReceived && receipts.last?.remote == true, "background callback trace")
    channel.reply(channel.sent.count - 1, true)
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in DispatchQueue.main.async { done.resume() } }
    require(fetch == .newData && receipts.last?.stage == .bridgeNoticeReply && receipts.last?.accepted == true,
      "current iOS ack missing")
    cases += 1

    var staleFetch: UIBackgroundFetchResult?
    var nextPayload = payload
    nextPayload["extra"] = ["delivery_id": "SECRET_DELIVERY_2", "account_id": "SECRET_OWNER", "message_id": "43"]
    receiver.receiveBackground(nextPayload) { staleFetch = $0 }
    let late = channel.sent.count - 1
    channel.call("account", ["state": "signedOut", "clientEpoch": "2"])
    channel.reply(late, true)
    require(receipts.last?.stage == .bridgeNoticeReply && receipts.last?.accepted == false && receipts.last?.current == false,
      "iOS stale epoch ack was accepted")
    await withCheckedContinuation { (done: CheckedContinuation<Void, Never>) in DispatchQueue.main.async { done.resume() } }
    require(staleFetch == .noData, "stale background reply must not report newData")
    cases += 1

    let requestCount = registers
    channel.call("diagnostics") { reply in
      let value = reply as? [String: Any]
      require(value?["authorization"] as? String == "authorized" && value?["delegate"] as? String == "receiver", "read-only settings getter missing")
      require(Set(value?.keys.map { $0 } ?? []) == ["configured", "phase", "authorization", "delegate", "alert", "style", "center", "lock", "sound", "badge"], "unsafe diagnostic keys")
    }
    require(diagnosticReads == 1 && permissions == 0 && registers == requestCount && ApplePushInstallation.accesses == 0,
      "read-only diagnostics caused permission/register/store effects")
    fixtureAuthorization = .denied; fixtureDelegate = .other
    channel.call("diagnostics") { reply in
      let value = reply as? [String: Any]
      require(value?["authorization"] as? String == "denied" && value?["delegate"] as? String == "other", "denied/other owner state not exposed")
    }
    require(diagnosticRows.last?.line == "apns_settings phase=explicit authorization=denied delegate=other alert=enabled style=banner center=enabled lock=disabled sound=enabled badge=disabled",
      "diagnostic privacy/schema boundary")
    cases += 1
    channel.call("permission", ["clientEpoch": "2"])
    require(permissions == 1 && diagnosticRows.last?.phase == .afterPermission, "permission result lacks actual settings readback")
    require(receipts.last?.stage == .permissionResult && receipts.last?.accepted == false && receipts.last?.opened == true && receipts.last?.failureDomain == .other && receipts.last?.code == 7,
      "permission result/error lost or leaked")
    require(!diagnosticRows.map { $0.line }.joined().contains("SECRET"), "diagnostic contents leaked")
    cases += 1

    deferPermission = true
    for boundary in ["owner", "background"] {
      let before = permissions
      let epoch = boundary == "owner" ? "2" : "3"
      channel.call("permission", ["clientEpoch": epoch]) { require($0 as? Bool == false, "stale permission intent acknowledged") }
      require(deferredPermission != nil, "native permission boundary did not suspend")
      if boundary == "owner" { channel.call("account", ["state": "signedIn", "id": "SECOND_OWNER", "clientEpoch": "3"]) }
      else { UIApplication.shared.applicationState = .background }
      let pending = deferredPermission!
      require(!pending.0(), "stale/background intent allowed OS prompt")
      if pending.0() { permissions += 1 }
      pending.1(false, false, nil)
      require(permissions == before, "stale native intent requested permission")
      deferredPermission = nil
      UIApplication.shared.applicationState = .active
      cases += 1
    }

    for stage in ApplePushTrace.Stage.allCases {
      let receipt = ApplePushTrace.Receipt(stage)
      require(receipt.line == "apns stage=\(stage.rawValue) accepted=0 current=1 opened=0 remote=0 domain=none code=0",
        "trace schema/privacy boundary")
    }
    for (domain, expected) in [(NSCocoaErrorDomain, ApplePushTrace.FailureDomain.cocoa),
      (NSURLErrorDomain, .url), (NSOSStatusErrorDomain, .osStatus), ("SECRET_DOMAIN", .other)] {
      let receipt = ApplePushTrace.Receipt(.registrationFailed,
        error: NSError(domain: domain, code: -42, userInfo: [NSLocalizedDescriptionKey: "SECRET_TOKEN"]))
      require(receipt.failureDomain == expected && !receipt.line.contains("SECRET"), "trace privacy leaked error contents")
    }
    require(!receipts.map { $0.line }.joined().contains("SECRET"), "receiver/bridge leaked sensitive inputs")
    cases += 1
    print("\(cases) iOS production method-body/typed trace cases passed (controlled UIKit/Flutter/system edges)")
  }
}
