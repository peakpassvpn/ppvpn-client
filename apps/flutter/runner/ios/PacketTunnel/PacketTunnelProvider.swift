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
  // Lifecycle state belongs to queue, including asynchronous settings,
  // path and exit callbacks. A token stays distinct even after a restart.
  private var monitor: NWPathMonitor?
  private var host: TunnelHost?
  private var run: Run?

  private final class Run {
    var completion: ((Error?) -> Void)?

    init(_ completion: @escaping (Error?) -> Void) { self.completion = completion }

    func finish(_ error: Error?) {
      let callback = completion
      completion = nil
      callback?(error)
    }
  }

  override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
    queue.async { [weak self] in
      guard let self else { return completionHandler(TunnelError("the provider is gone")) }
      self.shutDown()
      let run = Run(completionHandler)
      self.run = run
      self.start(run)
    }
  }

  /// On queue. Delayed system completions must re-enter it and check run.
  private func start(_ run: Run) {
    guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.appGroup)
    else {
      return shutDown(error: TunnelError("no App Group container"))
    }
    // The Flutter bridge puts ClientConfig.data_dir under APP_DIR/data;
    // TunnelTransport::in_dir connects to tunnel.sock inside that directory.
    // Create it here too so a system-started extension can start first.
    let dataDir = container.appendingPathComponent("data", isDirectory: true)
    do {
      try FileManager.default.createDirectory(at: dataDir, withIntermediateDirectories: true)
    } catch {
      return shutDown(error: error)
    }
    let network: NEPacketTunnelNetworkSettings
    do {
      network = try TunnelHost.tunSettings().network()
    } catch {
      log.error("tun settings: \(error.localizedDescription, privacy: .public)")
      return shutDown(error: error)
    }
    setTunnelNetworkSettings(network) { [weak self] error in
      guard let self else { return run.finish(TunnelError("the provider is gone")) }
      self.queue.async {
        guard self.run === run else { return }
        if let error {
          self.log.error("network settings: \(error.localizedDescription, privacy: .public)")
          return self.shutDown(error: error)
        }
        guard let fd = Self.utunDescriptor() else {
          return self.shutDown(error: TunnelError("no utun descriptor after the network settings"))
        }
        // Every path to the host, the first before it starts.
        self.watchNetwork(run: run) { [weak self] first in
          guard let self, self.run === run, first else { return }
          do {
            self.host = try TunnelHost.start(
              fd: fd,
              socketPath: dataDir.appendingPathComponent("tunnel.sock").path,
              stateDir: container.appendingPathComponent("tunnel-state").path
            ) { [weak self] reason in
              // Return to Rust promptly: stop drains this callback. The
              // queued work may run after stop/start, so check its token.
              self?.queue.async { [weak self] in
                guard let self, self.run === run else { return }
                self.log.error("host exit: \(reason, privacy: .public)")
                self.cancelTunnelWithError(TunnelError(reason))
                self.shutDown()
              }
            }
            self.log.info("tunnel started")
            run.finish(nil)
          } catch {
            self.log.error("start: \(error.localizedDescription, privacy: .public)")
            self.shutDown(error: error)
          }
        }
      }
    }
  }

  override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
    log.info("stop, reason \(reason.rawValue, privacy: .public)")
    queue.async { [weak self] in
      self?.shutDown()
      completionHandler()
    }
  }

  /// On queue: invalidate callbacks before stopping the monitor and host.
  private func shutDown(error: Error = TunnelError("the tunnel stopped before startup finished")) {
    let previous = run
    run = nil
    stopWatching()
    host?.stop()
    host = nil
    previous?.finish(error)
  }

  override func sleep(completionHandler: @escaping () -> Void) {
    queue.async { [weak self] in
      self?.host?.sleep()
      completionHandler()
    }
  }

  override func wake() {
    queue.async { [weak self] in self?.host?.wake() }
  }

  /// Each path goes to this run only; queued updates may outlive cancel.
  private func watchNetwork(run: Run, updated: @escaping (Bool) -> Void) {
    let monitor = NWPathMonitor()
    var first = true
    monitor.pathUpdateHandler = { [weak self] path in
      guard let self, self.run === run else { return }
      let state = NetworkState(path).json
      if let error = TunnelHost.networkChanged(state) {
        self.log.error("network: \(error, privacy: .public)")
      }
      let wasFirst = first
      first = false
      updated(wasFirst)
    }
    self.monitor = monitor
    monitor.start(queue: queue)
  }

  private func stopWatching() {
    monitor?.pathUpdateHandler = nil
    monitor?.cancel()
    monitor = nil
  }

  /// The utun the system made for this tunnel: the one descriptor whose
  /// control socket names a utun interface. NEPacketTunnelFlow has no public
  /// accessor for it.
  static func utunDescriptor() -> Int32? {
    var name = [CChar](repeating: 0, count: Int(IFNAMSIZ))
    for fd: Int32 in 0...1024 {
      var length = socklen_t(name.count)
      // SYSPROTO_CONTROL, UTUN_OPT_IFNAME (sys/kern_control.h, net/if_utun.h).
      if getsockopt(fd, 2, 2, &name, &length) == 0, String(cString: name).hasPrefix("utun") {
        return fd
      }
    }
    return nil
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
