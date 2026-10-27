// The app's channels on iOS (#448), the counterpart of runner/android's
// MainActivity and VpnPlugin. scripts/ci-build.sh appends this file to the
// generated AppDelegate.swift (a file the Runner target already compiles)
// and registers the plugin next to the generated ones.
//
// - com.peakpassvpn.ppvpn/app: `filesDir`, the app's own directory: the
//   App Group's container, which the packet tunnel shares (tunnel.sock),
//   or Application Support in a build without the App Group (unsigned,
//   the simulator).
// - com.peakpassvpn.ppvpn/vpn: `prepare` -> Bool (the VPN configuration is
//   saved; the system asks the user the first time), `start`, `stop`.
// - com.peakpassvpn.ppvpn/vpn-state: the tunnel's NEVPNStatus as off /
//   connecting / on / disconnecting, the current one first.
//
// The packet tunnel itself (com.peakpassvpn.ppvpn.tunnel) is not in the
// app yet: it comes with the tunnel host's C functions (#448 step 2).

import Flutter
import NetworkExtension
import UIKit

final class PPVPNPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
  static let appGroup = "group.com.peakpassvpn.ppvpn"
  static let tunnelBundleID = "com.peakpassvpn.ppvpn.tunnel"

  private var manager: NETunnelProviderManager?
  private var sink: FlutterEventSink?
  private var observer: NSObjectProtocol?

  static func register(with registrar: FlutterPluginRegistrar) {
    let plugin = PPVPNPlugin()
    let messenger = registrar.messenger()
    FlutterMethodChannel(name: "com.peakpassvpn.ppvpn/app", binaryMessenger: messenger)
      .setMethodCallHandler { call, result in
        switch call.method {
        case "filesDir": result(filesDir())
        default: result(FlutterMethodNotImplemented)
        }
      }
    FlutterMethodChannel(name: "com.peakpassvpn.ppvpn/vpn", binaryMessenger: messenger)
      .setMethodCallHandler(plugin.handle)
    FlutterEventChannel(name: "com.peakpassvpn.ppvpn/vpn-state", binaryMessenger: messenger)
      .setStreamHandler(plugin)
  }

  private static func filesDir() -> String? {
    let files = FileManager.default
    let dir =
      files.containerURL(forSecurityApplicationGroupIdentifier: appGroup)
      ?? files.urls(for: .applicationSupportDirectory, in: .userDomainMask).first
    guard let dir else { return nil }
    try? files.createDirectory(at: dir, withIntermediateDirectories: true)
    return dir.path
  }

  func handle(_ call: FlutterMethodCall, result: @escaping FlutterResult) {
    switch call.method {
    case "prepare":
      prepare { ok in result(ok) }
    case "start":
      guard let manager else {
        return result(FlutterError(code: "not-prepared", message: "prepare the VPN first", details: nil))
      }
      do {
        try manager.connection.startVPNTunnel()
        result(nil)
      } catch {
        result(FlutterError(code: "failed", message: error.localizedDescription, details: nil))
      }
    case "stop":
      manager?.connection.stopVPNTunnel()
      result(nil)
    default:
      result(FlutterMethodNotImplemented)
    }
  }

  /// The app's one tunnel configuration, saved (and enabled) when it is
  /// missing: saving it is what makes the system ask the user. False when
  /// the user declined or the system refused.
  private func prepare(_ done: @escaping (Bool) -> Void) {
    NETunnelProviderManager.loadAllFromPreferences { [weak self] managers, error in
      DispatchQueue.main.async {
        guard let self else { return done(false) }
        if let error {
          NSLog("vpn: load failed: \(error.localizedDescription)")
          return done(false)
        }
        let manager = managers?.first ?? NETunnelProviderManager()
        if manager.protocolConfiguration == nil {
          let tunnel = NETunnelProviderProtocol()
          tunnel.providerBundleIdentifier = Self.tunnelBundleID
          tunnel.serverAddress = "PPVPN"
          manager.protocolConfiguration = tunnel
          manager.localizedDescription = "PPVPN"
        }
        if manager.isEnabled && managers?.first != nil {
          self.use(manager)
          return done(true)
        }
        manager.isEnabled = true
        manager.saveToPreferences { error in
          if let error {
            NSLog("vpn: save failed: \(error.localizedDescription)")
            return DispatchQueue.main.async { done(false) }
          }
          // Saved: loaded again before it can start (NETunnelProviderManager).
          manager.loadFromPreferences { error in
            DispatchQueue.main.async {
              if let error {
                NSLog("vpn: reload failed: \(error.localizedDescription)")
                return done(false)
              }
              self.use(manager)
              done(true)
            }
          }
        }
      }
    }
  }

  private func use(_ manager: NETunnelProviderManager) {
    self.manager = manager
    publish()
  }

  func onListen(withArguments arguments: Any?, eventSink events: @escaping FlutterEventSink) -> FlutterError? {
    sink = events
    observer = NotificationCenter.default.addObserver(
      forName: .NEVPNStatusDidChange, object: nil, queue: .main
    ) { [weak self] _ in self?.publish() }
    // The configuration saved before this launch, if any: its status.
    NETunnelProviderManager.loadAllFromPreferences { [weak self] managers, _ in
      DispatchQueue.main.async {
        guard let self else { return }
        if self.manager == nil, let saved = managers?.first { self.manager = saved }
        self.publish()
      }
    }
    return nil
  }

  func onCancel(withArguments arguments: Any?) -> FlutterError? {
    if let observer { NotificationCenter.default.removeObserver(observer) }
    observer = nil
    sink = nil
    return nil
  }

  private func publish() {
    guard let sink else { return }
    switch manager?.connection.status ?? .invalid {
    case .connecting, .reasserting: sink("connecting")
    case .connected: sink("on")
    case .disconnecting: sink("disconnecting")
    default: sink("off")
    }
  }
}
