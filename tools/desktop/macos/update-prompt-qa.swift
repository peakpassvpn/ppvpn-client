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
  private var clientScenario = "transition"
  private var transitionClientWasActive = false
  private var otherReentryConfirmed = false
  private var userRestoreRequested = false
  private var userRestoreConfirmed = false
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
    let before=NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1
    NSApp.activate(ignoringOtherApps: true)
    let event="\(Date().timeIntervalSince1970) mode=\(full ? "full" : "windowed") pid=\(ProcessInfo.processInfo.processIdentifier) frame=\(window.frame) screen=\(screen.frame) active=\(NSApp.isActive) beforeFront=\(before) afterFront=\(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1)\n"
    let eventURL=dir.appendingPathComponent("other-activation.txt")
    if FileManager.default.fileExists(atPath:eventURL.path) {
      let file=try! FileHandle(forWritingTo:eventURL);file.seekToEndOfFile();file.write(Data(event.utf8));try! file.close()
    } else { try! Data(event.utf8).write(to:eventURL) }
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
  private struct FixtureSnapshot {
    let details: String
    // nil is unavailable/unknown; it must never establish fullscreen.
    let isFullscreen: Bool?
  }
  private func fixtureWindowSnapshot(_ pid: pid_t) -> FixtureSnapshot {
    guard let windows=CGWindowListCopyWindowInfo([.excludeDesktopElements],kCGNullWindowID) as? [[String:Any]],!windows.isEmpty else { return FixtureSnapshot(details:"window snapshot unavailable/empty",isFullscreen:nil) }
    var count:UInt32=0
    guard CGGetActiveDisplayList(0,nil,&count) == .success,count>0 else { return FixtureSnapshot(details:"display list unavailable/empty",isFullscreen:nil) }
    var displays=[CGDirectDisplayID](repeating:0,count:Int(count))
    guard CGGetActiveDisplayList(count,&displays,&count) == .success,count>0 else { return FixtureSnapshot(details:"display list unavailable/empty",isFullscreen:nil) }
    let monitors=displays.prefix(Int(count)).map { CGDisplayBounds($0) }
    guard monitors.allSatisfy({ !$0.isNull && !$0.isInfinite && $0.width>1 && $0.height>1 }) else { return FixtureSnapshot(details:"display bounds invalid",isFullscreen:nil) }
    var visibleFrames=[CGRect]()
    var details=[String]()
    for item in windows where (item[kCGWindowOwnerPID as String] as? Int32)==pid && (item[kCGWindowLayer as String] as? Int)==0 {
      let onScreen=item[kCGWindowIsOnscreen as String] as? Bool
      guard let bounds=item[kCGWindowBounds as String] as? [String:Any],let frame=CGRect(dictionaryRepresentation:bounds as CFDictionary),!frame.isNull,!frame.isInfinite,frame.width>1,frame.height>1 else { continue }
      details.append("id=\(item[kCGWindowNumber as String] as? UInt32 ?? 0) onScreen=\(String(describing:onScreen)) frame=\(frame)")
      if onScreen == true { visibleFrames.append(frame) }
    }
    let text="\(details.joined(separator:";")) displays=\(monitors)"
    guard !visibleFrames.isEmpty else { return FixtureSnapshot(details:text+"; no valid on-screen fixture frame",isFullscreen:nil) }
    let full=visibleFrames.contains { frame in monitors.contains { monitor in
      frame.minX<=monitor.minX && frame.minY<=monitor.minY && frame.maxX>=monitor.maxX && frame.maxY>=monitor.maxY
    } }
    return FixtureSnapshot(details:text,isFullscreen:full)
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
            self.log("PHASE actual client \(self.clientScenario) launch pid=\(app.processIdentifier) during other-app fullscreen")
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
        capture("actual-client-\(clientScenario)-first-show-deferred")
        log("PASS actual client remains running without first-show/fullscreen interruption")
        try! Data("windowed".utf8).write(to:modeURL)
        phase=clientScenario == "transition" ? "first-windowed" : "baseline-windowed";phaseAt=Date()
        finalStableAt=nil
      }
    case "first-windowed":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL original client exited before reopen transition") }
      let events = (try? String(contentsOf:dir.appendingPathComponent("native-live.txt"),encoding:.utf8)) ?? ""
      let request = events.components(separatedBy:.newlines).first { $0.contains("PPVPN[\(firstApp.processIdentifier):") && $0.contains("update launch: workspace activation requested") }
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
        transitionClientWasActive = NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp.processIdentifier
        log("TRANSITION immediately reenter fullscreen pid=\(firstApp.processIdentifier) originalWindows=\(originalClientWindows.sorted()) activeAlready=\(transitionClientWasActive); log timing is not proof of pending intent")
        try! Data("full".utf8).write(to:modeURL)
        // This is the fixture user's new fullscreen intention. Its deprecated
        // ignoringOtherApps call alone may not activate an already-running app.
        // Request that existing fixture explicitly, once; never touch the client.
        let configuration=NSWorkspace.OpenConfiguration()
        configuration.activates=true
        configuration.createsNewApplicationInstance=false
        configuration.allowsRunningApplicationSubstitution=false
        configuration.promptsUserIfNeeded=false
        let activationLag=Date().timeIntervalSince(emitted)
        guard activationLag >= 0 && activationLag <= 0.25 else { finish(1,"FAIL fixture activation request outside immediate transition bound") }
        log("OTHER_REENTRY requesting explicit fixture activation originalOtherPID=\(other?.processIdentifier ?? -1) requestLag=\(activationLag)")
        NSWorkspace.shared.openApplication(at:URL(fileURLWithPath:CommandLine.arguments[3]),configuration:configuration) { app,error in
          Task { @MainActor in
            guard error == nil, let app,app.processIdentifier==self.other?.processIdentifier else { self.finish(1,"FAIL fullscreen fixture activation did not confirm original other-app PID") }
            self.otherReentryConfirmed=true
            self.log("OTHER_REENTRY completion sameOtherPID=\(app.processIdentifier) foreground=\(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1)")
          }
        }
        phase="reentering-fullscreen";phaseAt=Date()
      } else if elapsed > 20 { finish(1,"FAIL actual workspace activation request not observed; transition not exercised") }
    case "reentering-fullscreen":
      let ready=(try? String(contentsOf:dir.appendingPathComponent("other-ready"),encoding:.utf8)) ?? "missing"
      let snapshot=fixtureWindowSnapshot(other?.processIdentifier ?? -1)
      log("SAMPLE reentry foreground=\(NSWorkspace.shared.frontmostApplication?.processIdentifier ?? -1) otherPID=\(other?.processIdentifier ?? -1) otherReady=\(ready) shouldDefer=\(UpdatePromptPolicy.shouldDeferPrompt) actualFull=\(String(describing:snapshot.isFullscreen)) frames=\(snapshot.details)")
      if NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp?.processIdentifier { transitionClientWasActive=true }
      if otherReentryConfirmed,ready == "full",snapshot.isFullscreen == true,NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier {
        phase="second-fullscreen";phaseAt=Date()
        log("PASS actual other-app fullscreen reentered after workspace request; activation consumption evidence=\(transitionClientWasActive ? "client was active" : "unproven, not asserted pending")")
      } else if elapsed > 5 {
        if snapshot.isFullscreen == nil { finish(77,"BLOCKED fullscreen fixture CG evidence unavailable; transition not exercised") }
        finish(1,"FAIL other-app fullscreen reentry not reached after request")
      }
    case "second-fullscreen":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL original client exited during reopen transition") }
      guard NSWorkspace.shared.frontmostApplication?.processIdentifier == other?.processIdentifier else { finish(1,"FAIL late activation/reopen stole repeated fullscreen foreground") }
      let snapshot=fixtureWindowSnapshot(other?.processIdentifier ?? -1)
      guard let actualFull=snapshot.isFullscreen else { finish(77,"BLOCKED fullscreen fixture CG evidence lost during strict interval") }
      guard actualFull else { finish(1,"FAIL actual fullscreen fixture geometry lost during strict interval") }
      let ids=clientWindowIDs(firstApp.processIdentifier)
      guard ids==originalClientWindows else { finish(1,"FAIL native original-window identity changed during reopen transition") }
      log("SAMPLE reopen-transition fullscreen pid=\(firstApp.processIdentifier) foreground=\(other!.processIdentifier) originalWindows=\(ids.sorted()) frames=\(snapshot.details)")
      if elapsed >= 8 {
        capture("actual-client-reopen-transition-fullscreen")
        log("PASS no late activation or Dart reopen focus during second fullscreen")
        try! Data("windowed".utf8).write(to:modeURL)
        phase="user-restore";phaseAt=Date();finalStableAt=nil
      }
    case "user-restore":
      guard let firstApp, !firstApp.isTerminated,clientWindowIDs(firstApp.processIdentifier)==originalClientWindows else { finish(1,"FAIL original client/window lost before explicit user restore") }
      let otherWindowed=(try? String(contentsOf:dir.appendingPathComponent("other-ready"),encoding:.utf8)) == "windowed"
      if !userRestoreRequested && otherWindowed && !UpdatePromptPolicy.shouldDeferPrompt {
        userRestoreRequested=true
        let alreadyFront=NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp.processIdentifier
        log("USER_RESTORE explicit existing-app reopen after fullscreen; no second automatic activation required; alreadyFront=\(alreadyFront)")
        let configuration=NSWorkspace.OpenConfiguration()
        configuration.activates=true
        configuration.createsNewApplicationInstance=false
        configuration.allowsRunningApplicationSubstitution=false
        NSWorkspace.shared.openApplication(at:URL(fileURLWithPath:CommandLine.arguments[4]),configuration:configuration) { app,error in
          Task { @MainActor in
            guard error == nil, let app, app.processIdentifier==firstApp.processIdentifier else { self.finish(1,"FAIL explicit user restore did not confirm original client PID without error") }
            self.userRestoreConfirmed=true
            self.log("USER_RESTORE completion confirmed original PID \(app.processIdentifier)")
          }
        }
      }
      if userRestoreConfirmed,clientWindowVisible(firstApp.processIdentifier),NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp.processIdentifier {
        if finalStableAt == nil { finalStableAt=Date() }
        if Date().timeIntervalSince(finalStableAt!) >= 2 {
          capture("actual-client-transition-user-restored")
          log("PASS transition: strict second fullscreen retained, same PID/window recover through explicit user intent")
          firstApp.terminate()
          phase="stop-transition-client";phaseAt=Date()
        }
      } else { finalStableAt=nil }
      if elapsed > 20 { finish(1,"FAIL explicit user restore of original client/window did not complete") }
    case "stop-transition-client":
      if firstApp?.isTerminated == true {
        log("PASS transition client terminated through normal application quit")
        firstApp=nil
        clientScenario="baseline"
        try! Data("full".utf8).write(to:modeURL)
        phase="first-start";phaseAt=Date()
      } else if elapsed > 20 { finish(1,"FAIL transition client did not exit before independent baseline") }
    case "baseline-windowed":
      guard let firstApp, !firstApp.isTerminated else { finish(1,"FAIL baseline client exited") }
      if clientWindowVisible(firstApp.processIdentifier),NSWorkspace.shared.frontmostApplication?.processIdentifier == firstApp.processIdentifier {
        if finalStableAt == nil { finalStableAt=Date() }
        if Date().timeIntervalSince(finalStableAt!) >= 2 {
          capture("actual-client-baseline-automatic-restored")
          finish(0,"PASS independent automatic recovery baseline plus strict actual-request fullscreen transition/user restore")
        }
      } else { finalStableAt=nil }
      if elapsed > 20 { finish(1,"FAIL independent baseline automatic window/focus recovery") }
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
