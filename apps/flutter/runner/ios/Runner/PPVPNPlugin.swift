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
// A build for the simulator also prints the Dart VM service's address for
// flutter test (VMServiceAnnouncer below).
//
// The packet tunnel itself (com.peakpassvpn.ppvpn.tunnel) is not in the
// app yet: it comes with the tunnel host's C functions (#448 step 2).

import Flutter
import NetworkExtension
import UIKit
import os

final class PPVPNPlugin: NSObject, FlutterPlugin, FlutterStreamHandler {
  static let appGroup = "group.com.peakpassvpn.ppvpn"
  static let tunnelBundleID = "com.peakpassvpn.ppvpn.tunnel"

  private var manager: NETunnelProviderManager?
  private var sink: FlutterEventSink?
  private var observer: NSObjectProtocol?

  static func register(with registrar: FlutterPluginRegistrar) {
    let plugin = PPVPNPlugin()
    let messenger = registrar.messenger()
    #if targetEnvironment(simulator)
      VMServiceAnnouncer.start(registrar)
    #endif
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

#if targetEnvironment(simulator)
  /// flutter test on the simulator (e2e/ios_smoke.sh) learns the app's VM
  /// service address from the simulator's log, through a `log stream` it
  /// spawns while it launches the app, and the engine prints the address
  /// once, about a second after launch. A stream still attaching then never
  /// sees it, and the tool waits until it gives up. The test's own Dart code
  /// cannot help: flutter test's bootstrap runs it only once the tool has
  /// connected. So the app prints the line again every second for its first
  /// minute, in the form the tool looks for, from the Runner executable
  /// (the tool's predicate takes the app's own image or Flutter's; ci-build.sh
  /// turns off the debug dylib for that). The tool takes the first it reads.
  ///
  /// Not `#if DEBUG`: flutter's Runner target does not set that condition
  /// (only its RunnerTests do), so the announcer was compiled out (#484).
  /// A release build has no VM service, and its vmServiceUrl stays nil.
  enum VMServiceAnnouncer {
    private static let log = Logger(subsystem: "com.peakpassvpn.ppvpn", category: "vm-service")

    static func start(_ registrar: FlutterPluginRegistrar) {
      var left = 60
      Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { timer in
        left -= 1
        if left == 0 { timer.invalidate() }
        guard let url = engine(registrar)?.vmServiceUrl else { return }
        log.notice("The Dart VM service is listening on \(url.absoluteString, privacy: .public)")
      }
    }

    private static func engine(_ registrar: FlutterPluginRegistrar) -> FlutterEngine? {
      if let controller = registrar.viewController as? FlutterViewController { return controller.engine }
      let windows = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.flatMap(\.windows)
      return windows.lazy.compactMap { $0.rootViewController as? FlutterViewController }.first?.engine
    }
  }
#endif
