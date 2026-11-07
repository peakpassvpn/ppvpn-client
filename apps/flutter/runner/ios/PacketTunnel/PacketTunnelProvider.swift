// The packet tunnel on iOS (#448): the extension com.peakpassvpn.ppvpn.tunnel,
// the process the engine runs in (docs/mobile.md). The app saves its
// configuration and starts it (runner/ios PPVPNPlugin); the app's client then
// speaks Core API v1 to the tunnel host over the Unix socket tunnel.sock in
// the App Group's data directory (Flutter's ClientConfig.data_dir). Here:
// the tunnel as the host's settings say
// (docs/host-integration.md section 11), its utun descriptor handed to the
// host, the network pushed to it (NWPathMonitor), sleep and wake.
//
// The host's C functions (crates/tunnel-host ppvpn_tunnel.h, in the
// PPVPNTunnel XCFramework, linked into this extension only) are behind
// TunnelHost (TunnelHost.swift).

import Darwin
import Network
import NetworkExtension
import os

final class PacketTunnelProvider: NEPacketTunnelProvider {
  static let appGroup = "group.com.peakpassvpn.ppvpn"

  private let log = Logger(subsystem: "com.peakpassvpn.ppvpn.tunnel", category: "tunnel")
  private let queue = DispatchQueue(label: "com.peakpassvpn.ppvpn.tunnel")
  private var monitor: NWPathMonitor?
  private var host: TunnelHost?
  private var lifecycleEpoch: UInt64 = 0
  private var stopping = false
  private var starting = false
  private var settingsUncertain = false
  private var initialSettingsPending = false
  private let stopLock = NSLock()
  // All admission and public writes run on queue. A sync barrier fences even
  // an output packet already dequeued from Rust before detach returns.
  private var packetGeneration: UInt64?
  private var outputTimer: DispatchSourceTimer?
  private var packetBuffer = [UInt8](repeating: 0, count: 65535)

  override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
    queue.async { [self] in
      guard !starting, !stopping, !settingsUncertain, host == nil else {
        return completionHandler(TunnelError("provider lifecycle is busy"))
      }
      lifecycleEpoch &+= 1
      starting = true
      let epoch = lifecycleEpoch
      beginTunnel(epoch: epoch) { [weak self] error in
        guard let self else { return completionHandler(error ?? TunnelError("provider is gone")) }
        self.queue.async {
          guard self.lifecycleEpoch == epoch else {
            return completionHandler(TunnelError("tunnel startup cancelled"))
          }
          self.starting = false
          completionHandler(error)
        }
      }
    }
  }

  private func beginTunnel(epoch: UInt64, completionHandler: @escaping (Error?) -> Void) {
    guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.appGroup)
    else {
      return completionHandler(TunnelError("no App Group container"))
    }
    // The Flutter bridge puts ClientConfig.data_dir under APP_DIR/data;
    // TunnelTransport::in_dir connects to tunnel.sock inside that directory.
    // Create it here too so a system-started extension can start first.
    let dataDir = container.appendingPathComponent("data", isDirectory: true)
    do {
      try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
    } catch {
      return completionHandler(error)
    }
    let network: NEPacketTunnelNetworkSettings
    do {
      network = try TunnelHost.tunSettings().network()
    } catch {
      log.error("tun settings: \(error.localizedDescription, privacy: .public)")
      return completionHandler(error)
    }
    initialSettingsPending = true
    setTunnelNetworkSettings(network) { [weak self] error in
      guard let self else { return completionHandler(TunnelError("the provider is gone")) }
      self.queue.async {
      self.initialSettingsPending = false
      guard self.lifecycleEpoch == epoch, !self.stopping else {
        self.setTunnelNetworkSettings(nil) { _ in }
        return completionHandler(TunnelError("tunnel startup cancelled"))
      }
      if let error {
        self.settingsUncertain = true
        self.log.error("network settings: \(error.localizedDescription, privacy: .public)")
        return completionHandler(error)
      }
      // Every path to the host, the first before it starts: it hands that
      // one to the engine at start (section 11).
      self.watchNetwork { [weak self] first in
        guard let self else { return completionHandler(TunnelError("the provider is gone")) }
        guard first else { return }
        guard self.lifecycleEpoch == epoch, !self.stopping else {
          return completionHandler(TunnelError("tunnel startup cancelled"))
        }
        do {
          self.host = try TunnelHost.start(
            socketPath: dataDir.appendingPathComponent("tunnel.sock").path,
            stateDir: container.appendingPathComponent("tunnel-state").path,
            settings: { [weak self] generation, json in
              self?.configurePackets(generation: generation, json: json) ?? false
            }
          ) { [weak self] reason in
            // The host gave up by itself: end the tunnel, then the host.
            self?.queue.async { [weak self] in
              guard let self, self.lifecycleEpoch == epoch else { return }
              self.log.error("host exit: \(reason, privacy: .public)")
              self.cancelTunnelWithError(TunnelError(reason))
              DispatchQueue.global(qos: .utility).async { [weak self] in self?.shutDown(expected: epoch) }
            }
          }
          self.log.info("tunnel started")
          completionHandler(nil)
        } catch {
          self.log.error("start: \(error.localizedDescription, privacy: .public)")
          self.stopWatching()
          completionHandler(error)
        }
      }
      }
    }
  }

  override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
    log.info("stop, reason \(reason.rawValue, privacy: .public)")
    DispatchQueue.global(qos: .utility).async { [weak self] in
      self?.shutDown()
      completionHandler()
    }
  }

  /// On the queue: no more paths, then the host (which blocks until done).
  private func shutDown(expected: UInt64? = nil) {
    stopLock.lock()
    defer { stopLock.unlock() }
    if let expected, !queue.sync(execute: { lifecycleEpoch == expected }) { return }
    let oldHost: TunnelHost? = queue.sync {
      stopping = true
      starting = false
      // An outstanding initial settings operation cannot be cancelled/joined
      // through a public API. Quarantine this provider instead of reusing it.
      if initialSettingsPending { settingsUncertain = true }
      lifecycleEpoch &+= 1
      stopWatching()
      fencePackets()
      let old = host
      host = nil
      return old
    }
    // Rust detach calls back onto queue; blocking stop must run elsewhere.
    oldHost?.stop()
    let cleared = PacketSettingsCompletion()
    setTunnelNetworkSettings(nil) { error in cleared.finish(error == nil) }
    if !cleared.wait() {
      queue.sync { settingsUncertain = true }
      log.error("stop settings did not settle")
    }
    queue.sync { stopping = false }
  }

  override func sleep(completionHandler: @escaping () -> Void) {
    queue.async { [weak self] in self?.host?.sleep(); completionHandler() }
  }

  override func wake() {
    queue.async { [weak self] in self?.host?.wake() }
  }

  /// Each path goes to the host; `updated` is told whether it was the first.
  private func watchNetwork(updated: @escaping (Bool) -> Void) {
    let monitor = NWPathMonitor()
    let epoch = lifecycleEpoch
    var first = true
    monitor.pathUpdateHandler = { [weak self] path in
      guard let self, self.lifecycleEpoch == epoch, !self.stopping else { return }
      let state = NetworkState(path).json
      if let error = TunnelHost.networkChanged(state) {
        self.log.error("network: \(error, privacy: .public)")
      }
      updated(first)
      first = false
    }
    monitor.start(queue: queue)
    self.monitor = monitor
  }

  private func stopWatching() {
    monitor?.pathUpdateHandler = nil
    monitor?.cancel()
    monitor = nil
  }

  private func fencePackets() {
    packetGeneration = nil
    outputTimer?.cancel()
    outputTimer = nil
  }

  /// Rust calls on its blocking pool, never queue. An uncertain settings
  /// timeout cancels this provider; it never certifies a clean OS rollback.
  private func configurePackets(generation: UInt64, json: String?) -> Bool {
    let allowed = queue.sync { fencePackets(); return json == nil || (!stopping && !settingsUncertain) }
    guard allowed else { return false }
    let settings: NEPacketTunnelNetworkSettings?
    do {
      settings = try json.map { try JSONDecoder().decode(TunSettings.self, from: Data($0.utf8)).network() }
    } catch { return false }
    let completion = PacketSettingsCompletion()
    setTunnelNetworkSettings(settings) { error in completion.finish(error == nil) }
    guard completion.wait() else {
      queue.sync { settingsUncertain = true }
      cancelTunnelWithError(TunnelError("packet settings did not settle"))
      return false
    }
    if json != nil {
      let admitted = queue.sync { () -> Bool in
        guard !stopping, !settingsUncertain else { return false }
        packetGeneration = generation
        readPackets(generation: generation)
        let timer = DispatchSource.makeTimerSource(queue: queue)
        timer.schedule(deadline: .now(), repeating: .milliseconds(5))
        timer.setEventHandler { [weak self] in self?.writePackets(generation: generation) }
        outputTimer = timer
        timer.resume()
        return true
      }
      if !admitted { return false }
    }
    return true
  }

  private func readPackets(generation: UInt64) {
    guard packetGeneration == generation else { return }
    packetFlow.readPackets { [weak self] packets, protocols in
      guard let self else { return }
      self.queue.async { [weak self] in
        guard let self, self.packetGeneration == generation else { return }
        for (packet, family) in zip(packets, protocols) {
          guard let first = packet.first,
            (first >> 4 == 4 && family.int32Value == AF_INET) ||
            (first >> 4 == 6 && family.int32Value == AF_INET6) else { continue }
          TunnelHost.send(packet, generation: generation)
        }
        self.readPackets(generation: generation)
      }
    }
  }

  private func writePackets(generation: UInt64) {
    guard packetGeneration == generation else { return }
    var packets: [Data] = []
    var protocols: [NSNumber] = []
    for _ in 0..<64 {
      let count = TunnelHost.receive(generation: generation, buffer: &packetBuffer)
      guard count > 0 else { break }
      let version = packetBuffer[0] >> 4
      guard version == 4 || version == 6 else { continue }
      packets.append(Data(packetBuffer.prefix(count)))
      protocols.append(NSNumber(value: version == 4 ? AF_INET : AF_INET6))
    }
    guard !packets.isEmpty else { return }
    if !packetFlow.writePackets(packets, withProtocols: protocols) {
      fencePackets()
      cancelTunnelWithError(TunnelError("packet flow refused output"))
    }
  }

}

struct TunnelError: LocalizedError {
  let errorDescription: String?
  init(_ message: String) { errorDescription = message }
}

/// The engine's TunSettings (section 11, ppvpn_tunnel_settings): the
/// tunnel's one IPv4 address and its routes (all traffic) as CIDR, the DNS
/// address inside it and the MTU. IPv6 is optional: the engine sends none
/// yet (#511), so an app's IPv6 leaves on the underlying network until it
/// does; with them the tunnel takes IPv6 too, as Android's TunnelService.
struct TunSettings: Decodable {
  let mtu: Int
  let ipv4Address: String
  let ipv4Routes: [String]
  let dnsServers: [String]
  let ipv6Address: String?
  let ipv6Routes: [String]?

  enum CodingKeys: String, CodingKey {
    case mtu
    case ipv4Address = "ipv4_address"
    case ipv4Routes = "ipv4_routes"
    case dnsServers = "dns_servers"
    case ipv6Address = "ipv6_address"
    case ipv6Routes = "ipv6_routes"
  }

  /// "10.60.159.89/30" as the address and its dotted mask.
  static func split(_ cidr: String) throws -> (String, String) {
    let parts = cidr.split(separator: "/", maxSplits: 1).map(String.init)
    guard parts.count == 2, let bits = Int(parts[1]), (0...32).contains(bits) else {
      throw TunnelError("not an IPv4 CIDR: \(cidr)")
    }
    let mask: UInt32 = bits == 0 ? 0 : UInt32.max << (32 - UInt32(bits))
    let dotted = [24, 16, 8, 0].map { String((mask >> UInt32($0)) & 0xFF) }.joined(separator: ".")
    return (parts[0], dotted)
  }

  /// "fd00::1/128" as the address and its prefix length.
  static func split6(_ cidr: String) throws -> (String, NSNumber) {
    let parts = cidr.split(separator: "/", maxSplits: 1).map(String.init)
    guard parts.count == 2, parts[0].contains(":"), let bits = Int(parts[1]), (0...128).contains(bits) else {
      throw TunnelError("not an IPv6 CIDR: \(cidr)")
    }
    return (parts[0], NSNumber(value: bits))
  }

  func network() throws -> NEPacketTunnelNetworkSettings {
    // The remote address only labels the tunnel in Settings.
    let settings = NEPacketTunnelNetworkSettings(tunnelRemoteAddress: "127.0.0.1")
    let (address, mask) = try Self.split(ipv4Address)
    let ipv4 = NEIPv4Settings(addresses: [address], subnetMasks: [mask])
    ipv4.includedRoutes = try ipv4Routes.map {
      let (destination, mask) = try Self.split($0)
      return NEIPv4Route(destinationAddress: destination, subnetMask: mask)
    }
    settings.ipv4Settings = ipv4
    if let ipv6Address {
      let (address, prefix) = try Self.split6(ipv6Address)
      let ipv6 = NEIPv6Settings(addresses: [address], networkPrefixLengths: [prefix])
      ipv6.includedRoutes = try (ipv6Routes ?? []).map {
        let (destination, prefix) = try Self.split6($0)
        return NEIPv6Route(destinationAddress: destination, networkPrefixLength: prefix)
      }
      settings.ipv6Settings = ipv6
    }
    let dns = NEDNSSettings(servers: dnsServers)
    // Every query, not only those for some domains.
    dns.matchDomains = [""]
    settings.dnsSettings = dns
    settings.mtu = NSNumber(value: mtu)
    return settings
  }
}

/// The OS completion may arrive after our deadline. Such a timeout is dirty
/// and terminates the provider; it must never start a late packet generation.
private final class PacketSettingsCompletion: @unchecked Sendable {
  private let ready = DispatchSemaphore(value: 0)
  private let lock = NSLock()
  private var success = false
  func finish(_ value: Bool) { lock.lock(); success = value; lock.unlock(); ready.signal() }
  func wait() -> Bool {
    guard ready.wait(timeout: .now() + 2) == .success else { return false }
    lock.lock(); defer { lock.unlock() }; return success
  }
}
