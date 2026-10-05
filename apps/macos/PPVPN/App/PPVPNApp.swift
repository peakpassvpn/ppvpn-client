import PPVPNAppLogic
import PPVPNClient
import SwiftUI

@main
struct PPVPNApp: App {
    @NSApplicationDelegateAdaptor private var appDelegate: AppDelegate
    @StateObject private var model = AppModel.live
    @AppStorage(Appearance.storageKey) private var appearance = Appearance.system

    var body: some Scene {
        Window("PPVPN", id: WindowID.main) {
            MainWindow()
                .environmentObject(model)
                .onAppear { appearance.apply() }
                .onChange(of: appearance) { $0.apply() }
        }
        .windowResizability(.contentMinSize)
        .defaultSize(width: 760, height: 540)
        .windowToolbarStyle(.unified(showsTitle: false))
        .commands {
            CommandGroup(replacing: .newItem) {}
            CommandGroup(replacing: .appInfo) {
                Button(tr("about")) { AboutPanel.show() }
            }
            CommandGroup(after: .appInfo) {
                CheckForUpdatesButton()
            }
            PageCommands(model: model)
        }

        // Message centre: its own small window, fronted rather than duplicated.
        Window(tr("msgCenter"), id: WindowID.messages) {
            InboxView()
                .environmentObject(model)
        }
        .windowResizability(.contentMinSize)
        .defaultSize(width: 360, height: 600)
        .defaultPosition(.trailing)

        MenuBarExtra {
            MenuBarContent()
                .environmentObject(model)
        } label: {
            // A plain image fed by TrayAnimator: animating inside the label
            // (TimelineView) re-lays out the status item in a loop and hangs.
            TrayLabel(model: model, animator: model.trayAnimator)
        }

        Settings {
            SettingsView()
                .environmentObject(model)
        }
    }
}

enum WindowID {
    static let main = "main"
    static let messages = "messages"
}

/// ⌘1/2/3 switch pages, ⌘R refreshes the node list.
struct PageCommands: Commands {
    @ObservedObject var model: AppModel

    var body: some Commands {
        CommandGroup(before: .toolbar) {
            ForEach(Array(MainTab.allCases.enumerated()), id: \.element) { index, tab in
                Button(tab.title) { model.tab = tab }
                    .keyboardShortcut(KeyEquivalent(Character(String(index + 1))))
                    .disabled(!model.isSignedIn)
            }
            Divider()
            Button(tr("refreshNodes")) { model.refreshProfile() }
                .keyboardShortcut("r")
                .disabled(!model.isSignedIn)
            Divider()
        }
    }
}

@MainActor
final class AppDelegate: NSObject, NSApplicationDelegate {
    private var observers: [NSObjectProtocol] = []
    private let pushAgent = PushAgentService()

    func applicationWillFinishLaunching(_ notification: Notification) {
        // The main window opens at every launch. Window restoration would
        // otherwise replay "closed" whenever PPVPN last quit from the menu bar.
        UserDefaults.standard.register(defaults: ["NSQuitAlwaysKeepsWindows": false])
        // Apply the saved appearance before any window exists, so nothing
        // (notably the toolbar's glass) starts in the system appearance.
        Appearance.current.apply()
        // Windows created later (settings, messages) take it on as they show.
        observers.append(NotificationCenter.default.addObserver(
            forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { note in
            let window = note.object as? NSWindow
            MainActor.assumeIsolated {
                guard let window, window.appearance != NSApp.appearance else { return }
                window.appearance = NSApp.appearance
            }
        })
    }

    func applicationDidFinishLaunching(_ notification: Notification) {
        // Show in the Dock only while a real window is open; with every window
        // closed PPVPN lives on in the menu bar.
        let center = NotificationCenter.default
        observers.append(center.addObserver(forName: NSWindow.willCloseNotification, object: nil, queue: .main) { _ in
            DispatchQueue.main.async { MainActor.assumeIsolated { Self.updateActivationPolicy() } }
        })
        observers.append(center.addObserver(forName: NSWindow.didBecomeKeyNotification, object: nil, queue: .main) { _ in
            MainActor.assumeIsolated { Self.updateActivationPolicy() }
        })
        #if DEBUG
        if ProcessInfo.processInfo.environment["PPVPN_PREVIEW"] != nil { return }
        #endif
        pushAgent.start()
    }

    func applicationShouldTerminateAfterLastWindowClosed(_ sender: NSApplication) -> Bool { false }

    /// ppvpn://push/<id> — sent by the push agent when a notification is
    /// clicked. Anything else is ignored.
    func application(_ application: NSApplication, open urls: [URL]) {
        for url in urls { PushLink(url)?.activate() }
    }

    private var isShuttingDown = false

    func applicationShouldTerminate(_ sender: NSApplication) -> NSApplication.TerminateReply {
        guard !isShuttingDown else { return .terminateLater }
        isShuttingDown = true
        Task {
            await AppModel.live.backend.shutdown()
            sender.reply(toApplicationShouldTerminate: true)
        }
        return .terminateLater
    }

    private static func updateActivationPolicy() {
        let hasWindow = NSApp.windows.contains { $0.isVisible && $0.styleMask.contains(.titled) }
        let policy: NSApplication.ActivationPolicy = hasWindow ? .regular : .accessory
        if NSApp.activationPolicy() != policy {
            NSApp.setActivationPolicy(policy)
        }
    }
}

enum Appearance: String, CaseIterable, Identifiable {
    static let storageKey = "appearance"
    case system, light, dark
    var id: Self { self }

    var title: String {
        switch self {
        case .system: tr("sys")
        case .light: tr("light")
        case .dark: tr("dark")
        }
    }

    static var current: Appearance {
        UserDefaults.standard.string(forKey: storageKey).flatMap(Appearance.init(rawValue:)) ?? .system
    }

    /// Sets the app appearance and pushes it onto every window explicitly:
    /// changing only NSApp.appearance at runtime can leave toolbar glass
    /// rendered for the previous appearance.
    @MainActor func apply() {
        let appearance: NSAppearance? = switch self {
        case .system: nil
        case .light: NSAppearance(named: .aqua)
        case .dark: NSAppearance(named: .darkAqua)
        }
        NSApp.appearance = appearance
        for window in NSApp.windows {
            window.appearance = appearance
            window.toolbar?.validateVisibleItems()
            window.contentView?.needsDisplay = true
        }
    }
}

extension AppModel {
    /// Process-wide model: the scenes and the app delegate share it.
    static let live = AppModel(backend: makeBackend(), networkPath: SystemNetworkPath())

    private static func makeBackend() -> ClientBackend {
        #if DEBUG
        switch ProcessInfo.processInfo.environment["PPVPN_PREVIEW"] {
        case "1": return PreviewBackend()
        case "signed-in": return PreviewBackend(signedIn: true)
        case "no-subscription": return PreviewBackend(signedIn: true, team: "t-acme")
        default: break
        }
        #endif
        return RustBackend(config: .current(), platform: MacPlatformHooks())
    }
}

extension ClientConfig {
    static func current(bundle: Bundle = .main) -> ClientConfig {
        let (support, logs) = AppDirectories.prepare()

        let override = UserDefaults.standard.string(forKey: AdvancedSettingsKeys.apiBase)?
            .trimmingCharacters(in: .whitespaces) ?? ""
        let apiBase = override.isEmpty
            ? bundle.object(forInfoDictionaryKey: "PPVPNAPIBaseDefault") as? String ?? "https://www.peakpassvpn.com"
            : override

        return ClientConfig(
            apiBase: apiBase,
            dataDir: support.path,
            logDir: logs.path,
            platform: "macos",
            appVersion: bundle.object(forInfoDictionaryKey: "CFBundleShortVersionString") as? String ?? "0")
    }
}


/// The menu bar label lives as long as the app, so it also opens the message
/// window on request (from a notification click) even with no window open.
private struct TrayLabel: View {
    @ObservedObject var model: AppModel
    @ObservedObject var animator: TrayAnimator
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Image(animator.imageName)
            .onChange(of: model.messagesWindowRequested) { requested in
                guard requested else { return }
                model.messagesWindowRequested = false
                NSApp.setActivationPolicy(.regular)
                NSApp.activate(ignoringOtherApps: true)
                openWindow(id: WindowID.messages)
                DispatchQueue.main.async { WindowDocker.shared.messagesShown() }
            }
    }
}

/// ppvpn://push/<id>, the push agent's click URL for a push without a link.
/// Only a numeric id is accepted: any page or app can open this scheme, so
/// nothing in the URL (such as a link) is trusted.
struct PushLink {
    let id: UInt64

    init?(_ url: URL) {
        guard url.scheme == "ppvpn", url.host == "push", url.query == nil, url.fragment == nil,
              url.pathComponents.count == 2,
              url.lastPathComponent.allSatisfy(\.isASCII), url.lastPathComponent.allSatisfy(\.isNumber),
              let id = UInt64(url.lastPathComponent) else { return nil }
        self.id = id
    }

    @MainActor func activate() {
        AppModel.live.activatePush(id: id)
    }
}
