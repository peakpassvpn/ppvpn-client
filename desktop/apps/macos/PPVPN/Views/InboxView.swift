import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Message centre window: paged list with a detail view switched in place.
/// The list stays mounted underneath the detail so returning keeps its scroll
/// position.
struct InboxView: View {
    @EnvironmentObject private var model: AppModel
    @State private var openID: UInt64?
    /// The message in the detail as last seen (it may drop out of the list
    /// on reload, or be beyond the loaded pages), or a push shown read-only.
    @State private var requested: InboxMessage?
    @State private var requestedIsPush = false

    var body: some View {
        ZStack {
            MessageList(open: open)
                .opacity(openID == nil ? 1 : 0)
                .allowsHitTesting(openID == nil)
            if let message = openMessage {
                MessageDetail(message: message, position: position(of: message),
                              isPush: isPushOpen,
                              previous: neighbour(-1), next: neighbour(1),
                              back: close, go: open)
                    .background(Color(nsColor: .windowBackgroundColor))
            }
        }
        .frame(minWidth: 340, idealWidth: 360, minHeight: 400, idealHeight: 600)
        .background(DockableWindow(role: .messages))
        .onAppear {
            model.reloadInbox()
            showRequested()
        }
        .onChange(of: model.detailRequest) { _ in showRequested() }
        .onChange(of: model.isSignedIn) { signedIn in if !signedIn { close() } }
    }

    private var isPushOpen: Bool { requestedIsPush && requested?.id == openID && openID != nil }

    private var openMessage: InboxMessage? {
        guard let id = openID else { return nil }
        if isPushOpen { return requested }
        return model.inbox.first { $0.id == id } ?? requested.flatMap { $0.id == id ? $0 : nil }
    }

    /// Shows what a notification click asked for. The model has already
    /// opened (and marked) it; this only displays it.
    private func showRequested() {
        guard let request = model.detailRequest else { return }
        model.detailRequest = nil
        leavePush()
        switch request {
        case .message(let message):
            requested = message
            requestedIsPush = false
        case .push(let message):
            requested = message
            requestedIsPush = true
        }
        openID = requested?.id
    }

    /// Entering the detail marks the message read. The message is kept, so
    /// the detail survives a reload that no longer lists it (new messages
    /// reload only the first page).
    private func open(_ message: InboxMessage) {
        leavePush()
        requested = message
        openID = message.id
        model.markRead(message)
    }

    private func close() {
        leavePush()
        requested = nil
        openID = nil
    }

    private func leavePush() {
        if isPushOpen { model.pushDetailClosed() }
        requestedIsPush = false
    }

    private func position(of message: InboxMessage) -> (index: Int, total: Int) {
        let index = model.inbox.firstIndex { $0.id == message.id } ?? 0
        return (index + 1, max(Int(model.inboxTotal), model.inbox.count))
    }

    private func neighbour(_ offset: Int) -> InboxMessage? {
        guard !isPushOpen, let id = openID, let index = model.inbox.firstIndex(where: { $0.id == id }) else { return nil }
        let target = index + offset
        return model.inbox.indices.contains(target) ? model.inbox[target] : nil
    }
}

// MARK: - Message kind

extension InboxMessage {
    /// critical = danger, important = warning, others carry no colour.
    var severityColor: Color? {
        switch severity {
        case .critical: Brand.danger
        case .important: Brand.warning
        case .normal, .unspecified: nil
        }
    }
}

/// 32 pt round type icon, tinted for critical / important.
private struct KindIcon: View {
    let message: InboxMessage

    var body: some View {
        let tint = message.severityColor
        Image(systemName: message.kindSymbol)
            .font(.system(size: 14, weight: .medium))
            .foregroundStyle(tint ?? .secondary)
            .frame(width: 32, height: 32)
            .background(Circle().fill(tint.map { $0.opacity(0.14) } ?? Color.primary.opacity(0.06)))
    }
}

// MARK: - List

private struct MessageList: View {
    @EnvironmentObject private var model: AppModel
    let open: (InboxMessage) -> Void

    var body: some View {
        VStack(spacing: 0) {
            header
            Divider()
            if model.inboxFailed {
                ErrorBanner { model.reloadInbox() }
            }
            content
        }
    }

    private var header: some View {
        HStack {
            let unread = model.snapshot.unreadNotifications
            Text(unread > 0 ? tr("unreadShort", ["n": unread > 99 ? "99+" : String(unread)]) : tr("allRead"))
                .foregroundStyle(.secondary)
            Spacer()
            Button {
                model.markAllRead()
            } label: {
                Label(tr("markAllRead"), systemImage: "checkmark.circle")
            }
            .controlSize(.small)
            .disabled(model.snapshot.unreadNotifications == 0 && model.inbox.allSatisfy(\.read))
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
    }

    @ViewBuilder private var content: some View {
        if !model.inboxLoaded && model.inbox.isEmpty && !model.inboxFailed {
            SkeletonList()
        } else if model.inbox.isEmpty && !model.inboxFailed {
            EmptyStateView(systemImage: "tray", title: tr("msgEmptyT"), message: tr("msgEmptyD")) {}
                .frame(maxHeight: .infinity)
        } else {
            ScrollView {
                LazyVStack(spacing: 0) {
                    ForEach(model.inbox, id: \.id) { message in
                        MessageRow(message: message, open: { open(message) })
                            .onAppear {
                                if message.id == model.inbox.last?.id { model.loadMoreInbox() }
                            }
                        Divider().padding(.leading, 12)
                    }
                    footer
                }
            }
        }
    }

    @ViewBuilder private var footer: some View {
        HStack(spacing: 6) {
            if model.inboxLoading {
                ProgressView().controlSize(.small)
                Text(tr("loadingMore"))
            } else if !model.inboxHasMore && !model.inbox.isEmpty {
                Text(tr("noMore"))
            }
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .frame(height: 36)
    }
}

/// Red persistent banner above the (kept) list after a failed load.
private struct ErrorBanner: View {
    let retry: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "exclamationmark.circle.fill").foregroundStyle(Brand.danger)
            VStack(alignment: .leading, spacing: 1) {
                Text(tr("msgErrT")).fontWeight(.semibold)
                Text(tr("msgErrD")).font(.system(size: 11)).foregroundStyle(.secondary)
            }
            Spacer(minLength: 6)
            Button(tr("retry"), action: retry).controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(Brand.danger.opacity(0.10))
    }
}

private struct MessageRow: View {
    @EnvironmentObject private var model: AppModel
    let message: InboxMessage
    let open: () -> Void
    @State private var hovering = false

    var body: some View {
        HStack(alignment: .top, spacing: 10) {
            Circle()
                .fill(Color.accentColor)
                .frame(width: 8, height: 8)
                .opacity(message.read ? 0 : 1)
                .padding(.top, 12)
            KindIcon(message: message)
            VStack(alignment: .leading, spacing: 3) {
                HStack(alignment: .firstTextBaseline, spacing: 6) {
                    Text(message.title)
                        .fontWeight(message.read ? .regular : .bold)
                        .lineLimit(1)
                    Spacer(minLength: 4)
                    if let date = message.createdDate {
                        Text(RelativeTime.format(date))
                            .font(.system(size: 11))
                            .foregroundStyle(.secondary)
                            .fixedSize()
                    }
                }
                kindLine
                Text(message.content)
                    .font(.callout)
                    .foregroundStyle(.secondary)
                    .lineLimit(2)
                HStack {
                    if let link = message.link {
                        Button {
                            NSWorkspace.shared.open(link)
                            model.markRead(message)
                        } label: {
                            Label(tr("viewDetails"), systemImage: "arrow.up.forward")
                        }
                        .controlSize(.small)
                    }
                    Spacer(minLength: 0)
                    if hovering && !message.read {
                        Button {
                            model.markRead(message)
                        } label: {
                            Label(tr("markRead"), systemImage: "checkmark")
                        }
                        .buttonStyle(.borderless)
                        .controlSize(.small)
                    }
                }
                .frame(minHeight: message.link != nil || (hovering && !message.read) ? 22 : 0)
            }
            Image(systemName: "chevron.right")
                .font(.system(size: 11, weight: .semibold))
                .foregroundStyle(.tertiary)
                .padding(.top, 10)
        }
        .padding(.leading, 12)
        .padding(.trailing, 10)
        .padding(.vertical, 10)
        .overlay(alignment: .leading) {
            if let color = message.severityColor {
                Rectangle().fill(color).frame(width: 3)
            }
        }
        .background(hovering ? Color.primary.opacity(0.04) : .clear)
        .contentShape(Rectangle())
        .onHover { hovering = $0 }
        .onTapGesture(perform: open)
        .contextMenu {
            Button(tr("markRead")) { model.markRead(message) }
                .disabled(message.read)
            if let link = message.link {
                Button(tr("viewDetails")) {
                    NSWorkspace.shared.open(link)
                    model.markRead(message)
                }
            }
        }
    }

    /// 「订阅即将到期 · 紧急」: type and level both in words, not colour alone.
    private var kindLine: some View {
        HStack(spacing: 4) {
            Text(message.kindTitle).foregroundStyle(.secondary)
            if let severity = message.severityTitle {
                Text("·").foregroundStyle(.tertiary)
                Text(severity).fontWeight(.semibold).foregroundStyle(message.severityColor ?? .secondary)
            }
        }
        .font(.system(size: 11))
    }
}

/// Six breathing placeholder rows for the first load.
private struct SkeletonList: View {
    @State private var dim = false

    var body: some View {
        VStack(spacing: 0) {
            ForEach(0..<6, id: \.self) { _ in
                HStack(alignment: .top, spacing: 10) {
                    Circle().frame(width: 32, height: 32)
                    VStack(alignment: .leading, spacing: 7) {
                        RoundedRectangle(cornerRadius: 3).frame(width: 170, height: 10)
                        RoundedRectangle(cornerRadius: 3).frame(width: 110, height: 8)
                        RoundedRectangle(cornerRadius: 3).frame(height: 8)
                    }
                }
                .foregroundStyle(Color.primary.opacity(0.08))
                .padding(.horizontal, 30)
                .padding(.vertical, 12)
            }
            Spacer(minLength: 0)
        }
        .opacity(dim ? 0.45 : 1)
        .animation(.easeInOut(duration: 1.4).repeatForever(autoreverses: true), value: dim)
        .onAppear { dim = true }
    }
}

// MARK: - Detail

private struct MessageDetail: View {
    @EnvironmentObject private var model: AppModel
    let message: InboxMessage
    let position: (index: Int, total: Int)
    /// A push shown from a notification click: read-only, outside the list.
    let isPush: Bool
    let previous: InboxMessage?
    let next: InboxMessage?
    let back: () -> Void
    let go: (InboxMessage) -> Void

    var body: some View {
        VStack(spacing: 0) {
            topBar
            Divider()
            ScrollView {
                VStack(alignment: .leading, spacing: 14) {
                    header
                    Divider()
                    Text(message.content)
                        .lineSpacing(6)
                        .textSelection(.enabled)
                        .fixedSize(horizontal: false, vertical: true)
                    if let link = message.link {
                        VStack(alignment: .leading, spacing: 4) {
                            Button {
                                NSWorkspace.shared.open(link)
                                if !isPush { model.markRead(message) }
                            } label: {
                                Label(tr("viewDetails"), systemImage: "arrow.up.forward")
                            }
                            .buttonStyle(.borderedProminent)
                            Text(tr("openLinkHint")).font(.system(size: 11)).foregroundStyle(.tertiary)
                        }
                    }
                }
                .frame(maxWidth: .infinity, alignment: .leading)
                .padding(16)
            }
            if !isPush {
                Divider()
                footer
            }
        }
        .onExitCommand(perform: back)
    }

    private var footer: some View {
        HStack {
            Button {
                model.markUnread(message)
                back()
            } label: {
                Label(tr("markUnread"), systemImage: "envelope.badge")
            }
            .controlSize(.small)
            .disabled(!message.read)
            Spacer()
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
    }

    private var topBar: some View {
        HStack(spacing: 10) {
            Button(action: back) {
                Label(tr("msgCenter"), systemImage: "chevron.left")
            }
            .buttonStyle(.borderless)
            .keyboardShortcut(.cancelAction)
            .help(tr("backToList"))
            Spacer()
            if !isPush {
                Text(tr("x_msgPosition", ["i": position.index, "n": position.total]))
                    .font(.system(size: 11).monospacedDigit())
                    .foregroundStyle(.secondary)
            }
            Button { previous.map(go) } label: { Image(systemName: "chevron.up") }
                .buttonStyle(.borderless)
                .disabled(previous == nil)
                .help(tr("prevMsg"))
            Button { next.map(go) } label: { Image(systemName: "chevron.down") }
                .buttonStyle(.borderless)
                .disabled(next == nil)
                .help(tr("nextMsg"))
        }
        .padding(.horizontal, 12)
        .frame(height: 40)
    }

    private var header: some View {
        HStack(alignment: .top, spacing: 12) {
            if let color = message.severityColor {
                Rectangle().fill(color).frame(width: 3)
            }
            VStack(alignment: .leading, spacing: 8) {
                HStack(spacing: 8) {
                    KindIcon(message: message)
                    Text(message.kindTitle).font(.system(size: 12)).foregroundStyle(.secondary)
                    if let severity = message.severityTitle, let color = message.severityColor {
                        Text(severity)
                            .font(.system(size: 11, weight: .semibold))
                            .foregroundStyle(color)
                            .padding(.horizontal, 5)
                            .padding(.vertical, 1)
                            .overlay(RoundedRectangle(cornerRadius: 4).strokeBorder(color, lineWidth: 1))
                    }
                }
                Text(message.title)
                    .font(.system(size: 17, weight: .semibold))
                    .fixedSize(horizontal: false, vertical: true)
                if let date = message.createdDate {
                    Text(RelativeTime.full(date))
                        .font(.system(size: 11))
                        .foregroundStyle(.secondary)
                }
            }
        }
        .fixedSize(horizontal: false, vertical: true)
    }
}
