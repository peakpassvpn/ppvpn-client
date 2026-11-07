// Controlled Flutter messenger/proof-store boundaries. AppKit/UN adapters and
// the production bridge bodies compile unchanged; no Flutter engine is loaded.
import Foundation

typealias FlutterResult = (Any?) -> Void
let FlutterMethodNotImplemented = NSObject()
final class FlutterError: NSObject {
  let code: String
  init(code: String, message: String?, details: Any?) { self.code = code }
}
final class FlutterMethodCall {
  let method: String
  let arguments: Any?
  init(_ method: String, _ arguments: Any?) { self.method = method; self.arguments = arguments }
}
final class FlutterPluginRegistrar {
  let messenger = NSObject()
}
@MainActor
final class FlutterMethodChannel {
  struct Sent {
    let method: String
    let arguments: [String: Any]
    var reply: FlutterResult?
  }
  static var latest: FlutterMethodChannel?
  var handler: ((FlutterMethodCall, @escaping FlutterResult) -> Void)?
  var sent: [Sent] = []
  init(name: String, binaryMessenger: NSObject) { Self.latest = self }
  func setMethodCallHandler(_ handler: @escaping (FlutterMethodCall, @escaping FlutterResult) -> Void) { self.handler = handler }
  func invokeMethod(_ method: String, arguments: Any?, result: @escaping FlutterResult) {
    sent.append(Sent(method: method, arguments: arguments as? [String: Any] ?? [:], reply: result))
  }
  func call(_ method: String, _ args: Any? = nil, result: @escaping FlutterResult = { _ in }) {
    handler?(FlutterMethodCall(method, args), result)
  }
  func reply(_ index: Int, _ value: Any?) {
    let done = sent[index].reply
    sent[index].reply = nil
    done?(value)
  }
}
@MainActor
struct ApplePushInstallation {
  let installationID = "unused"
  let secret = "unused"
  static var accesses = 0
  static func load(app: ApplePushApp) throws -> Self {
    accesses += 1
    throw FixtureFailure.unexpectedStoreAccess
  }
  enum FixtureFailure: Error { case unexpectedStoreAccess }
}
