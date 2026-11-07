import Foundation

@main
struct MacPushHooksTests {
  @MainActor
  static func main() {
    var cases = 0
    func require(_ value: @autoclosure () -> Bool, _ message: String) {
      if !value() { FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2) }
    }
    let app = ApplePushApp(platform: "macos", bundleID: "test.fixture", entitlement: "production")!
    // OFF and missing signed capability never invoke even injected system
    // boundaries; this also proves no Keychain access through the bridge.
    for validApp in [Optional(app), nil] {
      var effects = 0
      var offReceipts: [ApplePushTrace.Receipt] = []
      let receiver = MacApplePushReceiver(app: validApp, enabled: validApp == nil,
        system: MacApplePushSystem(installDelegate: { _ in effects += 1 },
          register: { effects += 1 }, permission: { effects += 1 }), trace: { offReceipts.append($0) })
      let bridge = MacApplePushBridge(receiver: receiver)
      bridge.prepareForLaunch(); bridge.start(); bridge.start()
      bridge.attach(FlutterPluginRegistrar())
      let channel = FlutterMethodChannel.latest!
      channel.call("configured") { require($0 as? Bool == false, "OFF configured") }
      channel.call("retry"); channel.call("permission")
      channel.call("installation", ["platform": "macos", "bundleID": "test.fixture", "environment": "production"]) {
        require(($0 as? FlutterError)?.code == "push-scope", "OFF proof boundary")
      }
      receiver.record(.registerRequested)
      receiver.didFailToRegister(NSError(domain: "secret", code: 9))
      receiver.didRegister(token: Data([1, 2, 3]))
      receiver.receiveBackground(["delivery_id": "push:1", "account_id": "A", "message_id": "42"])
      channel.call("account", ["state": "signedIn", "id": "A", "clientEpoch": "1"])
      require(effects == 0 && offReceipts.isEmpty && channel.sent.isEmpty && ApplePushInstallation.accesses == 0,
        "disabled/missing capability caused system or store side effects")
      cases += 1
    }

    var receipts: [ApplePushTrace.Receipt] = []
    var events: [String] = []
    var permissions = 0
    var registrations = 0
    weak var installed: MacApplePushReceiver?
    let receiver = MacApplePushReceiver(app: app, enabled: true,
      system: MacApplePushSystem(installDelegate: { installed = $0; events.append("delegate") },
        register: { registrations += 1; events.append("register") }, permission: { permissions += 1 }), trace: { receipts.append($0) })
    let bridge = MacApplePushBridge(receiver: receiver)
    bridge.prepareForLaunch()
    require(installed === receiver && events == ["delegate"], "early strong owner/delegate preparation")
    bridge.start(); bridge.start()
    require(registrations == 1 && events.first == "delegate" && permissions == 0, "launch must register once without permission prompt")
    cases += 1

    var failure: (String, Int)?
    receiver.registrationFailed = { failure = ($0, $1) }
    receiver.didRegister(token: Data([0, 255]))
    receiver.didFailToRegister(NSError(domain: "fixture.failure", code: 7,
      userInfo: [NSLocalizedDescriptionKey: "must not be forwarded"]))
    require(failure?.0 == "fixture.failure" && failure?.1 == 7, "failure callback metadata")
    receiver.receiveBackground(["extra": ["delivery_id": "push:1", "account_id": "A", "message_id": "42"],
      "body": "untrusted", "url": "https://untrusted.invalid"])
    bridge.attach(FlutterPluginRegistrar())
    let channel = FlutterMethodChannel.latest!
    var shows = 0
    bridge.showWindow = { shows += 1 }
    require(channel.sent.isEmpty, "cold token/notice must await authenticated restoration")
    channel.call("account", ["state": "signedIn", "id": "A", "clientEpoch": "1"])
    let register = channel.sent.firstIndex { $0.method == "register" }!
    let hint = channel.sent.firstIndex { $0.method == "notice" }!
    require(channel.sent[register].arguments["token"] as? String == "00ff", "fresh OS variable-length token callback")
    require(channel.sent[register].arguments["clientEpoch"] as? String == "1", "bridge epoch missing")
    require(channel.sent[register].arguments["accountID"] as? String == "A", "stable owner missing")
    require(channel.sent[hint].arguments["deliveryID"] as? String == "push:1", "cold native remote callback lost")
    require(Set(channel.sent[hint].arguments.keys).isSubset(of: ["deliveryID", "accountID", "messageID", "opened"]),
      "untrusted payload body/URL crossed bridge")
    channel.reply(register, true); channel.reply(hint, false)
    cases += 1

    func opened(_ id: String) -> Int {
      let notice = ApplePushNotice(userInfo: ["extra": ["delivery_id": id, "account_id": "A", "message_id": "42"]], fallbackID: "unused")!
      require(receiver.state.receive(notice, opened: true), "current owner tap rejected")
      return channel.sent.count - 1
    }
    let firstTap = opened("push:2")
    require(shows == 0, "window activated before authenticated reply")
    channel.reply(firstTap, true)
    require(shows == 1, "accepted tap must activate window")
    cases += 1

    let staleTap = opened("push:3")
    channel.call("account", ["state": "signedOut", "clientEpoch": "2"])
    channel.reply(staleTap, true)
    require(shows == 1, "late old epoch activated replacement account window")
    let remove = channel.sent.firstIndex { $0.method == "unregister" }!
    require(channel.sent[remove].arguments["accountID"] as? String == "A", "logout must close old owner")
    var drained = false
    channel.call("drain") { _ in drained = true }
    require(!drained, "logout barrier skipped old unregister")
    channel.reply(remove, true)
    require(drained, "logout barrier never settled")
    let before = channel.sent.count
    receiver.didRegister(token: Data([3]))
    receiver.receiveBackground(["delivery_id": "push:4", "account_id": "A", "message_id": "42"])
    require(channel.sent.count == before, "signed-out native callback was admitted")
    cases += 1

    channel.call("account", ["state": "signedIn", "id": "B", "clientEpoch": "3"])
    let replacement = channel.sent.last!
    require(replacement.method == "register" && replacement.arguments["accountID"] as? String == "B" &&
      replacement.arguments["token"] as? String == "03" && replacement.arguments["clientEpoch"] as? String == "3", "replacement owner/OS token/epoch mixed")
    channel.reply(channel.sent.count - 1, true)
    channel.call("retry")
    require(registrations == 2 && permissions == 0, "retry must ask OS without implicit permission prompt")
    channel.call("permission")
    require(permissions == 1, "explicit permission hook missing")
    cases += 1

    receiver.didRegister(token: Data([4]))
    let oldTokenReply = channel.sent.count - 1
    receiver.didRegister(token: Data([5]))
    let currentTokenReply = channel.sent.count - 1
    channel.reply(oldTokenReply, true)
    require(receipts.last?.stage == .bridgeRegisterReply && receipts.last?.current == true && receipts.last?.accepted == false,
      "superseded token reply incorrectly logged as admitted")
    channel.reply(currentTokenReply, true)
    require(receipts.last?.accepted == true, "current token reply missing")
    cases += 1
    require(receipts.filter { $0.stage == .registerRequested }.count == 2, "OS register receipt missing")
    require(receipts.contains { $0.stage == .tokenReceived && $0.accepted }, "fresh token receipt missing")
    require(receipts.contains { $0.stage == .registrationFailed && $0.failureDomain == .other && $0.code == 7 }, "safe failure receipt missing")
    require(receipts.contains { $0.stage == .backgroundReceived && $0.remote }, "remote callback receipt missing")
    require(receipts.contains { $0.stage == .bridgeNoticeReply && $0.opened && $0.current && $0.accepted }, "current ack receipt missing")
    require(receipts.contains { $0.stage == .bridgeNoticeReply && $0.opened && !$0.current && !$0.accepted }, "stale ack receipt missing")
    require(!receipts.map { $0.line }.joined().contains("fixture.failure"), "arbitrary failure domain leaked")
    cases += 1
    print("\(cases) production macOS receiver/bridge cases passed (controlled system boundaries; no app launch/APNs)")
  }
}
