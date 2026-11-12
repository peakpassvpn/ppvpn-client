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
  private var firstApp: NSRunningApplication?
  private var window: NSWindow?
  private var fullscreen = true
  private var phase = "starting"
  private var phaseAt = Date()
  private var originalClientWindows = Set<UInt32>()
  private var finalStableAt: Date?
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
    if CommandLine.arguments[1] != "other" { firstApp?.terminate(); other?.terminate() }
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
      timer = Timer.scheduledTimer(withTimeInterval:0.05,repeats:true) { [weak self] _ in
        MainActor.assumeIsolated {
          guard let self else { return }
          if let mode = try? String(contentsOf:self.modeURL,encoding:.utf8),
            (mode == "full") != self.fullscreen { self.setOtherWindow(mode == "full") }
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
        Task { @MainActor in
          guard let app else { self.finish(1,"FAIL other app launch: \(String(describing:error))") }
          self.other = app
          try! Data(String(app.processIdentifier).utf8).write(to:self.dir.appendingPathComponent("other-pid.txt"))
        }
      }
    }
    timer = Timer.scheduledTimer(withTimeInterval:0.05,repeats:true) { [weak self] _ in
      MainActor.assumeIsolated { self?.observe() }
    }
  }
  private func clientWindowIDs(_ pid: pid_t) -> Set<UInt32> {
    guard let windows = CGWindowListCopyWindowInfo([.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]] else { finish(1,"FAIL original window identity observation unavailable") }
    return Set(windows.compactMap {
      guard ($0[kCGWindowOwnerPID as String] as? Int32) == pid,
        ($0[kCGWindowLayer as String] as? Int) == 0,
        let bounds = $0[kCGWindowBounds as String] as? [String:Any],
        let frame = CGRect(dictionaryRepresentation:bounds as CFDictionary),frame.width>1,frame.height>1 else { return nil }
      return $0[kCGWindowNumber as String] as? UInt32
    })
  }
  private func clientWindowVisible(_ pid: pid_t) -> Bool {
    guard let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly,.excludeDesktopElements], kCGNullWindowID) as? [[String:Any]] else { finish(1,"FAIL window observation unavailable") }
    return windows.contains {
      guard ($0[kCGWindowOwnerPID as String] as? Int32) == pid,
        ($0[kCGWindowLayer as String] as? Int) == 0,
        let bounds = $0[kCGWindowBounds as String] as? [String:Any],
        let frame = CGRect(dictionaryRepresentation:bounds as CFDictionary) else { return false }
      return frame.width > 1 && frame.height > 1
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
        if CommandLine.arguments.count > 4 {
          try! Data("full".utf8).write(to:modeURL)
          phase="first-start";phaseAt=Date()
        } else { finish(0,"PASS actual standard Sparkle manual delayed-result UI") }
      } else if elapsed > 15 { finish(1,"FAIL deferred native alert never appeared") }
    case "first-start":
      if NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier,
        UpdatePromptPolicy.shouldDeferPrompt {
        phase="first-launching";phaseAt=Date()
        let config = NSWorkspace.OpenConfiguration()
        config.activates = false
        config.arguments = ["--ppvpn-after-update"]
        NSWorkspace.shared.openApplication(at:URL(fileURLWithPath:CommandLine.arguments[4]),configuration:config) { app,error in
          Task { @MainActor in
            guard let app else { self.finish(1,"FAIL real client launch: \(String(describing:error))") }
            self.firstApp=app
            try! Data(String(app.processIdentifier).utf8).write(to:self.dir.appendingPathComponent("client-pid.txt"))
            self.phase="first-fullscreen";self.phaseAt=Date()
            self.log("PHASE actual client first launch pid=\(app.processIdentifier) during other-app fullscreen")
          }
        }
      } else if elapsed > 15 { finish(1,"FAIL other app did not reenter fullscreen") }
    case "first-launching":
      // NSWorkspace's completion can follow nib loading. Observe from the
      // launch request onward, including that initial native interval.
      guard NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier else { finish(1,"FAIL client launch request interrupted fullscreen foreground") }
      let expectedURL = URL(fileURLWithPath:CommandLine.arguments[4]).standardizedFileURL
      for candidate in NSRunningApplication.runningApplications(withBundleIdentifier:"com.peakpassvpn.ppvpn.desktop")
      where candidate.bundleURL?.standardizedFileURL == expectedURL {
        guard !clientWindowVisible(candidate.processIdentifier) else { finish(1,"FAIL native client window appeared before launch completion") }
        log("SAMPLE launching-client pid=\(candidate.processIdentifier) onScreenWindows=0")
      }
      if elapsed > 15 { finish(1,"FAIL actual client launch timed out") }
    case "first-fullscreen":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL actual client exited before first-show check") }
      guard NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier else { finish(1,"FAIL actual client launch stole fullscreen foreground") }
      guard !clientWindowVisible(firstApp.processIdentifier) else { finish(1,"FAIL actual client nib/first window appeared during fullscreen") }
      log("SAMPLE actual-client fullscreen pid=\(firstApp.processIdentifier) onScreenWindows=0")
      if elapsed >= 7 {
        capture("actual-client-first-show-deferred")
        log("PASS actual client remains running without first-show/fullscreen interruption")
        try! Data("windowed".utf8).write(to:modeURL)
        phase="first-windowed";phaseAt=Date()
      }
    case "first-windowed":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL original client exited before reopen transition") }
      let events = (try? String(contentsOf:dir.appendingPathComponent("native-live.txt"),encoding:.utf8)) ?? ""
      let request = events.split(separator:"\n").first { $0.contains("PPVPN[\(firstApp.processIdentifier):") && $0.contains("update launch: workspace activation requested") }
      if let request {
        let formatter=DateFormatter()
        formatter.locale=Locale(identifier:"en_US_POSIX")
        formatter.timeZone=TimeZone.current
        formatter.dateFormat="yyyy-MM-dd HH:mm:ss.SSS"
        guard let emitted=formatter.date(from:String(request.prefix(23))) else { finish(1,"FAIL live request timestamp unparseable; transition not exercised") }
        let lag=Date().timeIntervalSince(emitted)
        log("TRANSITION live request observation lag=\(lag) seconds")
        guard lag >= 0 && lag <= 0.25 else { finish(1,"FAIL request observation too late; immediate transition not exercised") }
        originalClientWindows=clientWindowIDs(firstApp.processIdentifier)
        guard !originalClientWindows.isEmpty else { finish(1,"FAIL no original native window identity at workspace request") }
        log("TRANSITION observed actual request: \(request)")
        log("TRANSITION immediately reenter fullscreen pid=\(firstApp.processIdentifier) originalWindows=\(originalClientWindows.sorted())")
        try! Data("full".utf8).write(to:modeURL)
        phase="reentering-fullscreen";phaseAt=Date()
      } else if elapsed > 20 { finish(1,"FAIL actual workspace activation request not observed; transition not exercised") }
    case "reentering-fullscreen":
      if NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier,
        UpdatePromptPolicy.shouldDeferPrompt {
        phase="second-fullscreen";phaseAt=Date()
        log("PASS actual other-app fullscreen reentered after workspace request")
      } else if elapsed > 5 { finish(1,"FAIL other-app fullscreen reentry not reached after request") }
    case "second-fullscreen":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL original client exited during reopen transition") }
      guard NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier else { finish(1,"FAIL late activation/reopen stole repeated fullscreen foreground") }
      let ids=clientWindowIDs(firstApp.processIdentifier)
      guard ids==originalClientWindows else { finish(1,"FAIL native original-window identity changed during reopen transition") }
      log("SAMPLE reopen-transition fullscreen pid=\(firstApp.processIdentifier) foreground=\(other!.processIdentifier) originalWindows=\(ids.sorted())")
      if elapsed >= 8 {
        capture("actual-client-reopen-transition-fullscreen")
        log("PASS no late activation or Dart reopen focus during second fullscreen")
        try! Data("windowed".utf8).write(to:modeURL)
        phase="final-windowed";phaseAt=Date()
      }
    case "final-windowed":
      if let firstApp, !firstApp.isTerminated,clientWindowIDs(firstApp.processIdentifier)==originalClientWindows,
        clientWindowVisible(firstApp.processIdentifier),NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp.processIdentifier {
        if finalStableAt == nil { finalStableAt=Date() }
        if Date().timeIntervalSince(finalStableAt!) >= 2 {
          capture("actual-client-after-fullscreen")
          finish(0,"PASS same client/original window restores after reopen transition without late fullscreen focus")
        }
      } else { finalStableAt=nil }
      if elapsed > 20 { finish(1,"FAIL same original client window/focus did not recover after second fullscreen") }
    default: break
    }
  }
}
@main private struct UpdateQAMain {
  @MainActor static func main() {
    let delegate = UpdateQA()
    NSApplication.shared.delegate = delegate
    withExtendedLifetime(delegate) { NSApplication.shared.run() }
  }
}
