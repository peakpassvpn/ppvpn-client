import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Menu bar extra (.menu): status header · system proxy / Enhanced Mode
/// checks · current node submenu · unread messages · app items.
struct MenuBarContent: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        let presentation = model.presentation
        if model.snapshot.auth == .restoring {
            // Restoring the saved sign-in: neither signed in nor out yet.
        } else if !model.isSignedIn {
            Text(tr("notSignedIn"))
        } else if let restriction = model.restriction {
            Label(restriction.title, systemImage: restriction.systemImage)
            Divider()
            messagesItem
        } else {
            Label {
                Text(presentation.headline)
            } icon: {
                Image(nsImage: dotImage(presentation.tone))
            }
            Text("\(model.selectedNode?.name ?? "—") · \(model.currentLatencyText)")
            Divider()
            Toggle(tr("connect"), isOn: Binding(
                get: { presentation.connectSwitch != .off },
                set: { toggleConnection(on: $0) }))
                .disabled(!presentation.connectSwitchEnabled)
            Menu(tr("currentNodeIs", ["n": model.selectedNode?.name ?? "—"])) {
                ForEach(model.nodes) { node in
                    Toggle(isOn: Binding(get: { node.id == model.snapshot.selectedNodeId },
                                         set: { if $0 { model.select(node) } })) {
                        Label {
                            Text(nodeTitle(node))
                        } icon: {
                            Image(nsImage: flagMenuImage(node.exitCountryCode))
                        }
                    }
                }
            }
            .disabled(model.nodes.isEmpty)
            Divider()
            messagesItem
        }

        Divider()
        Button(tr("openMain")) { showMainWindow() }
        OpenSettingsButton { Text(tr("settingsMenuMac")) }
            .keyboardShortcut(",")
        CheckForUpdatesButton()
        Divider()
        Button(tr("quit")) { NSApp.terminate(nil) }
            .keyboardShortcut("q")
    }

    /// 「N 条未读消息」 with a red dot, or 「没有未读消息」; opens the message window.
    @ViewBuilder private var messagesItem: some View {
        let unread = model.snapshot.unreadNotifications
        Button {
            NSApp.activate(ignoringOtherApps: true)
            openWindow(id: WindowID.messages)
            DispatchQueue.main.async { WindowDocker.shared.messagesShown() }
        } label: {
            if unread > 0 {
                Label {
                    Text(tr("unreadN", ["n": unread > 99 ? "99+" : String(unread)]))
                } icon: {
                    Image(nsImage: dotImage(.err))
                }
            } else {
                Text(tr("noUnread"))
            }
        }
    }

    private func nodeTitle(_ node: Node) -> String {
        if case .latency(let ms) = model.probes[node.id] { return "\(node.name)    \(ms) ms" }
        return node.name
    }

    /// Menus can't tint SF Symbols per item, so the status dot is drawn.
    private func dotImage(_ tone: Tone) -> NSImage {
        let color: NSColor = switch tone {
        case .ok: NSColor(named: "Success") ?? .systemGreen
        case .busy: .controlAccentColor
        case .warn: NSColor(named: "Warning") ?? .systemOrange
        case .err: NSColor(named: "Danger") ?? .systemRed
        case .idle: .secondaryLabelColor
        }
        let image = NSImage(size: NSSize(width: 8, height: 8), flipped: false) { rect in
            color.setFill()
            NSBezierPath(ovalIn: rect).fill()
            return true
        }
        image.isTemplate = false
        return image
    }

    /// Connecting in Enhanced Mode before the service exists needs the
    /// explanation first, which lives on the overview.
    private func toggleConnection(on: Bool) {
        guard on else {
            model.disconnect()
            return
        }
        Task {
            guard await model.connectUnlessInstallNeeded() else { return }
            model.tab = .overview
            model.installExplanationRequested = true
            showMainWindow()
        }
    }

    private func showMainWindow() {
        NSApp.setActivationPolicy(.regular)
        openWindow(id: WindowID.main)
        NSApp.activate(ignoringOtherApps: true)
    }
}
