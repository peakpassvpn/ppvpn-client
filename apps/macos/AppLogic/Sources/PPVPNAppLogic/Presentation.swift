import Foundation
import PPVPNClient

// Derived UI state, ported from the design's reference `vals()`
// (prototype/ppvpn-core.js) and adapted to the single Connect switch: the
// client reports one ConnectionState for whichever ConnectionMode is chosen.
// Views read these instead of re-deriving.

extension ConnectionMode: @retroactive CaseIterable, @retroactive Identifiable {
    public static var allCases: [ConnectionMode] { [.enhanced, .compatible] }
    public var id: Self { self }

    /// Settings radio title and description.
    public var title: String { self == .enhanced ? tr("methodEnhanced") : tr("methodCompatible") }
    public var detail: String { self == .enhanced ? tr("methodEnhancedD") : tr("methodCompatibleD") }
    /// The status header's weakest line (ConnectionMethodCaption).
    public var caption: String { self == .enhanced ? tr("captionEnhanced") : tr("captionCompatible") }
}

extension RoutingMode: @retroactive CaseIterable, @retroactive Identifiable {
    public static var allCases: [RoutingMode] { [.rules, .global] }
    public var id: Self { self }

    /// Settings radio title and description.
    public var title: String { self == .rules ? tr("routingRules") : tr("routingGlobal") }
    public var detail: String { self == .rules ? tr("routingRulesD") : tr("routingGlobalD") }
}

/// Semantic tone shared by the connection card, status dot and notices.
public enum Tone: Equatable, Sendable {
    case ok, busy, warn, err, idle
}

/// Position of a three-state switch: off, on, or moving between them.
public enum SwitchState: Equatable, Sendable { case off, on, pending }

public enum TrayState {
    case off, busy, on, error

    public var imageName: String {
        switch self {
        case .off: "MenuBar-disconnected"
        case .busy: "MenuBar-connecting"
        case .on: "MenuBar-connected"
        case .error: "MenuBar-error"
        }
    }
}

public struct Restriction {
    public let code: ErrorCode
    public let title: String
    public let message: String
    public let systemImage: String
    public let offersPurchase: Bool
}

public struct Notice: Identifiable {
    public enum Action { case retry, takeOver, retryLocalProxy, backToAuto, dismissClearedPins, dismissProxyReset }
    public enum Secondary { case useCompatible }

    public let id: String
    public let tone: Tone
    public let systemImage: String
    public let title: String
    public let message: String
    public let actionTitle: String?
    public let action: Action?
    public var secondary: Secondary?
}

public struct ConnectionPresentation {
    public var tone: Tone
    public var headline: String
    /// Second header line after the flag and node name, e.g. "HKG-A · 38 ms".
    public var detail: String
    public var systemImage: String
    public var statusLine: String
    public var tray: TrayState
    /// The single Connect switch.
    public var connectSwitch: SwitchState
    public var connectSwitchEnabled: Bool
    public var notices: [Notice]
}

extension ConnectionPhase {
    /// Key suffix in the design strings (`st_<key>`).
    public var designKey: String {
        switch self {
        case .off: "off"
        case .preparing: "preparing"
        case .waitingPermission: "authorizing"
        case .connecting: "connecting"
        case .on: "on"
        case .reconnecting: "reconnecting"
        case .contended: "occupied"
        case .disconnecting: "disconnecting"
        case .error: "failed"
        }
    }

    public var tone: Tone {
        switch self {
        case .off: .idle
        case .on: .ok
        case .contended: .warn
        case .error: .err
        case .preparing, .waitingPermission, .connecting, .reconnecting, .disconnecting: .busy
        }
    }

    public var title: String { tr("st_" + designKey) }

    public var isTransitioning: Bool {
        switch self {
        case .preparing, .waitingPermission, .connecting, .reconnecting, .disconnecting: true
        case .off, .on, .contended, .error: false
        }
    }
}

extension AppState {
    public var restriction: Restriction? {
        switch snapshot.profileStatus {
        case .noSubscription:
            Restriction(code: .noSubscription, title: tr("noSubT"), message: tr("noSubD"),
                        systemImage: "bag", offersPurchase: true)
        case .subscriptionExpired(let expiredAt):
            Restriction(code: .subscriptionExpired, title: tr("expiredT"),
                        message: expiredAt.flatMap(ISO8601DateFormatter.parse).map {
                            tr("expiredD", ["d": $0.formatted(.dateTime.year().month(.abbreviated).day())])
                        } ?? tr("x_expiredGenericD"),
                        systemImage: "calendar.badge.exclamationmark", offersPurchase: true)
        case .teamDisabled:
            Restriction(code: .teamDisabled, title: tr("teamOffT"),
                        message: tr("teamOffD", ["team": snapshot.team?.name ?? ""]),
                        systemImage: "person.2.slash", offersPurchase: false)
        case .loading, .ready, .invalid:
            nil
        }
    }

    /// Route shown in the header: the live endpoint's label, else the label
    /// of that replica (or of the selected node's first one). Endpoint keys
    /// are internal and never shown: nil without a label.
    public var currentRoute: String? {
        let detail = snapshot.connection.detail
        if let label = detail.endpointLabel, !label.isEmpty { return label }
        if let key = detail.endpointKey {
            return selectedNode?.replicas.first { $0.endpointKey == key }?.routeLabel
        }
        return selectedNode?.orderedReplicas.first?.routeLabel
    }

    /// The standard core's failure: its local proxy is unavailable.
    public var standardError: ClientErrorInfo? {
        if case .failed(let error) = snapshot.standard { return error }
        return nil
    }

    /// What the local proxy card shows and copies: the routed user of the
    /// shared port (Profile rules, then the selected node; follows Rules /
    /// Global). None while the standard core failed, whatever was listed
    /// before. A node's own user is on the Nodes page.
    public var shownProxy: LocalProxy? { standardError == nil ? routedProxy : nil }

    /// The card's footnote.
    public var proxyNote: String { tr("proxyNoteRouted") }

    /// Why there is no local proxy to show (nil when there is one).
    public var localProxyUnavailableText: String? {
        if let standardError { return tr("proxyFailed", ["reason": standardError.message]) }
        if shownProxy != nil { return nil }
        if case .starting = snapshot.standard { return tr("proxyStarting") }
        return tr("x_proxyUnavailable")
    }

    public var currentLatencyText: String {
        if let ms = snapshot.connection.detail.latencyMs { return "\(ms) ms" }
        guard let id = snapshot.selectedNodeId, case .latency(let ms) = probes[id] else { return "—" }
        return "\(ms) ms"
    }

    public var presentation: ConnectionPresentation {
        let connection = snapshot.connection
        // Contended means occupied only by this account's other device
        // (ServiceBusy) or another OS user; any other contended reason (e.g.
        // NetworkPathContended) is a failure, as App.Core's State().
        let occupied = connection.reason.map { [.serviceBusy, .serviceOwnedByAnotherUser].contains($0.code) } ?? true
        let phase: ConnectionPhase = connection.phase == .contended && !occupied ? .error : connection.phase
        let route = currentRoute
        let ms = currentLatencyText
        // Without a route label, {r} and its separator are left out.
        func onDetail(_ key: String) -> String { route.map { tr(key, ["r": $0, "ms": ms]) } ?? ms }
        let trying = route.map { tr("d_connecting", ["r": $0]) } ?? ""
        let idleDetail = standardError == nil ? tr("d_idle") : tr("d_idleProxyFailed")
        var tone = phase.tone
        var headline: String
        var detail: String
        var image: String

        // Contended by another OS user of this Mac (not another device of this
        // account): no take-over, and the texts say who holds it.
        let heldByAnotherUser = phase == .contended && connection.reason?.code == .serviceOwnedByAnotherUser
        // Another proxy / VPN app the client named as competing for the path
        // (enhanced mode, failed or contended on it).
        let competitor = connection.competitor.flatMap { $0.isEmpty ? nil : $0 }
        let conflict = connectionMode == .enhanced && [.error, .contended].contains(phase) ? competitor : nil
        // "Taken over" only when an app is named.
        let pathTakenOver = conflict != nil

        switch connectionMode {
        case .enhanced:
            headline = switch phase {
            case .off: tr("h_idle")
            case .on: tr("h_on")
            case .error: pathTakenOver ? tr("h_pathContended") : tr("h_failed")
            case .contended:
                heldByAnotherUser ? tr("h_occupiedByUser") : pathTakenOver ? tr("h_pathContended") : tr("h_occupied")
            default: phase.title
            }
            detail = switch phase {
            case .off: idleDetail
            case .preparing: tr("d_preparing")
            case .waitingPermission: tr("d_authorizing")
            case .connecting: trying
            case .on: onDetail(switchedLine ? "d_switched" : "d_on")
            case .reconnecting: reconnecting(to: route)
            case .disconnecting: tr("d_disconnecting")
            // Failed / occupied: the notice carries the reason, and the
            // method caption already says 增强模式.
            case .contended, .error: ""
            }
        case .compatible:
            // The design's system-proxy texts.
            switch phase {
            case .off: (headline, detail) = (tr("h_idle"), idleDetail)
            case .on: (headline, detail) = (tr("h_on"), onDetail("d_stdOn"))
            case .error: (headline, detail) = (tr("h_stdFail"), failureReason)
            case .reconnecting: (headline, detail) = (tr("st_reconnecting"), reconnecting(to: route))
            case .disconnecting: (headline, detail) = (tr("st_disconnecting"), tr("d_disconnecting"))
            case .preparing, .waitingPermission, .connecting, .contended:
                (headline, detail) = (tr("h_stdStarting"), "")
            }
        }
        image = switch tone {
        case .ok: "checkmark.shield.fill"
        case .warn: heldByAnotherUser ? "person.2" : pathTakenOver ? "arrow.triangle.branch" : "laptopcomputer.and.iphone"
        case .err: pathTakenOver ? "arrow.triangle.branch" : "exclamationmark"
        case .busy: "shield"
        case .idle: "network.slash"
        }

        let restricted = restriction
        // The title's one short state (the header below says the rest).
        // While the saved sign-in is restored (a second or two at launch)
        // say nothing rather than 「未登录」, which read as the login page.
        let statusLine = snapshot.auth == .restoring ? ""
            : !isSignedIn ? tr("notSignedIn")
            : restricted.map(\.title) ?? shortState(phase: phase, headline: headline)
        let tray: TrayState
        if !isSignedIn || restricted != nil {
            tray = .off
        } else {
            tray = switch tone {
            case .ok: .on
            case .busy: .busy
            case .err, .warn: .error
            case .idle: .off
            }
        }
        if restricted != nil { tone = .idle }

        let connectSwitch: SwitchState = phase == .on ? .on : phase.isTransitioning ? .pending : .off
        let connectSwitchEnabled = ![.preparing, .waitingPermission, .disconnecting].contains(phase) && !nodes.isEmpty

        var notices: [Notice] = []
        if isSignedIn, restricted == nil, let conflict {
            // One notice for the failure and the path taken over: who holds it.
            notices.append(Notice(
                id: "conflict", tone: .err, systemImage: "arrow.triangle.branch", title: tr("conflictT"),
                message: tr("conflictD", ["app": conflict]),
                actionTitle: connection.retryable ? tr("retry") : nil,
                action: connection.retryable ? .retry : nil,
                secondary: connection.suggestCompatible ? .useCompatible : nil))
        } else if isSignedIn, restricted == nil {
            if phase == .error {
                let title = connectionMode == .enhanced
                    ? "\(tr("tunMode")) · \(tr("st_failed"))" : "\(tr("stdMode")) · \(tr("std_failed"))"
                notices.append(Notice(
                    id: "failed", tone: .err, systemImage: "xmark.circle.fill", title: title,
                    message: failureReason,
                    actionTitle: connection.retryable ? tr("retry") : nil,
                    action: connection.retryable ? .retry : nil,
                    secondary: connection.suggestCompatible ? .useCompatible : nil))
            }
            if phase == .contended {
                let (image, title, message): (String, String, String) =
                    if heldByAnotherUser {
                        ("person.2", tr("h_occupiedByUser"), ErrorCode.serviceOwnedByAnotherUser.message)
                    } else {
                        ("laptopcomputer.and.iphone", tr("st_occupied"), tr("d_occupied"))
                    }
                // Take over when this account's other device holds it; retry
                // when the path was taken; nothing for another OS user.
                let action: Notice.Action? = connection.canTakeOver ? .takeOver : connection.retryable ? .retry : nil
                notices.append(Notice(
                    id: "occupied", tone: .warn, systemImage: image, title: title, message: message,
                    actionTitle: action == .takeOver ? tr("takeOver") : action == .retry ? tr("retry") : nil,
                    action: action,
                    secondary: connection.suggestCompatible ? .useCompatible : nil))
            }
        }

        if isSignedIn, restricted == nil, let standardError {
            notices.append(Notice(
                id: "proxyFailed", tone: .err, systemImage: "network.slash", title: tr("proxyFailedT"),
                message: standardError.message, actionTitle: tr("retry"), action: .retryLocalProxy))
        }
        // Some routing rule sets are not loaded yet (their traffic goes
        // through the proxy meanwhile); clears once the core loads them.
        if isSignedIn, restricted == nil, !snapshot.ruleSetsUnavailable.isEmpty {
            notices.append(Notice(
                id: "rulesUnavailable", tone: .warn, systemImage: "arrow.triangle.branch",
                title: tr("rulesT"), message: tr("rulesUnavailableD"), actionTitle: nil, action: nil))
        }

        // The user pinned the current node to a line the core reports down:
        // it stays pinned; offer automatic failover.
        if isSignedIn, restricted == nil, let node = selectedNode,
           let pin = snapshot.ingressPins.first(where: { $0.nodeId == node.id }),
           snapshot.nodeIngresses.first(where: { $0.nodeId == node.id })?.ingresses
               .contains(where: { $0.endpointKey == pin.endpointKey && $0.healthy == false }) == true {
            notices.append(Notice(
                id: "ingressDown", tone: .warn, systemImage: "exclamationmark.triangle",
                title: tr("ingressDownT"),
                message: tr("ingressDownD", ["n": node.name, "r": node.lineName(pin.endpointKey) ?? ""]),
                actionTitle: tr("backToAuto"), action: .backToAuto))
        }
        // A profile refresh dropped a pinned line: back on automatic, say so once.
        if isSignedIn, restricted == nil, !snapshot.clearedIngressPins.isEmpty {
            var names: [String] = []
            for pin in snapshot.clearedIngressPins {
                let name = nodes.first { $0.id == pin.nodeId }?.name ?? pin.nodeId
                if !names.contains(name) { names.append(name) }
            }
            notices.append(Notice(
                id: "pinCleared", tone: .warn, systemImage: "arrow.uturn.backward",
                title: tr("pinClearedT"), message: tr("pinClearedD", ["n": names.joined(separator: ", ")]),
                actionTitle: tr("ok"), action: .dismissClearedPins))
        }
        // The standard core rebuilt the local proxy credentials: apps holding
        // the old ones (a browser extension) must copy them again. Said once.
        if isSignedIn, restricted == nil, snapshot.localProxyCredentialsReset {
            notices.append(Notice(
                id: "proxyReset", tone: .warn, systemImage: "key",
                title: tr("proxyResetT"), message: tr("proxyResetD"),
                actionTitle: tr("ok"), action: .dismissProxyReset))
        }
        // The TUN took another VPN's routes over for now; they go back on
        // disconnect. Said while connected, nothing to do about it.
        if isSignedIn, restricted == nil, phase == .on, !snapshot.replacedRoutes.isEmpty {
            notices.append(Notice(
                id: "routesReplaced", tone: .warn, systemImage: "info.circle",
                title: tr("routesReplacedT"), message: tr("routesReplacedD"), actionTitle: nil, action: nil))
        }

        return ConnectionPresentation(
            tone: tone, headline: headline, detail: detail, systemImage: image,
            statusLine: statusLine, tray: tray,
            connectSwitch: connectSwitch, connectSwitchEnabled: connectSwitchEnabled,
            notices: notices)
    }

    /// 未连接 / 正在连接 / 正在重连 / 已连接 / 正在断开; failures and take-overs keep
    /// their (short) headline.
    private func shortState(phase: ConnectionPhase, headline: String) -> String {
        switch phase {
        case .off: tr("h_idle")
        case .preparing, .waitingPermission, .connecting: tr("st_connecting")
        case .reconnecting: tr("st_reconnecting")
        case .on: tr("h_on")
        case .disconnecting: tr("st_disconnecting")
        case .error, .contended: headline
        }
    }

    /// `d_reconnecting {r0} {r}` when both lines have labels, `d_connecting
    /// {r}` when only the new one has, otherwise nothing. The previous line
    /// may belong to the previous node.
    private func reconnecting(to route: String?) -> String {
        guard let route else { return "" }
        let previous = snapshot.connection.detail.previousEndpointKey.flatMap { key in
            ([selectedNode].compactMap { $0 } + nodes).lazy
                .flatMap(\.replicas).first { $0.endpointKey == key }?.routeLabel
        }
        guard let previous else { return tr("d_connecting", ["r": route]) }
        return tr("d_reconnecting", ["r0": previous, "r": route])
    }

    /// Automatic failover moved the connection to another line (a pinned
    /// node never switches): 「已切换到 {r}」 instead of the plain line.
    private var switchedLine: Bool {
        let detail = snapshot.connection.detail
        guard let previous = detail.previousEndpointKey, previous != detail.endpointKey else { return false }
        return !snapshot.ingressPins.contains { $0.nodeId == snapshot.selectedNodeId }
    }

    /// Failure reason under 「增强模式 · 连接失败」 / 「系统代理 · 设置失败」.
    public var failureReason: String {
        guard let reason = snapshot.connection.reason else { return tr("fr_timeout") }
        switch reason.code {
        case .serviceInstallCancelled: return tr("fr_auth")
        case .connectFailed, .timeout, .unreachable: return tr("fr_timeout")
        // Another program kept changing the TUN's routes: no more reconnects.
        case .networkPathContended where reason.detail == "TUN_ROUTING_TAKEN_OVER": return tr("fr_routesTakenOver")
        // Taken over by nobody named: an ordinary failure.
        case .networkPathContended where snapshot.connection.competitor?.isEmpty ?? true: return tr("fr_timeout")
        case .systemProxyUnavailable: return tr("sysproxyUnavailable")
        case .systemProxyFailed:
            // A standard (non-admin) account can't run networksetup.
            return reason.detail.hasPrefix("ADMIN_REQUIRED") ? tr("x_adminRequired") : tr("fr_port")
        default: return reason.code.message
        }
    }
}

extension Replica {
    /// The line's label from the backend; nil without one (endpoint keys are
    /// internal and never shown).
    public var routeLabel: String? {
        guard let label, !label.isEmpty else { return nil }
        return label
    }
}

extension Node {
    /// More than one line: a pin changes something (single-line nodes show no picker).
    public var hasLineChoice: Bool { replicas.count > 1 }

    /// A line's name for the picker: its label, else `routeN` by its place in
    /// failover order (线路 2); nil when `endpointKey` is not one of this node's lines.
    public func lineName(_ endpointKey: String) -> String? {
        let replicas = orderedReplicas
        guard let index = replicas.firstIndex(where: { $0.endpointKey == endpointKey }) else { return nil }
        return replicas[index].routeLabel ?? tr("routeN", ["n": index + 1])
    }

    /// Lines in failover order: "HKG-A → HKG-B → SZX-R". A line without a
    /// label is named by its place (`routeN`: 线路 2); a single line without
    /// one shows nothing.
    public var routesText: String {
        let replicas = orderedReplicas
        if replicas.count == 1 { return replicas[0].routeLabel ?? "" }
        return replicas.enumerated()
            .map { index, replica in replica.routeLabel ?? tr("routeN", ["n": index + 1]) }
            .joined(separator: " → ")
    }
}
