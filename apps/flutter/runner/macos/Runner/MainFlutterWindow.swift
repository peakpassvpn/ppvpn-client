// The generated runner's window (flutter create), plus the channel
// lib/activation.dart listens on (#401 step 4). scripts/ci-build.sh copies
// this over the generated file.
//
// A click on a notification comes back as ppvpn://push/<id>, which the push
// agent opens (as for the native app, PPVPNApp.swift); opening the app again
// (the Dock icon, Finder) comes as a reopen. Both go to Dart; what arrives
// before Dart asks ("ready") is held until then.

import Cocoa
import FlutterMacOS

class MainFlutterWindow: NSWindow {
  private var activation: Activation?

  override func awakeFromNib() {
    let flutterViewController = FlutterViewController()
    let windowFrame = self.frame
    self.contentViewController = flutterViewController
    self.setFrame(windowFrame, display: true)

    RegisterGeneratedPlugins(registry: flutterViewController)
    activation = Activation(messenger: flutterViewController.engine.binaryMessenger)

    super.awakeFromNib()
  }
}

private final class Activation: NSObject {
  private let channel: FlutterMethodChannel
  private var pending: [String] = []
  private var ready = false

  init(messenger: FlutterBinaryMessenger) {
    channel = FlutterMethodChannel(name: "ppvpn/activation", binaryMessenger: messenger)
    super.init()
    channel.setMethodCallHandler { [weak self] call, result in
      guard let self, call.method == "ready" else {
        result(FlutterMethodNotImplemented)
        return
      }
      self.ready = true
      result(self.pending)
      self.pending = []
    }
    // The window loads before the app finishes launching. Apple event
    // handlers set at will-finish replace AppKit's and come before the
    // launch's own URL.
    NotificationCenter.default.addObserver(
      self, selector: #selector(willFinishLaunching(_:)),
      name: NSApplication.willFinishLaunchingNotification, object: nil)
  }

  @objc private func willFinishLaunching(_ notification: Notification) {
    let events = NSAppleEventManager.shared()
    events.setEventHandler(
      self, andSelector: #selector(getURL(_:withReplyEvent:)),
      forEventClass: AEEventClass(kInternetEventClass), andEventID: AEEventID(kAEGetURL))
    events.setEventHandler(
      self, andSelector: #selector(reopen(_:withReplyEvent:)),
      forEventClass: AEEventClass(kCoreEventClass), andEventID: AEEventID(kAEReopenApplication))
  }

  @objc private func getURL(_ event: NSAppleEventDescriptor, withReplyEvent reply: NSAppleEventDescriptor) {
    if let url = event.paramDescriptor(forKeyword: AEKeyword(keyDirectObject))?.stringValue {
      send(url)
    }
  }

  @objc private func reopen(_ event: NSAppleEventDescriptor, withReplyEvent reply: NSAppleEventDescriptor) {
    send("show")
  }

  private func send(_ item: String) {
    if ready {
      channel.invokeMethod("activate", arguments: item)
    } else {
      pending.append(item)
    }
  }
}
