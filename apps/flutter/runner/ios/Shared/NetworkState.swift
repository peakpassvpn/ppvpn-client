// One network-state contract for the UI engine and packet-tunnel host.
import Foundation
import Network
import Darwin

/// The host's physical interface, addresses and gateway; `{}` offline.
struct NetworkState: Encodable {
  let interface: String
  let type: String
  let addresses: [String]
  let gateway: String?
  let expensive: Bool
  let constrained: Bool

  // Network's, not NetworkExtension's older class of the same name.
  init(_ path: Network.NWPath) {
    let physical = path.availableInterfaces.filter {
      !$0.name.hasPrefix("utun") && [NWInterface.InterfaceType.wifi, .cellular, .wiredEthernet].contains($0.type)
    }
    let used = path.status == .satisfied
      ? (physical.first { path.usesInterfaceType($0.type) } ?? physical.first) : nil
    interface = used?.name ?? ""
    switch used?.type {
    case .wifi: type = "wifi"
    case .cellular: type = "cellular"
    case .wiredEthernet: type = "ethernet"
    default: type = "other"
    }
    addresses = used.map { Self.addresses(of: $0.name) } ?? []
    gateway = path.gateways.lazy.compactMap { endpoint -> String? in
      // Without a link-local address's zone ("%en0"): the engine reads an IP address.
      if case .hostPort(let host, _) = endpoint { return Self.unzoned("\(host)") }
      return nil
    }.first
    expensive = path.isExpensive
    constrained = path.isConstrained
  }

  var json: String {
    guard !interface.isEmpty, let data = try? JSONEncoder().encode(self) else { return "{}" }
    return String(decoding: data, as: UTF8.self)
  }

  /// The interface's IPv4 and IPv6 addresses as CIDR ("192.168.1.20/24",
  /// "2001:db8::5/64"), as the engine's NetworkState reads them: the prefix
  /// from the netmask, a link-local address without its zone. The engine
  /// takes a global IPv6 address here for the network's IPv6 path.
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
      guard getnameinfo(address, length, &host, socklen_t(host.count), nil, 0, NI_NUMERICHOST) == 0 else {
        continue
      }
      let prefix = entry.pointee.ifa_netmask.map { prefixLength($0, family: family) } ?? (family == AF_INET ? 32 : 128)
      found.append("\(unzoned(String(cString: host)))/\(prefix)")
    }
    return found
  }

  /// The set bits of a netmask.
  private static func prefixLength(_ mask: UnsafeMutablePointer<sockaddr>, family: Int32) -> Int {
    if family == AF_INET {
      return mask.withMemoryRebound(to: sockaddr_in.self, capacity: 1) {
        UInt32(bigEndian: $0.pointee.sin_addr.s_addr).nonzeroBitCount
      }
    }
    return mask.withMemoryRebound(to: sockaddr_in6.self, capacity: 1) {
      withUnsafeBytes(of: $0.pointee.sin6_addr) { $0.reduce(0) { $0 + $1.nonzeroBitCount } }
    }
  }

  /// "fe80::1%en0" as "fe80::1".
  private static func unzoned(_ host: String) -> String {
    host.split(separator: "%", maxSplits: 1).first.map(String.init) ?? host
  }
}
