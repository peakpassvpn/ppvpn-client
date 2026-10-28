// The packet tunnel on iOS (#448): the extension com.peakpassvpn.ppvpn.tunnel,
// the process the engine runs in (docs/mobile.md). The app saves its
// configuration and starts it (runner/ios PPVPNPlugin); the app's client then
// speaks Core API v1 to the tunnel host over the Unix socket tunnel.sock in
// the App Group's container. Here: the tunnel as the host's settings say
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

  override func startTunnel(options: [String: NSObject]?, completionHandler: @escaping (Error?) -> Void) {
    guard let container = FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: Self.appGroup)
    else {
      return completionHandler(TunnelError("no App Group container"))
    }
    let network: NEPacketTunnelNetworkSettings
    do {
      network = try TunnelHost.tunSettings().network()
    } catch {
      log.error("tun settings: \(error.localizedDescription, privacy: .public)")
      return completionHandler(error)
    }
    setTunnelNetworkSettings(network) { [weak self] error in
      guard let self else { return completionHandler(TunnelError("the provider is gone")) }
      if let error {
        self.log.error("network settings: \(error.localizedDescription, privacy: .public)")
        return completionHandler(error)
      }
      guard let fd = Self.utunDescriptor() else {
        return completionHandler(TunnelError("no utun descriptor after the network settings"))
      }
      // Every path to the host, the first before it starts: it hands that
      // one to the engine at start (section 11).
      self.watchNetwork { [weak self] first in
        guard let self else { return completionHandler(TunnelError("the provider is gone")) }
        guard first else { return }
        do {
          self.host = try TunnelHost.start(
            fd: fd,
            socketPath: container.appendingPathComponent("tunnel.sock").path,
            stateDir: container.appendingPathComponent("tunnel-state").path
          ) { [weak self] reason in
            // The host gave up by itself: end the tunnel, then the host.
            self?.log.error("host exit: \(reason, privacy: .public)")
            self?.cancelTunnelWithError(TunnelError(reason))
            self?.queue.async { [weak self] in self?.shutDown() }
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

  override func stopTunnel(with reason: NEProviderStopReason, completionHandler: @escaping () -> Void) {
    log.info("stop, reason \(reason.rawValue, privacy: .public)")
    queue.async { [weak self] in
      self?.shutDown()
      completionHandler()
    }
  }

  /// On the queue: no more paths, then the host (which blocks until done).
  private func shutDown() {
    stopWatching()
    host?.stop()
    host = nil
  }

  override func sleep(completionHandler: @escaping () -> Void) {
    host?.sleep()
    completionHandler()
  }

  override func wake() {
    host?.wake()
  }

  /// Each path goes to the host; `updated` is told whether it was the first.
  private func watchNetwork(updated: @escaping (Bool) -> Void) {
    let monitor = NWPathMonitor()
    var first = true
    monitor.pathUpdateHandler = { [weak self] path in
      let state = NetworkState(path).json
      if let error = TunnelHost.networkChanged(state) {
        self?.log.error("network: \(error, privacy: .public)")
      }
      updated(first)
      first = false
    }
    monitor.start(queue: queue)
    self.monitor = monitor
  }

  private func stopWatching() {
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
/// address inside it and the MTU.
struct TunSettings: Decodable {
  let mtu: Int
  let ipv4Address: String
  let ipv4Routes: [String]
  let dnsServers: [String]

  enum CodingKeys: String, CodingKey {
    case mtu
    case ipv4Address = "ipv4_address"
    case ipv4Routes = "ipv4_routes"
    case dnsServers = "dns_servers"
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
    let dns = NEDNSSettings(servers: dnsServers)
    // Every query, not only those for some domains.
    dns.matchDomains = [""]
    settings.dnsSettings = dns
    settings.mtu = NSNumber(value: mtu)
    return settings
  }
}

/// The engine's NetworkState JSON (section 11) from a path: the interface
/// the system would use, its type and addresses, the gateway; `{}` when
/// offline.
struct NetworkState: Encodable {
  let interface: String
  let type: String
  let addresses: [String]
  let gateway: String?
  let expensive: Bool
  let constrained: Bool

  // Network's, not NetworkExtension's older class of the same name.
  init(_ path: Network.NWPath) {
    let used = path.status == .satisfied ? path.availableInterfaces.first : nil
    interface = used?.name ?? ""
    switch used?.type {
    case .wifi: type = "wifi"
    case .cellular: type = "cellular"
    case .wiredEthernet: type = "ethernet"
    default: type = "other"
    }
    addresses = used.map { Self.addresses(of: $0.name) } ?? []
    gateway = path.gateways.lazy.compactMap { endpoint -> String? in
      if case .hostPort(let host, _) = endpoint { return "\(host)" }
      return nil
    }.first
    expensive = path.isExpensive
    constrained = path.isConstrained
  }

  var json: String {
    guard !interface.isEmpty, let data = try? JSONEncoder().encode(self) else { return "{}" }
    return String(decoding: data, as: UTF8.self)
  }

  private static func addresses(of interface: String) -> [String] {
    var list: UnsafeMutablePointer<ifaddrs>?
    guard getifaddrs(&list) == 0, let first = list else { return [] }
    defer { freeifaddrs(list) }
    var found: [String] = []
    for entry in sequence(first: first, next: { $0.pointee.ifa_next }) {
      guard String(cString: entry.pointee.ifa_name) == interface, let address = entry.pointee.ifa_addr else {
        continue
      }
      let family = Int32(address.pointee.sa_family)
      guard family == AF_INET || family == AF_INET6 else { continue }
      var host = [CChar](repeating: 0, count: Int(NI_MAXHOST))
      let length = socklen_t(family == AF_INET ? MemoryLayout<sockaddr_in>.size : MemoryLayout<sockaddr_in6>.size)
      if getnameinfo(address, length, &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 {
        found.append(String(cString: host))
      }
    }
    return found
  }
}
