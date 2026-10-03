import Foundation
import PPVPNClient

// MARK: - Message kind

/// Presentation of the backend `kind` and `severity`.
extension InboxMessage {
    public enum Kind { case expiring, expired, bill, order, route, broadcast, other }

    public var messageKind: Kind {
        switch category {
        case .subscriptionExpiring: .expiring
        case .subscriptionExpired: .expired
        case .billing: .bill
        case .order: .order
        case .route: .route
        case .announcement: .broadcast
        case .other: .other
        }
    }

    public var kindTitle: String {
        switch messageKind {
        case .expiring: tr("t_expiring")
        case .expired: tr("t_expired")
        case .bill: tr("t_bill")
        case .order: tr("t_order")
        case .route: tr("t_route")
        case .broadcast: tr("t_broadcast")
        case .other: tr("t_other")
        }
    }

    public var kindSymbol: String {
        switch messageKind {
        case .expiring: "calendar.badge.clock"
        case .expired: "calendar.badge.exclamationmark"
        case .bill: "doc.text"
        case .order: "bag"
        case .route: "arrow.triangle.branch"
        case .broadcast: "megaphone"
        case .other: "bell"
        }
    }

    public var severityTitle: String? {
        switch severity {
        case .critical: tr("sev_critical")
        case .important: tr("sev_important")
        case .normal, .unspecified: nil
        }
    }

    public var createdDate: Date? { ISO8601DateFormatter.parse(createdAt) }
    public var link: URL? { deepLink.flatMap(URL.init(string:)) }
}
