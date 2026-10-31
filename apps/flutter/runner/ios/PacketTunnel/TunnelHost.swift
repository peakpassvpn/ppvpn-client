// The tunnel host's C functions (crates/tunnel-host ppvpn_tunnel.h, #448)
// for PacketTunnelProvider: one host per process, any thread. Errors come
// back as JSON {"code", "message", "retryable"}, strings the host returns
// are freed with ppvpn_tunnel_free_string.
//
// A build without the PPVPNTunnel XCFramework (scripts/ci-build.sh links
// it when it is there) gets a host that refuses to start, so that the
// extension, its signing and its embedding are built and checked before
// the host exists.

import Foundation

#if canImport(PPVPNTunnel)
  import PPVPNTunnel
#endif

final class TunnelHost {
  /// The host stopped by itself; the reason's message.
  typealias Exit = (String) -> Void

  private let exit: Exit

  private init(exit: @escaping Exit) {
    self.exit = exit
  }

  #if canImport(PPVPNTunnel)
    static func tunSettings() throws -> TunSettings {
      guard let json = take(ppvpn_tunnel_settings(PPVPN_TUNNEL_PLATFORM_IOS)) else {
        throw TunnelError("the tunnel host has no settings for iOS")
      }
      return try JSONDecoder().decode(TunSettings.self, from: Data(json.utf8))
    }

    /// Returns once the host serves `socketPath`; the app then applies the
    /// profile and starts the engine over it.
    static func start(fd: Int32, socketPath: String, stateDir: String, exit: @escaping Exit) throws -> TunnelHost {
      let host = TunnelHost(exit: exit)
      // Held until stop returns: the host calls back with it until then.
      let context = Unmanaged.passRetained(host).toOpaque()
      let onExit: ppvpn_tunnel_exit_fn = { context, reason in
        guard let context else { return }
        let host = Unmanaged<TunnelHost>.fromOpaque(context).takeUnretainedValue()
        host.exit(TunnelHost.message(reason.map { String(cString: $0) }))
      }
      if let error = take(
        ppvpn_tunnel_start(fd, socketPath, stateDir, PPVPN_TUNNEL_PLATFORM_IOS, nil, onExit, context))
      {
        Unmanaged<TunnelHost>.fromOpaque(context).release()
        throw TunnelError(message(error))
      }
      host.context = context
      return host
    }

    /// The error's message, nil when the host took it.
    static func networkChanged(_ state: String) -> String? {
      take(ppvpn_tunnel_network_changed(state)).map(message)
    }

    private var context: UnsafeMutableRawPointer?

    /// Blocks until the engine and the socket are gone.
    func stop() {
      ppvpn_tunnel_stop()
      if let context {
        self.context = nil
        Unmanaged<TunnelHost>.fromOpaque(context).release()
      }
    }

    func sleep() { ppvpn_tunnel_sleep() }
    func wake() { ppvpn_tunnel_wake() }

    private static func take(_ pointer: UnsafeMutablePointer<CChar>?) -> String? {
      guard let pointer else { return nil }
      defer { ppvpn_tunnel_free_string(pointer) }
      return String(cString: pointer)
    }
  #else
    static func tunSettings() throws -> TunSettings {
      throw TunnelError("the tunnel host is not linked into this build")
    }

    static func start(fd: Int32, socketPath: String, stateDir: String, exit: @escaping Exit) throws -> TunnelHost {
      throw TunnelError("the tunnel host is not linked into this build")
    }

    static func networkChanged(_ state: String) -> String? { nil }

    func stop() {}
    func sleep() {}
    func wake() {}
  #endif

  /// "CODE: message" from the host's error or exit JSON.
  static func message(_ json: String?) -> String {
    guard let json,
      let object = try? JSONSerialization.jsonObject(with: Data(json.utf8)) as? [String: Any]
    else {
      return json ?? "unknown error"
    }
    let code = object["code"] as? String ?? "?"
    let message = object["message"] as? String ?? ""
    return message.isEmpty ? code : "\(code): \(message)"
  }
}
