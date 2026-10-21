//! Conversions between the client's inbox types and the flat ones in
//! [`crate::api`] (the message center), and of the pushes the push agent
//! showed. Kept out of `api` so that the codegen sees no client type.

use ppvpn_client::{InboxMessage, InboxPage, MessageCategory, MessageSeverity, PushMessage};

use crate::api::{UiMessage, UiMessageCategory, UiMessagePage, UiMessageSeverity, UiPush};

pub(crate) fn ui_page(page: InboxPage) -> UiMessagePage {
    UiMessagePage {
        items: page.items.into_iter().map(ui_message).collect(),
        total: page.total,
    }
}

fn ui_message(message: InboxMessage) -> UiMessage {
    UiMessage {
        // Backend ids stay far below 2^63; one above would wrap negative
        // and fail in `backend_id` instead of naming another message.
        id: message.id as i64,
        title: message.title,
        content: message.content,
        category: ui_category(message.category),
        severity: ui_severity(message.severity),
        deep_link: message.deep_link,
        read: message.read,
        created_at: message.created_at,
    }
}

/// A shown push: the inbox message it is about, and itself as a read-only
/// message (its id is the push's) for when there is none. Its link only
/// when it is http(s), the only kind the app opens.
pub(crate) fn ui_push(push: PushMessage) -> UiPush {
    UiPush {
        message_id: push.message_id.and_then(|id| i64::try_from(id).ok()),
        message: UiMessage {
            id: push.id as i64,
            title: push.title,
            content: push.body,
            category: ui_category(push.category),
            severity: ui_severity(push.severity),
            deep_link: push.deep_link.filter(|link| {
                let link = link.to_ascii_lowercase();
                link.starts_with("https://") || link.starts_with("http://")
            }),
            read: true,
            created_at: push.created_at,
        },
    }
}

fn ui_category(category: MessageCategory) -> UiMessageCategory {
    match category {
        MessageCategory::SubscriptionExpiring => UiMessageCategory::SubscriptionExpiring,
        MessageCategory::SubscriptionExpired => UiMessageCategory::SubscriptionExpired,
        MessageCategory::Billing => UiMessageCategory::Billing,
        MessageCategory::Order => UiMessageCategory::Order,
        MessageCategory::Route => UiMessageCategory::Route,
        MessageCategory::Announcement => UiMessageCategory::Announcement,
        MessageCategory::Other => UiMessageCategory::Other,
    }
}

fn ui_severity(severity: MessageSeverity) -> UiMessageSeverity {
    match severity {
        MessageSeverity::Critical => UiMessageSeverity::Critical,
        MessageSeverity::Important => UiMessageSeverity::Important,
        MessageSeverity::Normal => UiMessageSeverity::Normal,
        MessageSeverity::Unspecified => UiMessageSeverity::Unspecified,
    }
}

/// A [`UiMessage::id`] back to the backend's.
pub(crate) fn backend_id(id: i64) -> Result<u64, String> {
    u64::try_from(id).map_err(|_| format!("message id {id} is not a backend id"))
}
