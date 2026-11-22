// Dependencies of the production PacketTunnelProvider lifecycle, not a
// second implementation of it. Every asynchronous boundary is controlled
// by a semaphore; tests never depend on sleeps or scheduler timing.
import Foundation
import Dispatch

enum Privacy { case `public` }
extension String.StringInterpolation {
  mutating func appendInterpolation<T>(_ value: T, privacy: Privacy) { appendLiteral("\(value)") }
}
struct Logger {
  init(subsystem: String, category: String) {}
  func info(_ text: String) {}
  func error(_ text: String) {}
}
struct TunnelError: LocalizedError {
  let errorDescription: String?
  init(_ message: String) { errorDescription = message }
}
struct FileManager {
  static let `default` = FileManager()
  func containerURL(forSecurityApplicationGroupIdentifier: String) -> URL? {
    URL(fileURLWithPath: "/app-group")
  }
}
struct NEProviderStopReason { let rawValue = 1 }
class NEPacketTunnelNetworkSettings {}
class NEPacketTunnelProvider {
  var settings: [(Error?) -> Void] = []
  var cancellations = 0
  let settingsReady = DispatchSemaphore(value: 0)
  func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {}
  func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {}
  func sleep(completionHandler: @escaping () -> Void) {}
  func wake() {}
  func setTunnelNetworkSettings(_ settings: NEPacketTunnelNetworkSettings,
                               completionHandler: @escaping (Error?) -> Void) {
    self.settings.append(completionHandler)
    settingsReady.signal()
  }
  func cancelTunnelWithError(_ error: Error?) { cancellations += 1 }
}
struct TunSettings {
  func network() throws -> NEPacketTunnelNetworkSettings { NEPacketTunnelNetworkSettings() }
}
struct Path {}
struct NetworkState {
  init(_ path: Path) {}
  var json: String { "{}" }
}
class NWPathMonitor {
  static var instances: [NWPathMonitor] = []
  static let ready = DispatchSemaphore(value: 0)
  var pathUpdateHandler: ((Path) -> Void)?
  private var queue: DispatchQueue!
  func start(queue: DispatchQueue) {
    self.queue = queue
    Self.instances.append(self)
    Self.ready.signal()
  }
  func cancel() {}
  // Capture before cancellation, like an update already submitted by NE.
  func delayed() -> () -> Void {
    let handler = pathUpdateHandler!
    let queue = queue!
    return {
      let done = DispatchSemaphore(value: 0)
      queue.async { handler(Path()); done.signal() }
      wait(done)
    }
  }
}
class TunnelHost {
  static var starts = 0
  static var stops = 0
  static var networks = 0
  static var exits: [(String) -> Void] = []
  static var failStart = false
  static func tunSettings() throws -> TunSettings { TunSettings() }
  static func networkChanged(_ json: String) -> String? { networks += 1; return nil }
  static func start(fd: Int32, socketPath: String, stateDir: String,
                    exit: @escaping (String) -> Void) throws -> TunnelHost {
    if failStart { throw TunnelError("start failed") }
    starts += 1
    exits.append(exit)
    return TunnelHost()
  }
  func stop() { Self.stops += 1 }
  func sleep() {}
  func wake() {}
}

func expect(_ value: @autoclosure () -> Bool, _ message: String) {
  if !value() { fatalError(message) }
}
func wait(_ semaphore: DispatchSemaphore) {
  expect(semaphore.wait(timeout: .now() + 5) == .success, "callback timed out")
}
func settle(_ provider: PacketTunnelProvider) {
  let done = DispatchSemaphore(value: 0)
  provider.sleep { done.signal() }
  wait(done)
}
func stop(_ provider: PacketTunnelProvider) {
  let done = DispatchSemaphore(value: 0)
  provider.stopTunnel(with: NEProviderStopReason()) { done.signal() }
  wait(done)
}
func settings(_ provider: PacketTunnelProvider) -> (Error?) -> Void {
  wait(provider.settingsReady)
  return provider.settings.last!
}
func monitor() -> NWPathMonitor {
  wait(NWPathMonitor.ready)
  return NWPathMonitor.instances.last!
}
func reset() {
  NWPathMonitor.instances = []
  TunnelHost.starts = 0
  TunnelHost.stops = 0
  TunnelHost.networks = 0
  TunnelHost.exits = []
  TunnelHost.failStart = false
}
