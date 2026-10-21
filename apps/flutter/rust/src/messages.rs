//! Conversions between the client's inbox types and the flat ones in
//! [`crate::api`] (the message center). Kept out of `api` so that the
//! codegen sees no client type.

use ppvpn_client::{InboxMessage, InboxPage, MessageCategory, MessageSeverity};

use crate::api::{UiMessage, UiMessageCategory, UiMessagePage, UiMessageSeverity};

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
        category: match message.category {
            MessageCategory::SubscriptionExpiring => UiMessageCategory::SubscriptionExpiring,
            MessageCategory::SubscriptionExpired => UiMessageCategory::SubscriptionExpired,
            MessageCategory::Billing => UiMessageCategory::Billing,
            MessageCategory::Order => UiMessageCategory::Order,
            MessageCategory::Route => UiMessageCategory::Route,
            MessageCategory::Announcement => UiMessageCategory::Announcement,
            MessageCategory::Other => UiMessageCategory::Other,
        },
        severity: match message.severity {
            MessageSeverity::Critical => UiMessageSeverity::Critical,
            MessageSeverity::Important => UiMessageSeverity::Important,
            MessageSeverity::Normal => UiMessageSeverity::Normal,
            MessageSeverity::Unspecified => UiMessageSeverity::Unspecified,
        },
        deep_link: message.deep_link,
        read: message.read,
        created_at: message.created_at,
    }
}

/// A [`UiMessage::id`] back to the backend's.
pub(crate) fn backend_id(id: i64) -> Result<u64, String> {
    u64::try_from(id).map_err(|_| format!("message id {id} is not a backend id"))
}
