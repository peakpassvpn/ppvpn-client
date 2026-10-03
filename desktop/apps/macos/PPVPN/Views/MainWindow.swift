import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Main window: unified toolbar (status line · page switcher · bell, account,
/// settings) over the login flow or the three pages.
struct MainWindow: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        content
            .frame(minWidth: 640, minHeight: 460)
            .background(Color(nsColor: .windowBackgroundColor))
            .background(DockableWindow(role: .main))
            .toolbar { toolbar }
            .modifier(DebugOpenMessages())
            .alert(model.presentedError?.title ?? "", isPresented: errorBinding, presenting: model.presentedError) { _ in
                Button(tr("ok")) {}
            } message: { alert in
                Text(alert.message)
            }
    }

    @ViewBuilder private var content: some View {
        switch model.snapshot.auth {
        case .restoring:
            ProgressView().frame(maxWidth: .infinity, maxHeight: .infinity)
        case .signedOut, .awaitingBrowser:
            LoginView()
        case .signedIn:
            switch model.tab {
            case .overview: OverviewView()
            case .nodes: NodesView()
            case .logs: LogsView()
            }
        }
    }

    @ToolbarContentBuilder private var toolbar: some ToolbarContent {
        titleItem
        if model.isSignedIn {
            ToolbarItem(placement: .principal) {
                Picker(tr("overview"), selection: $model.tab) {
                    ForEach(MainTab.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
            }
        }
        if model.isSignedIn {
            ToolbarItemGroup(placement: .primaryAction) {
                BellButton()
                AccountMenu()
                SettingsButton()
            }
        } else {
            #if compiler(>=6.2)
            trailingSpacer
            #endif
            ToolbarItem(placement: .primaryAction) { SettingsButton() }
        }
    }

    #if compiler(>=6.2)
    /// macOS 26 packs toolbar items to the leading edge when there is no
    /// principal item; a flexible spacer keeps the gear on the trailing side.
    /// (Only compiled with the macOS 26 SDK; CI's Xcode 16 has no ToolbarSpacer.)
    @ToolbarContentBuilder private var trailingSpacer: some ToolbarContent {
        if #available(macOS 26, *) {
            ToolbarSpacer(.flexible)
        }
    }
    #endif

    /// The status line is text, not a control: keep macOS 26 from giving it a
    /// glass button background. (Guarded so older SDKs, as on CI, still build.)
    @ToolbarContentBuilder private var titleItem: some ToolbarContent {
        #if compiler(>=6.2)
        if #available(macOS 26, *) {
            ToolbarItem(placement: .navigation) { TitleBlock() }
                .sharedBackgroundVisibility(.hidden)
        } else {
            ToolbarItem(placement: .navigation) { TitleBlock() }
        }
        #else
        ToolbarItem(placement: .navigation) { TitleBlock() }
        #endif
    }

    private var errorBinding: Binding<Bool> {
        Binding(get: { model.presentedError != nil }, set: { if !$0 { model.presentedError = nil } })
    }
}

/// "PPVPN" with one short state beneath ("已连接", with a small icon),
/// standing in for the window title so the icon can carry colour. Kept short
/// and truncated so the toolbar never pushes its buttons into overflow.
private struct TitleBlock: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        let presentation = model.presentation
        VStack(alignment: .leading, spacing: 1) {
            Text(tr("app")).font(.system(size: 13, weight: .semibold))
            HStack(spacing: 4) {
                Image(systemName: stateSymbol(presentation))
                    .font(.system(size: 9, weight: .semibold))
                    .foregroundStyle(dotColor(presentation))
                Text(presentation.statusLine)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .lineLimit(1)
                    .truncationMode(.tail)
            }
            .frame(maxWidth: 150, alignment: .leading)
            .opacity(presentation.statusLine.isEmpty ? 0 : 1)
        }
        .padding(.leading, 4)
        .fixedSize(horizontal: false, vertical: true)
        .accessibilityElement(children: .combine)
    }

    private func stateSymbol(_ presentation: ConnectionPresentation) -> String {
        guard model.isSignedIn, model.restriction == nil else { return "circle" }
        return switch presentation.tone {
        case .ok: "checkmark.circle.fill"
        case .busy: "arrow.triangle.2.circlepath"
        case .warn: "exclamationmark.triangle.fill"
        case .err: "exclamationmark.circle.fill"
        case .idle: "circle"
        }
    }

    private func dotColor(_ presentation: ConnectionPresentation) -> Color {
        guard model.isSignedIn, model.restriction == nil else { return Color.secondary.opacity(0.6) }
        return presentation.tone.color
    }
}

/// Bell with the server-side unread count; opens (or fronts) the message window.
private struct BellButton: View {
    @EnvironmentObject private var model: AppModel
    @Environment(\.openWindow) private var openWindow

    var body: some View {
        Button {
            openWindow(id: WindowID.messages)
            DispatchQueue.main.async { WindowDocker.shared.messagesShown() }
        } label: {
            Image(systemName: "bell")
                .overlay(alignment: .topTrailing) {
                    CountBadge(count: model.snapshot.unreadNotifications)
                        .scaleEffect(0.85)
                        .offset(x: 9, y: -8)
                }
        }
        .help(tr("messages"))
        .accessibilityLabel(model.snapshot.unreadNotifications > 0
            ? tr("unreadN", ["n": model.snapshot.unreadNotifications]) : tr("messages"))
    }
}

private struct SettingsButton: View {
    var body: some View {
        OpenSettingsButton {
            Image(systemName: "gearshape")
        }
        .help(tr("settings"))
    }
}

/// Opens the Settings scene: SettingsLink on macOS 14+, the legacy selector on 13.
struct OpenSettingsButton<Label: View>: View {
    var beforeOpening: () -> Void = {}
    @ViewBuilder var label: Label

    var body: some View {
        if #available(macOS 14, *) {
            SettingsLink { label }
                .simultaneousGesture(TapGesture().onEnded { beforeOpening() })
        } else {
            Button {
                beforeOpening()
                NSApp.activate(ignoringOtherApps: true)
                NSApp.sendAction(Selector(("showSettingsWindow:")), to: nil, from: nil)
            } label: { label }
        }
    }
}

/// Screenshot hooks (Debug): PPVPN_OPEN_MESSAGES=1 opens the message window;
/// PPVPN_OPEN_SETTINGS=general|account|advanced opens Settings on that tab.
private struct DebugOpenMessages: ViewModifier {
    @Environment(\.openWindow) private var openWindow

    func body(content: Content) -> some View {
        #if DEBUG
        content
            .task {
                guard ProcessInfo.processInfo.environment["PPVPN_OPEN_MESSAGES"] == "1" else { return }
                try? await Task.sleep(for: .seconds(1.5))
                openWindow(id: WindowID.messages)
            }
            .modifier(DebugOpenSettings())
        #else
        content
        #endif
    }
}

#if DEBUG
private struct DebugOpenSettings: ViewModifier {
    func body(content: Content) -> some View {
        if #available(macOS 14, *) {
            content.modifier(Opener())
        } else {
            content
        }
    }

    @available(macOS 14, *)
    private struct Opener: ViewModifier {
        @Environment(\.openSettings) private var openSettings

        func body(content: Content) -> some View {
            content.task {
                guard let tab = ProcessInfo.processInfo.environment["PPVPN_OPEN_SETTINGS"] else { return }
                UserDefaults.standard.set(tab, forKey: SettingsTab.storageKey)
                try? await Task.sleep(for: .seconds(1.5))
                openSettings()
            }
        }
    }
}
#endif
