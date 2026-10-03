import AppKit
import Combine
import Foundation
import PPVPNAppLogic
import PPVPNClient

/// The app's `AppState`: publishes its changes to SwiftUI and does the AppKit
/// side of its hooks.
@MainActor
final class AppModel: AppState, ObservableObject {
    // Only sent from the main actor (`willChange()`); SwiftUI subscribes there too.
    nonisolated(unsafe) let objectWillChange = ObservableObjectPublisher()
    /// Drives the tray icon's breathing; separate so its ticks only redraw
    /// the menu bar label, not every view observing the model.
    let trayAnimator = TrayAnimator()

    init(backend: ClientBackend, networkPath: NetworkPathSource? = nil) {
        super.init(backend: backend, networkPath: networkPath)
        #if DEBUG
        // Automation hook for screenshots: PPVPN_TAB=overview|nodes|logs.
        if let tab = ProcessInfo.processInfo.environment["PPVPN_TAB"].flatMap(MainTab.init(rawValue:)) {
            self.tab = tab
        }
        #endif
    }

    override func willChange() { objectWillChange.send() }

    override func open(_ url: URL) { NSWorkspace.shared.open(url) }

    override func didUpdate(from previous: ClientSnapshot) {
        if snapshot.unreadNotifications != previous.unreadNotifications {
            NSApp.dockTile.badgeLabel = snapshot.unreadNotifications > 0 ? String(snapshot.unreadNotifications) : nil
        }
        #if DEBUG
        // Automation hook: start device login once when launched signed out.
        if snapshot.auth == .signedOut, previous.auth == .restoring,
           ProcessInfo.processInfo.environment["PPVPN_AUTO_SIGN_IN"] == "1" {
            signIn()
        }
        #endif
        trayAnimator.state = presentation.tray
    }
}

/// Menu bar icon state with the two-frame "connecting" breathing (0.7 s).
@MainActor
final class TrayAnimator: ObservableObject {
    @Published private(set) var imageName = TrayState.off.imageName
    private var breathTask: Task<Void, Never>?

    public var state = TrayState.off {
        didSet {
            guard state != oldValue else { return }
            imageName = state.imageName
            breathTask?.cancel()
            breathTask = nil
            guard state == .busy else { return }
            breathTask = Task { [weak self] in
                var dim = false
                while !Task.isCancelled {
                    try? await Task.sleep(for: .milliseconds(700))
                    guard !Task.isCancelled else { return }
                    dim.toggle()
                    self?.imageName = dim ? TrayState.off.imageName : TrayState.busy.imageName
                }
            }
        }
    }
}
