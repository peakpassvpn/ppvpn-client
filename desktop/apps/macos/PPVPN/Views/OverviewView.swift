import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Overview: config warning · connection card · connection notices · live
/// traffic · local proxy. A restricted account replaces everything below the
/// warning with a full empty state.
struct OverviewView: View {
    @EnvironmentObject private var model: AppModel
    @State private var explainingInstall = false

    var body: some View {
        ScrollView {
            VStack(alignment: .leading, spacing: Metrics.sectionSpacing) {
                if case .invalid = model.snapshot.profileStatus, !model.nodes.isEmpty {
                    ConfigWarningBar()
                }
                if let restriction = model.restriction {
                    RestrictionCard(restriction: restriction)
                } else {
                    ConnectionCard(explainingInstall: $explainingInstall)
                    ForEach(model.presentation.notices) { NoticeCard(notice: $0) }
                    TrafficSection()
                    LocalProxySection()
                }
            }
            .padding(Metrics.contentPadding)
        }
        .onChange(of: model.installExplanationRequested) { requested in
            if requested {
                explainingInstall = true
                model.installExplanationRequested = false
            }
        }
        .onAppear {
            if model.installExplanationRequested {
                explainingInstall = true
                model.installExplanationRequested = false
            }
        }
        .alert(tr("installT"), isPresented: $explainingInstall) {
            Button(tr("installGo")) { model.connect() }
            Button(tr("cancel"), role: .cancel) {}
        } message: {
            Text(tr("installD"))
        }
    }
}

// MARK: - Config warning

/// Degraded, persistent: the newest profile failed to parse and the previous
/// one is still in use.
struct ConfigWarningBar: View {
    var body: some View {
        HStack(alignment: .firstTextBaseline, spacing: 8) {
            Image(systemName: "exclamationmark.triangle.fill").foregroundStyle(Brand.warning)
            Text(tr("cfgT")).fontWeight(.semibold)
            Text(tr("cfgD")).foregroundStyle(.secondary).lineLimit(2)
            Spacer(minLength: 0)
        }
        .font(.callout)
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: 8).fill(Brand.warning.opacity(0.13)))
    }
}

// MARK: - Connection card

private struct ConnectionCard: View {
    @EnvironmentObject private var model: AppModel
    @Binding var explainingInstall: Bool

    var body: some View {
        let presentation = model.presentation
        Card {
            header(presentation)
            if model.connectionMode == .enhanced, !model.snapshot.serviceInstalled {
                InstallHint { explainingInstall = true }
                    .padding(.horizontal, 14)
                    .padding(.bottom, 10)
            }
            Divider().padding(.horizontal, 14)
            CardRow(title: tr("currentNode")) {
                NodePicker()
            }
        }
    }

    /// Status and the single Connect switch in one block: glyph, title,
    /// node · detail, the connection method as the weakest line, switch.
    private func header(_ presentation: ConnectionPresentation) -> some View {
        HStack(spacing: 14) {
            StatusGlyph(systemImage: presentation.systemImage, tone: presentation.tone)
            VStack(alignment: .leading, spacing: 4) {
                Text(presentation.headline)
                    .font(.system(size: 20, weight: .bold))
                    .foregroundStyle(presentation.tone == .err ? Brand.danger : Color.primary)
                HStack(spacing: 6) {
                    if let node = model.selectedNode {
                        FlagView(countryCode: node.exitCountryCode)
                        Text(node.name).fontWeight(.semibold)
                        if !presentation.detail.isEmpty {
                            Text("· \(presentation.detail)").foregroundStyle(.secondary).lineLimit(1)
                        }
                    } else {
                        Text(presentation.detail).foregroundStyle(.secondary).lineLimit(1)
                    }
                }
                .font(.system(size: 13))
                Text(model.connectionMode.caption)
                    .font(.system(size: 11))
                    .foregroundStyle(.tertiary)
                    .lineLimit(1)
            }
            Spacer(minLength: 8)
            TriStateSwitch(value: presentation.connectSwitch,
                           isEnabled: presentation.connectSwitchEnabled,
                           action: toggleConnection)
                .accessibilityLabel(tr("connect"))
        }
        .padding(14)
    }

    /// Connected or connecting → disconnect; otherwise connect, explaining
    /// Enhanced Mode's one-time install first when the service is missing.
    private func toggleConnection() {
        if model.isConnectedOrConnecting {
            model.disconnect()
        } else {
            Task { if await model.connectUnlessInstallNeeded() { explainingInstall = true } }
        }
    }
}

/// Persistent to-do under the Enhanced Mode row while the service is missing.
struct InstallHint: View {
    let install: () -> Void

    var body: some View {
        HStack(spacing: 8) {
            Image(systemName: "lock.shield").foregroundStyle(Color.accentColor)
            Text(tr("needInstall")).font(.callout)
            Spacer(minLength: 8)
            Button(tr("installBtn"), action: install)
                .controlSize(.small)
        }
        .padding(.horizontal, 12)
        .padding(.vertical, 8)
        .background(RoundedRectangle(cornerRadius: 7).fill(Brand.accentSoft))
    }
}

/// Current node pop-up: flag, name and latency per entry.
struct NodePicker: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        Menu {
            ForEach(model.nodes) { node in
                Toggle(isOn: Binding(get: { node.id == model.snapshot.selectedNodeId },
                                     set: { if $0 { model.select(node) } })) {
                    Label {
                        Text(title(for: node))
                    } icon: {
                        Image(nsImage: flagMenuImage(node.exitCountryCode))
                    }
                }
            }
        } label: {
            Label {
                Text(model.selectedNode.map(title(for:)) ?? tr("x_noneSelected"))
            } icon: {
                Image(nsImage: flagMenuImage(model.selectedNode?.exitCountryCode))
            }
            .labelStyle(.titleAndIcon)
        }
        .fixedSize()
        .disabled(model.nodes.isEmpty)
    }

    private func title(for node: Node) -> String {
        if case .latency(let ms) = model.probes[node.id] {
            return "\(node.name)    \(ms) ms"
        }
        return node.name
    }
}

// MARK: - Notices

/// Persistent connection problem with its fix: retry, take over, retry proxy.
private struct NoticeCard: View {
    @EnvironmentObject private var model: AppModel
    let notice: Notice

    var body: some View {
        HStack(spacing: 10) {
            Image(systemName: notice.systemImage)
                .foregroundStyle(notice.tone.color)
                .font(.system(size: 15))
            VStack(alignment: .leading, spacing: 2) {
                Text(notice.title).fontWeight(.semibold)
                Text(notice.message).font(.callout).foregroundStyle(.secondary).lineLimit(2)
            }
            Spacer(minLength: 8)
            if notice.secondary == .useCompatible {
                Button(tr("useCompatible")) { model.setConnectionMode(.compatible) }
            }
            if let title = notice.actionTitle {
                Button(title, action: act)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .background(RoundedRectangle(cornerRadius: Metrics.cardRadius).fill(notice.tone.soft))
        .overlay(RoundedRectangle(cornerRadius: Metrics.cardRadius).strokeBorder(notice.tone.color.opacity(0.25), lineWidth: 0.5))
    }

    private func act() {
        switch notice.action {
        case .retry?: model.retry()
        case .takeOver?: model.takeOver()
        case .retryLocalProxy?: model.refreshProfile()
        case .backToAuto?: model.unpinCurrentNode()
        case .dismissClearedPins?: model.dismissClearedIngressPins()
        case nil: break
        }
    }
}

// MARK: - Traffic

private struct TrafficSection: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeader(title: tr("traffic"))
            Card {
                HStack(spacing: 0) {
                    cell(title: tr("upload"), image: "arrow.up", value: model.traffic.upBps.byteRate)
                    Divider().padding(.vertical, 10)
                    cell(title: tr("download"), image: "arrow.down", value: model.traffic.downBps.byteRate)
                }
            }
        }
    }

    private func cell(title: String, image: String, value: String) -> some View {
        VStack(alignment: .leading, spacing: 6) {
            Label(title, systemImage: image)
                .font(.system(size: 11))
                .foregroundStyle(.secondary)
            Text(value)
                .font(.system(size: 22, weight: .semibold).monospacedDigit())
        }
        .frame(maxWidth: .infinity, alignment: .leading)
        .padding(14)
    }
}

// MARK: - Local proxy

private struct LocalProxySection: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            SectionHeader(title: tr("localProxy"))
            if model.offersProxyScope {
                Picker(tr("localProxy"), selection: $model.proxyScope) {
                    ForEach(LocalProxyScope.allCases) { Text($0.title).tag($0) }
                }
                .pickerStyle(.segmented)
                .labelsHidden()
                .fixedSize()
                Text(model.proxyScope.detail)
                    .font(.system(size: 11))
                    .foregroundStyle(.secondary)
                    .padding(.leading, 2)
            }
            if let proxy = model.shownProxy {
                LocalProxyCard(proxy: proxy)
            } else {
                Card {
                    Text(model.localProxyUnavailableText ?? "")
                        .foregroundStyle(.secondary)
                        .frame(maxWidth: .infinity, minHeight: Metrics.rowMinHeight, alignment: .leading)
                        .padding(14)
                }
            }
            Text(model.proxyNote)
                .font(.system(size: 11))
                .foregroundStyle(.tertiary)
                .padding(.leading, 2)
        }
    }
}

/// HTTP / SOCKS5 address with copy (the copy carries the credentials), then
/// user name and masked password.
struct LocalProxyCard: View {
    let proxy: LocalProxy

    var body: some View {
        Card {
            row(label: "HTTP", value: proxy.httpAddress) { CopyButton(value: proxy.httpURL) }
            Divider().padding(.horizontal, 14)
            row(label: "SOCKS5", value: proxy.socksAddress) { CopyButton(value: proxy.socksURL) }
            ProxyCredentials(proxy: proxy) { label, value, trailing in
                VStack(spacing: 0) {
                    Divider().padding(.horizontal, 14)
                    row(label: label, value: value) { trailing }
                }
            }
        }
    }

    private func row<Trailing: View>(label: String, value: String, @ViewBuilder trailing: () -> Trailing) -> some View {
        HStack(spacing: 12) {
            Text(label)
                .foregroundStyle(.secondary)
                .frame(width: 64, alignment: .leading)
            Text(value)
                .font(.system(size: 12, design: .monospaced))
                .textSelection(.enabled)
                .lineLimit(1)
                .truncationMode(.middle)
                .help(value)
            Spacer(minLength: 8)
            trailing()
        }
        .frame(minHeight: Metrics.rowMinHeight)
        .padding(.horizontal, 14)
        .padding(.vertical, 5)
    }
}

// MARK: - Restricted

/// No subscription / expired / team disabled: one full empty state.
struct RestrictionCard: View {
    @EnvironmentObject private var model: AppModel
    let restriction: Restriction

    var body: some View {
        Card {
            EmptyStateView(systemImage: restriction.systemImage, title: restriction.title,
                           message: restriction.message) {
                if restriction.offersPurchase {
                    Button(tr("refresh")) { model.refreshProfile() }
                    Button {
                        model.openPurchasePage()
                    } label: {
                        Label(tr("buy"), systemImage: "arrow.up.forward")
                            .labelStyle(.titleAndIcon)
                    }
                    .buttonStyle(.borderedProminent)
                    .keyboardShortcut(.defaultAction)
                } else {
                    Menu(tr("switchTeam")) {
                        TeamItems()
                    }
                    .fixedSize()
                }
            }
            .padding(.vertical, 12)
        }
    }
}
