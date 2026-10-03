import AppKit
import Foundation
import PPVPNAppLogic
import PPVPNClient
import UserNotifications

let agentLog = AppLog(subsystem: "com.peakpassvpn.ppvpn.desktop", category: "push-agent")

/// The push agent: a UI-less helper that launchd keeps running at login. The
/// crate's `PushAgent` long-polls the backend; this shows each push as a
/// notification, and a click opens the message in the main app.
@MainActor
final class Agent: NSObject, NSApplicationDelegate, UNUserNotificationCenterDelegate {
    private let center = UNUserNotificationCenter.current()
    private var core: PushAgent!
    private var authorized = false
    private var signalSource: DispatchSourceSignal?

    func applicationDidFinishLaunching(_ notification: Notification) {
        center.delegate = self
        let (data, logs) = AppDirectories.prepare()
        let version = Bundle.main.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0"
        core = PushAgent(
            config: PushAgentConfig(dataDir: data.path, logDir: logs.path, platform: "macos", appVersion: version),
            listener: Listener(owner: self))
        agentLog.info("push agent: started \(Bundle.main.bundlePath)")

        // launchd stops the job with SIGTERM: stop cleanly so KeepAlive
        // (SuccessfulExit = false) does not start it again.
        signal(SIGTERM, SIG_IGN)
        let source = DispatchSource.makeSignalSource(signal: SIGTERM, queue: .main)
        source.setEventHandler { [core] in core?.stop() }
        source.resume()
        signalSource = source

        let core = core!
        Thread.detachNewThread {
            core.run()
            DispatchQueue.main.async { exit(0) }
        }
        Task { await watchAuthorization() }
    }

    // MARK: Notifications

    /// Retries what could not be shown once notifications become allowed
    /// (the user may allow them in System Settings at any time).
    private func watchAuthorization() async {
        while true {
            let allowed = Self.allows(await center.notificationSettings().authorizationStatus)
            if allowed, !authorized {
                agentLog.info("push agent: notifications allowed")
                core.retryPending()
            }
            authorized = allowed
            try? await Task.sleep(for: .seconds(60))
        }
    }

    nonisolated static func allows(_ status: UNAuthorizationStatus) -> Bool {
        status == .authorized || status == .provisional
    }

    /// Asks once, when there is something to show; the answer arrives later
    /// and `watchAuthorization` retries then.
    fileprivate func requestAuthorization() {
        Task {
            do {
                let granted = try await center.requestAuthorization(options: [.alert, .sound, .badge])
                agentLog.info("push agent: authorization \(granted ? "granted" : "denied")")
                if granted {
                    authorized = true
                    core.retryPending()
                }
            } catch {
                agentLog.error("push agent: authorization failed: \(error.localizedDescription)")
            }
        }
    }

    /// Shows `message`; blocks the calling (crate) thread until the system
    /// took it or refused.
    nonisolated fileprivate func show(_ message: PushMessage) -> Bool {
        let done = DispatchSemaphore(value: 0)
        let result = Result()
        UNUserNotificationCenter.current().getNotificationSettings { settings in
            guard Self.allows(settings.authorizationStatus) else {
                if settings.authorizationStatus == .notDetermined {
                    DispatchQueue.main.async { MainActor.assumeIsolated { self.requestAuthorization() } }
                }
                done.signal()
                return
            }
            let content = UNMutableNotificationContent()
            content.title = message.title
            content.body = message.body
            content.sound = .default
            content.threadIdentifier = String(describing: message.category)
            content.userInfo = ["id": String(message.id)]
            let request = UNNotificationRequest(identifier: "ppvpn-push-\(message.id)", content: content, trigger: nil)
            UNUserNotificationCenter.current().add(request) { error in
                if let error {
                    agentLog.error("push agent: notification \(message.id) failed: \(error.localizedDescription)")
                } else {
                    result.shown = true
                }
                done.signal()
            }
        }
        _ = done.wait(timeout: .now() + 15)
        return result.shown
    }

    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            willPresent notification: UNNotification) async
        -> UNNotificationPresentationOptions {
        [.banner, .list, .sound]
    }

    /// Opens ppvpn://push/<id> in the app this agent is bundled in; the app
    /// decides what the click does from its record of the push (the URL
    /// carries nothing else).
    nonisolated func userNotificationCenter(_ center: UNUserNotificationCenter,
                                            didReceive response: UNNotificationResponse) async {
        guard response.actionIdentifier == UNNotificationDefaultActionIdentifier,
              let id = (response.notification.request.content.userInfo["id"] as? String).flatMap(UInt64.init),
              let url = URL(string: "ppvpn://push/\(id)") else { return }
        await MainActor.run { Self.openInApp(url) }
    }

    private static func openInApp(_ url: URL) {
        // PPVPN.app/Contents/Library/LoginItems/PPVPN Agent.app
        let app = Bundle.main.bundleURL
            .deletingLastPathComponent().deletingLastPathComponent()
            .deletingLastPathComponent().deletingLastPathComponent()
        guard app.pathExtension == "app" else {
            NSWorkspace.shared.open(url)
            return
        }
        NSWorkspace.shared.open([url], withApplicationAt: app, configuration: NSWorkspace.OpenConfiguration()) { _, error in
            if let error { agentLog.error("push agent: open \(app.path) failed: \(error.localizedDescription)") }
        }
    }

    private final class Result: @unchecked Sendable {
        var shown = false
    }

    private final class Listener: PushAgentListener, @unchecked Sendable {
        private weak var owner: Agent?

        init(owner: Agent) { self.owner = owner }

        func onPush(message: PushMessage) -> Bool {
            owner?.show(message) ?? false
        }

        func onState(state: PushAgentState) {
            agentLog.info("push agent: \(String(describing: state))")
            guard state == .running else { return }
            // Signed in and registered: the moment to ask for permission.
            Task { @MainActor [weak owner] in
                guard let owner else { return }
                let status = await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
                if status == .notDetermined { owner.requestAuthorization() }
            }
        }
    }
}
