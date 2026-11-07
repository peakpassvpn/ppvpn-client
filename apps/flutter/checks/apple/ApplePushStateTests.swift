import Foundation

@main
struct ApplePushTests {
  @MainActor
  static func main() {
    var count = 0
    func check(_ ok: @autoclosure () -> Bool, _ message: String) {
      if !ok() { fatalError(message) }
    }
    let app = ApplePushApp(platform: "ios", bundleID: "com.example.client", entitlement: "production")!
    func notice(_ delivery: String, _ owner: String? = "A", _ message: Any = "42") -> ApplePushNotice {
      var extra: [String: Any] = ["delivery_id": delivery, "message_id": message]
      if let owner { extra["account_id"] = owner }
      return ApplePushNotice(userInfo: ["extra": extra, "body": "untrusted", "deep_link": "https://untrusted.invalid"],
                             fallbackID: "fallback")!
    }
    check(ApplePushEnvironment(entitlement: "development") == .sandbox, "sandbox mapping")
    check(ApplePushEnvironment(entitlement: "production") == .production, "production mapping")
    for raw in [nil, "", "DEBUG", "sandbox", "TestFlight"] {
      check(ApplePushEnvironment(entitlement: raw) == nil, "unknown environment must not default to production")
    }
    check(ApplePushApp(platform: "ios", bundleID: nil, entitlement: "production") == nil, "missing topic")
    check(ApplePushApp(platform: "other", bundleID: "x", entitlement: "production") == nil, "unknown platform")
    count += 1

    do {
      let state = ApplePushState(app: app)
      var requests: [ApplePushRegistration] = []
      var removals: [ApplePushRegistration] = []
      state.attach(register: { requests.append($0) }, unregister: { removals.append($0) }, deliver: { _, _ in })
      state.setAccount("A")
      state.receiveToken(Data())
      check(requests.isEmpty, "empty token")
      state.receiveToken(Data([0, 255]))
      check(requests.count == 1 && requests[0].token == "00ff", "variable-length token")
      state.receiveToken(Data([0, 255]))
      check(requests.count == 1, "duplicate OS callback")
      let first = requests[0]
      check(state.acknowledge(first, success: true), "current ack")
      state.receiveToken(Data([3]))
      check(removals.isEmpty && requests.count == 2, "rotation retains binding and replaces token")
      check(!state.acknowledge(first, success: true), "stale rotation ack")
      check(state.acknowledge(requests[1], success: true), "new token ack")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var requests: [ApplePushRegistration] = []
      var removals: [ApplePushRegistration] = []
      var deliveries: [String] = []
      state.attach(register: { requests.append($0) }, unregister: { removals.append($0) },
                   deliver: { value, _ in deliveries.append(value.deliveryID) })
      state.setAccount("A")
      state.receiveToken(Data([1]))
      let old = requests[0]
      state.setAccount("B")
      check(requests.count == 2 && requests[1].accountID == "B", "account switch rebinds")
      check(removals == [old], "in-flight old registration must be retired")
      check(!state.acknowledge(old, success: true), "old account response")
      check(!state.receive(notice("old", "A"), opened: true), "old-account payload")
      check(state.receive(notice("current", "B"), opened: true), "current-account payload")
      check(deliveries == ["current"], "only current owner can navigate")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var requests: [ApplePushRegistration] = []
      state.attach(register: { requests.append($0) }, unregister: { _ in }, deliver: { _, _ in })
      state.setAccount("A")
      state.receiveToken(Data([1]))
      let old = requests[0]
      state.setAccount(nil)
      check(!state.acknowledge(old, success: true), "logout rejects late ack")
      check(!state.receive(notice("signed-out"), opened: true), "logout drops taps")
      state.receiveToken(Data([2]))
      check(requests.count == 1, "signed-out token must not bind a user")
      state.setAccount("B")
      check(requests.count == 2 && requests[1].token == "02", "fresh token used after login")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var deliveries: [String] = []
      state.receive(notice("cold"), opened: true)
      state.receive(notice("foreign", "B"), opened: true)
      state.setAccount("A")
      state.attach(register: { _ in }, unregister: { _ in }, deliver: { value, _ in deliveries.append(value.deliveryID) })
      check(deliveries == ["cold"], "cold start waits for owner and bridge")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var deliveries: [Bool] = []
      state.attach(register: { _ in }, unregister: { _ in }, deliver: { _, opened in deliveries.append(opened) })
      state.setAccount("A")
      check(state.receive(notice("same"), opened: false), "first foreground receipt")
      check(!state.receive(notice("same"), opened: false), "duplicate receipt")
      check(state.receive(notice("same"), opened: true), "receipt must not consume tap")
      check(!state.receive(notice("same"), opened: true), "duplicate tap")
      check(deliveries == [false, true], "independent histories")
      check(!state.receive(notice("owner-order", "B"), opened: true), "reject foreign owner first")
      check(state.receive(notice("owner-order", "A"), opened: true), "rejection must not consume current delivery")
      count += 1
    }
    do {
      for malformed: Any in [true, NSNumber(value: false), 3.5, "-1", "1e6", " 42 ", "0", String(UInt64.max)] {
        check(notice("parse", "A", malformed).messageID == nil, "malformed message id")
      }
      check(notice("parse", "A", NSNumber(value: 42)).messageID == 42, "numeric id")
      check(notice("parse", "A", "42").messageID == 42, "string id")
      check(ApplePushNotice(userInfo: ["account_id": 42], fallbackID: "x") == nil, "invalid owner must not become unscoped")
      check(ApplePushNotice(userInfo: [:], fallbackID: "") == nil, "empty delivery id")
      count += 1
    }
    do {
      var outcomes: [ApplePushFetchResult] = []
      let completed = ApplePushCompletion { outcomes.append($0) }
      check(completed.finish(.newData), "first fetch result")
      check(!completed.finish(.failed) && !completed.finish(.noData), "late/duplicate completion")
      let timedOut = ApplePushCompletion { outcomes.append($0) }
      check(timedOut.finish(.failed), "deadline result")
      check(!timedOut.finish(.newData), "late network answer after deadline")
      check(outcomes == [.newData, .failed], "one completion per task")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var requests: [ApplePushRegistration] = []
      state.attach(register: { requests.append($0) }, unregister: { _ in }, deliver: { _, _ in })
      state.setAccount("A")
      state.receiveToken(Data([1]))
      check(state.acknowledge(requests[0], success: false), "failed registration recorded")
      state.requestRegistration()
      check(requests.count == 2 && requests[0].request != requests[1].request, "retry uses new request identity")
      check(!state.acknowledge(requests[0], success: true), "late success of failed attempt")
      check(state.acknowledge(requests[1], success: true), "retry success")
      count += 1
    }
    do {
      let state = ApplePushState(app: app, capacity: 2)
      var deliveries: [String] = []
      for id in ["old", "recent", "newest"] { state.receive(notice(id), opened: true) }
      state.attach(register: { _ in }, unregister: { _ in }, deliver: { value, _ in deliveries.append(value.deliveryID) })
      state.setAccount("A")
      check(deliveries == ["recent", "newest"], "bounded cold-start mailbox")
      check(state.receive(notice("old"), opened: true), "dedupe is intentionally bounded, not exactly-once storage")
      count += 1
    }
    do {
      let state = ApplePushState(app: nil)
      var calls = 0
      state.attach(register: { _ in calls += 1 }, unregister: { _ in }, deliver: { _, _ in })
      state.setAccount("A")
      state.receiveToken(Data([1]))
      check(calls == 0, "missing validated environment disables registration")
      count += 1
    }
    do {
      let state = ApplePushState(app: app)
      var deliveries = 0
      state.attach(register: { _ in }, unregister: { _ in }, deliver: { _, _ in deliveries += 1 })
      state.setAccount("A")
      state.beginRestoration()
      check(!state.receive(notice("old-during-restore"), opened: true), "restoration blocks previous owner")
      state.setAccount("B")
      check(deliveries == 0, "buffered foreign owner cannot follow replacement auth")
      count += 1
    }
    print("\(count) Apple push state cases passed")
  }
}
