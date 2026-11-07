// Controlled boundaries for the unmodified production provider body.
// No app, OS settings, real TUN, sockets, or native engine are started.
import Foundation
import Dispatch
#if os(Linux)
import Glibc
#else
import Darwin
#endif

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
  func containerURL(forSecurityApplicationGroupIdentifier: String) -> URL? { URL(fileURLWithPath: "/app-group") }
  func createDirectory(at: URL, withIntermediateDirectories: Bool, attributes: [FileAttributeKey: Any]? = nil) throws {}
}
struct NEProviderStopReason { let rawValue = 1 }
class NEPacketTunnelNetworkSettings {}
class PacketFlow {
  var reads: [([Data], [NSNumber]) -> Void] = []
  var writes: [([Data], [NSNumber])] = []
  var writeEntered: DispatchSemaphore?
  var writeRelease: DispatchSemaphore?
  var allowWrite = true
  func readPackets(completionHandler: @escaping ([Data], [NSNumber]) -> Void) { reads.append(completionHandler) }
  func writePackets(_ packets: [Data], withProtocols protocols: [NSNumber]) -> Bool {
    writeEntered?.signal()
    if let release = writeRelease { wait(release) }
    writes.append((packets, protocols))
    return allowWrite
  }
}
class NEPacketTunnelProvider {
  let packetFlow = PacketFlow()
  private let lock = NSLock()
  private var settings: [(Error?) -> Void] = []
  var cancellations = 0
  let settingsReady = DispatchSemaphore(value: 0)
  let cancelled = DispatchSemaphore(value: 0)
  func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {}
  func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {}
  func sleep(completionHandler: @escaping () -> Void) {}
  func wake() {}
  func setTunnelNetworkSettings(_ settings: NEPacketTunnelNetworkSettings?, completionHandler: @escaping (Error?) -> Void) {
    guard settings != nil else { return completionHandler(nil) }
    lock.lock(); self.settings.append(completionHandler); lock.unlock()
    settingsReady.signal()
  }
  func nextSettings() -> (Error?) -> Void {
    wait(settingsReady)
    lock.lock(); defer { lock.unlock() }; return settings.removeFirst()
  }
  func cancelTunnelWithError(_ error: Error?) { cancellations += 1; cancelled.signal() }
}
struct TunSettings: Decodable {
  func network() throws -> NEPacketTunnelNetworkSettings { NEPacketTunnelNetworkSettings() }
}
struct Path {}
struct NetworkState { init(_ path: Path) {}; var json: String { "{}" } }
class NWPathMonitor {
  static var instances: [NWPathMonitor] = []
  static var ready = DispatchSemaphore(value: 0)
  var pathUpdateHandler: ((Path) -> Void)?
  private var queue: DispatchQueue!
  func start(queue: DispatchQueue) { self.queue = queue; Self.instances.append(self); Self.ready.signal() }
  func cancel() {}
  func delayed() -> () -> Void {
    let handler = pathUpdateHandler!; let queue = queue!
    return { let done = DispatchSemaphore(value: 0); queue.async { handler(Path()); done.signal() }; wait(done) }
  }
}
class TunnelHost {
  static var starts = 0
  static var stops = 0
  static var networks = 0
  static var exits: [(String) -> Void] = []
  static var configurations: [(UInt64, String?) -> Bool] = []
  static var sends: [(UInt64, Data)] = []
  static var output: [(UInt64, Data)] = []
  static var stopped = DispatchSemaphore(value: 0)
  static var failStart = false
  static var sleeps = 0
  static var wakes = 0
  static func tunSettings() throws -> TunSettings { TunSettings() }
  static func networkChanged(_ json: String) -> String? { networks += 1; return nil }
  static func start(socketPath: String, stateDir: String, settings: @escaping (UInt64, String?) -> Bool,
                    exit: @escaping (String) -> Void) throws -> TunnelHost {
    if failStart { throw TunnelError("start failed") }
    starts += 1; exits.append(exit); configurations.append(settings); return TunnelHost()
  }
  static func send(_ packet: Data, generation: UInt64) { sends.append((generation, packet)) }
  static func receive(generation: UInt64, buffer: inout [UInt8]) -> Int {
    guard let i = output.firstIndex(where: { $0.0 == generation }) else { return 0 }
    let bytes = output.remove(at: i).1
    for (i, byte) in bytes.enumerated() { buffer[i] = byte }
    return bytes.count
  }
  func stop() { Self.stops += 1; Self.stopped.signal() }
  func sleep() { Self.sleeps += 1 }
  func wake() { Self.wakes += 1 }
}
func expect(_ value: @autoclosure () -> Bool, _ message: String) { if !value() { FileHandle.standardError.write(Data((message + "\n").utf8)); exit(2) } }
func wait(_ semaphore: DispatchSemaphore) { expect(semaphore.wait(timeout: .now() + 5) == .success, "controlled callback deadline") }
func reset() {
  NWPathMonitor.instances = []; NWPathMonitor.ready = DispatchSemaphore(value: 0)
  TunnelHost.starts = 0; TunnelHost.stops = 0; TunnelHost.networks = 0
  TunnelHost.exits = []; TunnelHost.configurations = []; TunnelHost.sends = []; TunnelHost.output = []
  TunnelHost.stopped = DispatchSemaphore(value: 0); TunnelHost.failStart = false
  TunnelHost.sleeps = 0; TunnelHost.wakes = 0
}
func stop(_ provider: PacketTunnelProvider) {
  let done = DispatchSemaphore(value: 0); provider.stopTunnel(with: NEProviderStopReason()) { done.signal() }; wait(done)
  while TunnelHost.stopped.wait(timeout: .now()) == .success {}
}
func monitor() -> NWPathMonitor { wait(NWPathMonitor.ready); return NWPathMonitor.instances.last! }
