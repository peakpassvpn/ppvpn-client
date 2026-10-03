import AppKit
import SwiftUI

/// Docks the message centre beside the main window: opened next to it
/// (right side, or left when the screen has no room), top edges aligned, and
/// attached as a child window so it follows the main window. Dragging it away
/// undocks it; releasing it within `snapDistance` of either side docks it
/// again. The choice is remembered. Closing the main window leaves the
/// message window where it is, as the design asks.
@MainActor
final class WindowDocker {
    static let shared = WindowDocker()

    enum Role { case main, messages }

    private weak var main: NSWindow?
    private weak var messages: NSWindow?
    private var observers: [NSObjectProtocol] = []
    private var dragging = false

    private static let gap: CGFloat = 8
    private static let snapDistance: CGFloat = 24
    private static let dockedKey = "messagesDocked"

    private var prefersDocked: Bool {
        get { UserDefaults.standard.object(forKey: Self.dockedKey) as? Bool ?? true }
        set { UserDefaults.standard.set(newValue, forKey: Self.dockedKey) }
    }

    private var isDocked: Bool {
        guard let main, let messages else { return false }
        return messages.parent === main
    }

    func attach(_ window: NSWindow, as role: Role) {
        switch role {
        case .main:
            guard main !== window else { return }
            main = window
            observe(window, NSWindow.didResizeNotification) { $0.realign() }
            observe(window, NSWindow.willCloseNotification) { $0.detach() }
            observe(window, NSWindow.didBecomeMainNotification) { docker in
                if docker.prefersDocked, docker.messages?.isVisible == true, !docker.isDocked { docker.dock() }
            }
        case .messages:
            guard messages !== window else { return }
            messages = window
            observe(window, NSWindow.willMoveNotification) { $0.beginDrag() }
            observe(window, NSWindow.didMoveNotification) { $0.dragMoved() }
            observe(window, NSWindow.willCloseNotification) { $0.detach() }
            if prefersDocked { dock() }
        }
    }

    /// Called when the message window is (re)opened from the bell or menu.
    func messagesShown() {
        if prefersDocked, !isDocked { dock() }
    }

    private func observe(_ window: NSWindow, _ name: Notification.Name,
                         _ handler: @escaping @MainActor @Sendable (WindowDocker) -> Void) {
        observers.append(NotificationCenter.default.addObserver(forName: name, object: window, queue: .main) { [weak self] _ in
            MainActor.assumeIsolated { if let self { handler(self) } }
        })
    }

    // MARK: Docking

    private func dock() {
        guard let main, let messages, main.isVisible, messages.isVisible else { return }
        messages.setFrameOrigin(dockedOrigin(main: main, messages: messages, side: preferredSide(main: main, messages: messages)))
        if messages.parent !== main {
            messages.parent?.removeChildWindow(messages)
            main.addChildWindow(messages, ordered: .above)
        }
    }

    private func detach() {
        guard let messages, let parent = messages.parent else { return }
        parent.removeChildWindow(messages)
    }

    /// Keeps a docked window flush with the main window's edge after a resize.
    private func realign() {
        guard isDocked, let main, let messages else { return }
        let side: Side = messages.frame.midX >= main.frame.midX ? .right : .left
        messages.setFrameOrigin(dockedOrigin(main: main, messages: messages, side: side))
    }

    private enum Side { case left, right }

    private func preferredSide(main: NSWindow, messages: NSWindow) -> Side {
        let screen = (main.screen ?? NSScreen.main)?.visibleFrame ?? .infinite
        return main.frame.maxX + Self.gap + messages.frame.width <= screen.maxX ? .right : .left
    }

    private func dockedOrigin(main: NSWindow, messages: NSWindow, side: Side) -> NSPoint {
        let x = side == .right
            ? main.frame.maxX + Self.gap
            : main.frame.minX - Self.gap - messages.frame.width
        return NSPoint(x: x, y: main.frame.maxY - messages.frame.height)
    }

    // MARK: Dragging

    /// willMove is posted for user drags only; a docked window leaves its
    /// parent so it moves on its own.
    private func beginDrag() {
        dragging = true
        detach()
    }

    private func dragMoved() {
        guard dragging else { return }
        waitForMouseUp()
    }

    private func waitForMouseUp() {
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.08) { [weak self] in
            guard let self, self.dragging else { return }
            if NSEvent.pressedMouseButtons != 0 {
                self.waitForMouseUp()
            } else {
                self.dragging = false
                self.snapIfClose()
            }
        }
    }

    private func snapIfClose() {
        guard let main, let messages, main.isVisible else { return }
        let m = main.frame, w = messages.frame
        let overlapsVertically = w.maxY > m.minY && w.minY < m.maxY
        let nearRight = abs(w.minX - (m.maxX + Self.gap)) <= Self.snapDistance
        let nearLeft = abs(w.maxX - (m.minX - Self.gap)) <= Self.snapDistance
        guard overlapsVertically, nearRight || nearLeft else {
            prefersDocked = false
            return
        }
        prefersDocked = true
        messages.setFrameOrigin(dockedOrigin(main: main, messages: messages, side: nearRight ? .right : .left))
        main.addChildWindow(messages, ordered: .above)
    }
}

/// Hands the hosting NSWindow to the docker once the view is in a window.
struct DockableWindow: NSViewRepresentable {
    let role: WindowDocker.Role

    func makeNSView(context: Context) -> NSView {
        let view = WindowReporter()
        view.onWindow = { window in WindowDocker.shared.attach(window, as: role) }
        return view
    }

    func updateNSView(_ nsView: NSView, context: Context) {}

    private final class WindowReporter: NSView {
        var onWindow: ((NSWindow) -> Void)?

        override func viewDidMoveToWindow() {
            super.viewDidMoveToWindow()
            guard let window else { return }
            DispatchQueue.main.async { [weak self] in
                MainActor.assumeIsolated { self?.onWindow?(window) }
            }
        }
    }
}
