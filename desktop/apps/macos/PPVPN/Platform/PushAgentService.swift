import AppKit
import Foundation
import PPVPNAppLogic
import ServiceManagement

/// Keeps the bundled push agent (Contents/Library/LoginItems/PPVPN Agent.app)
/// registered with launchd, and re-registers it when it stopped running —
/// e.g. after the app was moved to the Trash and back, which leaves launchd
/// with a job it can no longer start.
@MainActor
final class PushAgentService {
    static let label = "com.peakpassvpn.ppvpn.desktop.agent"
    static let agentBundleID = "com.peakpassvpn.ppvpn.desktop.agent"

    private let service = SMAppService.agent(plistName: "\(label).plist")
    private let dataDirectory = AppDirectories.data
    /// What a heal is rate-limited by: the build. Debug builds keep one
    /// version, so there the agent binary's date stands in for it.
    private let version: String = {
        #if DEBUG
        let agent = Bundle.main.bundleURL
            .appendingPathComponent("Contents/Library/LoginItems/PPVPN Agent.app/Contents/MacOS/PPVPN Agent")
        let date = (try? FileManager.default.attributesOfItem(atPath: agent.path))?[.modificationDate] as? Date
        return "debug-\(Int(date?.timeIntervalSince1970 ?? 0))"
        #else
        return Bundle.main.object(forInfoDictionaryKey: "CFBundleVersion") as? String ?? "0"
        #endif
    }()
    private var watchingSince = Date()
    private var checkTask: Task<Void, Never>?
    private var wakeObserver: NSObjectProtocol?

    /// Registers the agent (if needed) and starts watching its heartbeat.
    func start() {
        // launchd would point at wherever this copy is; only an installed app
        // may own the agent (notifications also need an Applications folder).
        guard Self.isInstalled else {
            clientLog.info("push agent: not registering from \(Bundle.main.bundlePath)")
            return
        }
        register()
        scheduleCheck()
        wakeObserver = NSWorkspace.shared.notificationCenter.addObserver(
            forName: NSWorkspace.didWakeNotification, object: nil, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated {
                self?.watchingSince = Date()
                self?.scheduleCheck()
            }
        }
    }

    private static var isInstalled: Bool {
        let path = Bundle.main.bundlePath
        let home = FileManager.default.homeDirectoryForCurrentUser.path
        return path.hasPrefix("/Applications/") || path.hasPrefix(home + "/Applications/")
    }

    private func register() {
        switch service.status {
        case .enabled:
            // Builds signed without a Team ID get a new code requirement
            // each: launchd refuses the updated agent (OS_REASON_CODESIGNING)
            // until it is registered again, so re-register once per build.
            if UserDefaults.standard.string(forKey: Self.registeredBuildKey) != version {
                reregisterNewBuild()
            }
            return
        case .requiresApproval:
            // Switched off by the user in System Settings → Login Items.
            clientLog.info("push agent: disabled in Login Items")
            return
        case .notRegistered, .notFound:
            break
        @unknown default:
            break
        }
        do {
            try service.register()
            UserDefaults.standard.set(version, forKey: Self.registeredBuildKey)
            clientLog.info("push agent: registered, status \(service.status.rawValue)")
        } catch {
            clientLog.error("push agent: register failed: \(error.localizedDescription)")
        }
    }

    /// Checks a full window after launch or wake, then every few minutes.
    private func scheduleCheck() {
        checkTask?.cancel()
        checkTask = Task { [weak self] in
            try? await Task.sleep(for: .seconds(PushAgentHealth.staleAfter + 20))
            while !Task.isCancelled {
                self?.check()
                try? await Task.sleep(for: .seconds(5 * 60))
            }
        }
    }

    private func check() {
        guard service.status == .enabled else { return }
        let heartbeat = PushAgentHealth.heartbeatDate(in: dataDirectory)
        guard PushAgentHealth.isDown(heartbeat: heartbeat, watchingSince: watchingSince, now: Date(),
                                     agentRunning: Self.agentRunning) else { return }
        // A clean exit (SIGTERM) is not restarted by KeepAlive: try a plain
        // restart first, then re-register (rate-limited) if that fails.
        clientLog.info("push agent: not running; restarting")
        Self.kickstart()
        Task { [weak self] in
            try? await Task.sleep(for: .seconds(10))
            guard let self, !Self.agentRunning else { return }
            let now = Date()
            guard PushAgentHealth.shouldHeal(
                heartbeat: heartbeat, watchingSince: watchingSince, now: now, agentRunning: false,
                lastHeal: PushAgentHealth.readHeal(in: dataDirectory), version: version)
            else { return }
            clientLog.info("push agent: restart failed; re-registering")
            PushAgentHealth.writeHeal(.init(version: version, at: now), in: dataDirectory)
            reregister()
        }
    }

    private static var agentRunning: Bool {
        !NSRunningApplication.runningApplications(withBundleIdentifier: agentBundleID).isEmpty
    }

    private static func kickstart() {
        let process = Process()
        process.executableURL = URL(fileURLWithPath: "/bin/launchctl")
        process.arguments = ["kickstart", "gui/\(getuid())/\(label)"]
        do {
            try process.run()
        } catch {
            clientLog.error("push agent: kickstart failed: \(error.localizedDescription)")
        }
    }

    private static let registeredBuildKey = "pushAgentRegisteredBuild"

    /// Right at launch a re-registration still gets the old code
    /// requirement (the system picks the new bundle up a little later), so
    /// wait before re-registering and try again (for up to ~6 minutes) until
    /// the agent runs. The
    /// previous build's agent keeps working meanwhile. These attempts do not
    /// count against the heal limit.
    private func reregisterNewBuild() {
        clientLog.info("push agent: new build \(version); re-registering shortly")
        Task { [weak self] in
            for delay in [30, 60, 60, 90, 120] {
                try? await Task.sleep(for: .seconds(delay))
                guard let self else { return }
                await self.reregisterAndWait()
                if Self.agentRunning {
                    UserDefaults.standard.set(self.version, forKey: Self.registeredBuildKey)
                    clientLog.info("push agent: running for build \(self.version)")
                    return
                }
            }
            clientLog.error("push agent: not running after re-registering build \(self?.version ?? "?")")
        }
    }

    private func reregisterAndWait() async {
        do {
            try await service.unregister()
        } catch {
            clientLog.error("push agent: unregister failed: \(error.localizedDescription)")
        }
        do {
            try service.register()
        } catch {
            clientLog.error("push agent: register failed: \(error.localizedDescription)")
        }
        try? await Task.sleep(for: .seconds(10))
    }

    /// Unregisters, waits until launchd has removed the job, then registers
    /// again: registering while the old job is still being torn down keeps
    /// its stale code requirement and the agent fails with EX_CONFIG.
    private func reregister() {
        Task {
            do {
                try await service.unregister()
            } catch {
                clientLog.error("push agent: unregister failed: \(error.localizedDescription)")
            }
            do {
                try service.register()
                clientLog.info("push agent: re-registered, status \(service.status.rawValue)")
            } catch {
                clientLog.error("push agent: register failed: \(error.localizedDescription)")
            }
        }
    }
}
