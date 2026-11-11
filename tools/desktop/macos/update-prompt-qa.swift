// Compiled together with the exact policy region from MainFlutterWindow.swift.
// Loads the actual pinned standard Sparkle framework and its original UI.
import Cocoa
import Sparkle
import os

private let updateLog = Logger(subsystem: "PPVPN.UpdateQA", category: "updates")
@MainActor private final class UpdateQA: NSObject, NSApplicationDelegate, SPUStandardUserDriverDelegate {
  private var controller: SPUStandardUpdaterController?
  private var timer: Timer?
  private var other: NSRunningApplication?
  private var window: NSWindow?
  private var fullscreen = true
  private var phase = "starting"
  private var phaseAt = Date()
  private let dir = URL(fileURLWithPath: CommandLine.arguments[2])
  private var modeURL: URL { dir.appendingPathComponent("mode") }
  private func log(_ value: String) { print("\(Date().timeIntervalSince1970) \(value)"); fflush(stdout) }
  private func capture(_ name: String) {
    let task = Process()
    task.executableURL = URL(fileURLWithPath:"/usr/sbin/screencapture")
    task.arguments = ["-x",dir.appendingPathComponent(name+".png").path]
    try? task.run()
    task.waitUntilExit()
  }
  private func finish(_ code: Int32, _ value: String) -> Never {
    log(value)
    if code != 0 { capture("failure") }
    if CommandLine.arguments[1] != "other" { other?.terminate() }
    exit(code)
  }
  @objc var supportsGentleScheduledUpdateReminders: Bool { true }
  @objc func ppvpnShouldDeferUpdatePrompt() -> Bool { UpdatePromptPolicy.shouldDeferPrompt }
  private var visibleAlerts: [NSWindow] {
    NSApp.windows.filter {
      guard $0.isVisible, let controller = $0.windowController else { return false }
      return NSStringFromClass(type(of: controller)) == "SUUpdateAlert"
    }
  }
  private func setOtherWindow(_ full: Bool) {
    guard let window else { return }
    fullscreen = full
    window.styleMask = full ? .borderless : [.titled, .closable, .resizable]
    guard let screen = NSScreen.main else { finish(77, "BLOCKED no main display") }
    window.setFrame(full ? screen.frame : NSRect(x: 100,y: 100,width: 700,height: 450), display: true)
    window.makeKeyAndOrderFront(nil)
    NSApp.activate(ignoringOtherApps: true)
    try! Data((full ? "full" : "windowed").utf8).write(to: dir.appendingPathComponent("other-ready"))
  }
  func applicationDidFinishLaunching(_ notification: Notification) {
    guard let session = CGSessionCopyCurrentDictionary() as? [String: Any],
      session[kCGSessionOnConsoleKey as String] as? Bool == true else { finish(77,"BLOCKED no active console") }
    NSApp.setActivationPolicy(.regular)
    if CommandLine.arguments[1] == "other" {
      window = NSWindow(contentRect:NSRect(x:100,y:100,width:700,height:450),styleMask:.borderless,backing:.buffered,defer:false)
      window?.title = "PPVPN QA Other App"
      let label = NSTextField(labelWithString: "OTHER APP — actual foreground fullscreen fixture")
      label.frame = NSRect(x:40,y:40,width:650,height:40)
      window?.contentView?.addSubview(label)
      setOtherWindow(true)
      timer = Timer.scheduledTimer(withTimeInterval:0.1,repeats:true) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          if self.fullscreen && (try? String(contentsOf:self.modeURL,encoding:.utf8)) == "windowed" { self.setOtherWindow(false) }
        }
      }
      return
    }
    guard let driver = NSClassFromString("SPUStandardUserDriver"), class_getClassMethod(driver, NSSelectorFromString("ppvpnHasPreShowGate")) != nil else { finish(1,"FAIL unpatched Sparkle") }
    controller = SPUStandardUpdaterController(startingUpdater:true,updaterDelegate:nil,userDriverDelegate:self)
    controller?.updater.automaticallyChecksForUpdates = false
    NSApp.activate(ignoringOtherApps:true)
    controller?.checkForUpdates(nil)
    log("PHASE manual check sent before fullscreen; feed delays three seconds")
    DispatchQueue.main.asyncAfter(deadline:.now()+0.5) {
      let config = NSWorkspace.OpenConfiguration()
      config.arguments = ["other",self.dir.path]
      NSWorkspace.shared.openApplication(at:URL(fileURLWithPath:CommandLine.arguments[3]),configuration:config) { app,error in
        MainActor.assumeIsolated {
          guard let app else { self.finish(1,"FAIL other app launch: \(String(describing:error))") }
          self.other = app
          try! Data(String(app.processIdentifier).utf8).write(to:self.dir.appendingPathComponent("other-pid.txt"))
        }
      }
    }
    timer = Timer.scheduledTimer(withTimeInterval:0.1,repeats:true) { [weak self] _ in
      MainActor.assumeIsolated { self?.observe() }
    }
  }
  private func observe() {
    let elapsed = Date().timeIntervalSince(phaseAt)
    switch phase {
    case "starting":
      if let other, NSWorkspace.shared.frontmostApplication?.processIdentifier == other.processIdentifier,
        UpdatePromptPolicy.shouldDeferPrompt {
        phase="fullscreen";phaseAt=Date();log("PASS actual other-app foreground fullscreen")
      } else if elapsed > 15 { finish(1,"FAIL other-app fullscreen not reached") }
    case "fullscreen":
      guard visibleAlerts.isEmpty else { finish(1,"FAIL native update alert displayed during fullscreen") }
      guard NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier else { finish(1,"FAIL fullscreen focus stolen") }
      log("SAMPLE fullscreen foreground=\(other!.processIdentifier) nativeAlerts=0")
      if elapsed >= 7 {
        capture("fullscreen-no-prompt")
        log("PASS delayed manual result did not interrupt fullscreen")
        try! Data("windowed".utf8).write(to:modeURL)
        phase="windowed";phaseAt=Date()
      }
    case "windowed":
      if !visibleAlerts.isEmpty {
        capture("after-fullscreen")
        log("PASS original Sparkle update alert appears after fullscreen exits")
        visibleAlerts.forEach { $0.performClose(nil) }
        finish(0,"PASS actual standard Sparkle manual delayed-result UI")
      } else if elapsed > 15 { finish(1,"FAIL deferred native alert never appeared") }
    default: break
    }
  }
}
let delegate = UpdateQA()
NSApplication.shared.delegate = delegate
NSApplication.shared.run()
