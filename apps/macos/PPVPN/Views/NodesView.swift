import PPVPNAppLogic
import PPVPNClient
import SwiftUI

/// Nodes: action bar (probe method · test all · refresh) over the node table,
/// with the selected node's local proxy and counts at the bottom.
struct NodesView: View {
    @EnvironmentObject private var model: AppModel
    @State private var selection: Node.ID?

    var body: some View {
        VStack(spacing: 0) {
            ActionBar()
            Divider()
            if let restriction = model.restriction {
                RestrictionCard(restriction: restriction)
                    .padding(Metrics.contentPadding)
                Spacer(minLength: 0)
            } else if model.nodes.isEmpty {
                emptyState
            } else {
                table
                Divider()
                SelectedNodeBar(node: selectedNode)
                Divider()
                Footer()
            }
        }
        .onAppear { selection = selection ?? model.snapshot.selectedNodeId }
    }

    private var selectedNode: Node? {
        model.nodes.first { $0.id == selection } ?? model.selectedNode
    }

    @ViewBuilder private var emptyState: some View {
        if case .invalid = model.snapshot.profileStatus {
            EmptyStateView(systemImage: "exclamationmark.triangle", tint: Brand.warning,
                           title: tr("invalidT"), message: tr("invalidD")) {
                Button(tr("refreshNodes")) { model.refreshProfile() }
            }
            .frame(maxHeight: .infinity)
        } else {
            EmptyStateView(systemImage: "globe", title: tr("loadingT"), message: tr("loadingD"), busy: true) {}
                .frame(maxHeight: .infinity)
        }
    }

    private var table: some View {
        Table(model.nodes, selection: $selection) {
            TableColumn("") { node in
                if node.id == model.snapshot.selectedNodeId {
                    Image(systemName: "checkmark").fontWeight(.semibold)
                }
            }
            .width(14)
            TableColumn(tr("colName")) { node in
                HStack(spacing: 7) {
                    FlagView(countryCode: node.exitCountryCode)
                    Text(node.name)
                        .fontWeight(node.id == model.snapshot.selectedNodeId ? .semibold : .regular)
                        .lineLimit(1)
                }
            }
            .width(min: 96, ideal: 130)
            TableColumn(tr("colTier")) { node in
                if let label = node.entryLabel, !label.isEmpty {
                    TierTag(text: label)
                }
            }
            .width(min: 40, ideal: 48)
            TableColumn(tr("colRegion")) { node in
                Text(node.exitRegion ?? "—").lineLimit(1)
            }
            .width(min: 48, ideal: 72)
            TableColumn(tr("colRoutes")) { node in
                if node.hasLineChoice {
                    LinePicker(node: node)
                } else {
                    Text(node.routesText)
                        .font(.system(size: 11, design: .monospaced))
                        .foregroundStyle(.secondary)
                        .lineLimit(1)
                        .truncationMode(.tail)
                }
            }
            .width(min: 80)
            TableColumn(tr("colLatency")) { node in
                LatencyLabel(outcome: model.probes[node.id])
                    .frame(maxWidth: .infinity, alignment: .trailing)
            }
            .width(64)
        }
        .tableStyle(.inset(alternatesRowBackgrounds: true))
        .contextMenu(forSelectionType: Node.ID.self) { ids in
            if let node = node(for: ids) {
                Button(tr("setCurrent")) { model.select(node) }
                    .disabled(node.id == model.snapshot.selectedNodeId)
                Button(tr("testOne")) { model.probe(node) }
                if node.hasLineChoice {
                    Picker(tr("colRoutes"), selection: lineBinding(node)) {
                        lineOptions(node)
                    }
                }
                if model.standardError == nil, let proxy = model.proxies[node.id] {
                    Divider()
                    Button(tr("copyHttp")) { copyToPasteboard(proxy.httpURL) }
                    Button(tr("copySocks")) { copyToPasteboard(proxy.socksURL) }
                }
            }
        } primaryAction: { ids in
            if let node = node(for: ids) { model.select(node) }
        }
    }

    private func lineBinding(_ node: Node) -> Binding<String?> {
        Binding(get: { model.pinnedEndpointKey(of: node) },
                set: { model.pinIngress(node, endpointKey: $0) })
    }

    private func node(for ids: Set<Node.ID>) -> Node? {
        guard let id = ids.first else { return nil }
        return model.nodes.first { $0.id == id }
    }
}

/// 自动 then each line in failover order, tagged by endpoint key (nil: automatic).
@ViewBuilder
private func lineOptions(_ node: Node) -> some View {
    Text(tr("lineAuto")).tag(String?.none)
    ForEach(node.orderedReplicas, id: \.endpointKey) { replica in
        Text(node.lineName(replica.endpointKey) ?? "").tag(Optional(replica.endpointKey))
    }
}

/// A multi-line node's line: automatic failover, or one pinned line. A
/// pinned line the core reports down keeps its pin and shows a warning.
private struct LinePicker: View {
    @EnvironmentObject private var model: AppModel
    let node: Node

    var body: some View {
        HStack(spacing: 4) {
            Picker(tr("colRoutes"), selection: Binding(
                get: { model.pinnedEndpointKey(of: node) },
                set: { model.pinIngress(node, endpointKey: $0) })) {
                lineOptions(node)
            }
            .labelsHidden()
            .pickerStyle(.menu)
            .controlSize(.small)
            .fixedSize()
            if pinnedDown {
                Image(systemName: "exclamationmark.triangle.fill")
                    .foregroundStyle(Brand.warning)
                    .help(tr("ingressDownT"))
            }
        }
        .help(node.routesText)
    }

    private var pinnedDown: Bool {
        guard let pinned = model.pinnedEndpointKey(of: node) else { return false }
        return model.snapshot.nodeIngresses.first { $0.nodeId == node.id }?.ingresses
            .contains { $0.endpointKey == pinned && $0.healthy == false } ?? false
    }
}

/// Content-area action bar (40 pt), kept out of the window toolbar.
private struct ActionBar: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            Text(tr("probe")).foregroundStyle(.secondary)
            Picker(tr("probe"), selection: $model.probeMethod) {
                Text("Ping").tag(ProbeMethod.icmp)
                Text("TCP").tag(ProbeMethod.tcp)
                Text("Connect").tag(ProbeMethod.connect)
            }
            .pickerStyle(.segmented)
            .labelsHidden()
            .fixedSize()
            Spacer(minLength: 8)
            Button {
                model.probeAll()
            } label: {
                Label(tr("testAll"), systemImage: "speedometer")
            }
            .disabled(model.nodes.isEmpty || model.restriction != nil)
            Button {
                model.refreshProfile()
            } label: {
                Label(tr("refreshNodes"), systemImage: "arrow.clockwise")
            }
        }
        .controlSize(.small)
        .padding(.horizontal, 14)
        .frame(height: 40)
    }
}

/// Soft accent tag for the entry tier (专线 / 优化).
struct TierTag: View {
    let text: String

    var body: some View {
        Text(text)
            .font(.system(size: 10.5, weight: .medium))
            .foregroundStyle(Color.accentColor)
            .padding(.horizontal, 6)
            .padding(.vertical, 1)
            .background(RoundedRectangle(cornerRadius: 4).fill(Brand.accentSoft))
    }
}

/// The selected row's local proxy: "香港 01 的本地代理", user, HTTP / SOCKS5 copy.
private struct SelectedNodeBar: View {
    @EnvironmentObject private var model: AppModel
    let node: Node?

    var body: some View {
        VStack(alignment: .leading, spacing: 8) {
            HStack(spacing: 8) {
                FlagView(countryCode: node?.exitCountryCode)
                Text(tr("nodeProxyT", ["n": node?.name ?? "—"])).fontWeight(.semibold)
            }
            if let proxy {
                HStack(spacing: 16) {
                    address(label: "HTTP", value: "\(proxy.host):\(proxy.port)", copy: proxy.httpURL)
                    Divider().frame(height: 14)
                    address(label: "SOCKS5", value: "\(proxy.host):\(proxy.port)", copy: proxy.socksURL)
                    Spacer(minLength: 0)
                }
                ProxyCredentials(proxy: proxy) { label, value, trailing in
                    HStack(spacing: 8) {
                        Text(label).foregroundStyle(.secondary)
                        Text(value)
                            .font(.system(size: 12, design: .monospaced))
                            .textSelection(.enabled)
                            .lineLimit(1)
                            .truncationMode(.middle)
                            .help(value)
                        trailing
                        Spacer(minLength: 0)
                    }
                    .font(.callout)
                }
            } else {
                Text(tr("x_proxyUnavailable")).font(.callout).foregroundStyle(.secondary)
            }
        }
        .padding(.horizontal, 14)
        .padding(.vertical, 10)
        .frame(maxWidth: .infinity, alignment: .leading)
    }

    private var proxy: LocalProxy? { model.standardError == nil ? node.flatMap { model.proxies[$0.id] } : nil }

    private func address(label: String, value: String, copy: String) -> some View {
        HStack(spacing: 8) {
            Text(label).foregroundStyle(.secondary)
            Text(value).font(.system(size: 12, design: .monospaced)).textSelection(.enabled)
            CopyButton(value: copy)
        }
        .font(.callout)
    }
}

private struct Footer: View {
    @EnvironmentObject private var model: AppModel

    var body: some View {
        HStack(spacing: 10) {
            Text(tr("nodeCount", ["n": model.nodes.count]))
            if model.probes.values.contains(.running) {
                HStack(spacing: 5) {
                    ProgressView().controlSize(.mini)
                    Text(tr("testing"))
                }
            }
            Spacer(minLength: 8)
            Text(tr("nodesHint"))
        }
        .font(.system(size: 11))
        .foregroundStyle(.secondary)
        .padding(.horizontal, 14)
        .frame(height: 26)
    }
}
