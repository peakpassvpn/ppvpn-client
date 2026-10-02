//! Events, subscribed by kind (docs/host-integration.md, section 6). The
//! JSON of an [`Event`] is Core API v1's `watch-events`: a CamelCase `type`,
//! `at`, and the fields that type carries. Kinds and fields only grow.

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use crate::request::PinClearReason;
use crate::status::EngineState;

/// One event. `at` is when it happened.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
#[non_exhaustive]
pub enum Event {
    /// A transition of the state machine (section 5); the reasons are in
    /// `state`.
    StateChanged {
        at: DateTime<Utc>,
        state: EngineState,
        previous: EngineState,
    },
    ProfileApplied {
        at: DateTime<Utc>,
        revision: String,
    },
    /// An apply or an internal rebuild failed; the active configuration
    /// stays. `code` is the error code.
    ReloadFailed {
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        message: String,
    },
    CoreStarted {
        at: DateTime<Utc>,
    },
    CoreStopped {
        at: DateTime<Utc>,
    },
    NodeSelected {
        at: DateTime<Utc>,
        revision: String,
        node_id: String,
    },
    NodeEndpointChanged {
        at: DateTime<Utc>,
        revision: String,
        node_id: String,
    },
    NodeIngressSwitched {
        at: DateTime<Utc>,
        node_id: String,
        endpoint_key: String,
        previous_endpoint_key: String,
    },
    /// `endpoint_key` empty: back to automatic failover.
    NodeIngressPinned {
        at: DateTime<Utc>,
        node_id: String,
        #[serde(default)]
        endpoint_key: String,
    },
    /// The pin's node or ingress is gone from the new profile.
    NodeIngressPinCleared {
        at: DateTime<Utc>,
        revision: String,
        node_id: String,
        endpoint_key: String,
        reason: PinClearReason,
    },
    /// `message` is `success` or the error code.
    EntranceProbed {
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        revision: String,
        node_id: String,
        message: String,
    },
    /// `message` is `success` or the error code.
    AvailabilityProbed {
        at: DateTime<Utc>,
        node_id: String,
        message: String,
    },
    /// `message` is the new state; `code` the error when not ready.
    RuleSetChanged {
        at: DateTime<Utc>,
        rule_set_id: String,
        message: String,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        code: String,
    },
    /// `message` is `enabled` or `disabled`.
    SystemProxyChanged {
        at: DateTime<Utc>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        revision: String,
        message: String,
    },
    LocalProxyEndpointChanged {
        at: DateTime<Utc>,
        listen: String,
        port: u16,
    },
    KernelSwitched {
        at: DateTime<Utc>,
        revision: String,
        closed_connections: u32,
        kept_connections: u32,
        draining_kernels: u32,
    },
    /// `code` is `idle` or `deadline`.
    KernelDrained {
        at: DateTime<Utc>,
        code: String,
        closed_connections: u32,
    },
    NetworkChanged {
        at: DateTime<Utc>,
        has_default_interface: bool,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        interface_name: String,
        #[serde(default, skip_serializing_if = "is_zero")]
        interface_index: u32,
    },
    TunRoutingBroken {
        at: DateTime<Utc>,
        missing: Vec<String>,
        #[serde(default, skip_serializing_if = "String::is_empty")]
        error: String,
    },
    TunRoutingRestored {
        at: DateTime<Utc>,
        missing: Vec<String>,
    },
}

fn is_zero(value: &u32) -> bool {
    *value == 0
}

/// The kinds to subscribe to; one bounded buffer each, so a burst of one
/// kind never pushes out another.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[non_exhaustive]
pub enum EventKind {
    StateChanged,
    ProfileApplied,
    ReloadFailed,
    CoreStarted,
    CoreStopped,
    NodeSelected,
    NodeEndpointChanged,
    NodeIngressSwitched,
    NodeIngressPinned,
    NodeIngressPinCleared,
    EntranceProbed,
    AvailabilityProbed,
    RuleSetChanged,
    SystemProxyChanged,
    LocalProxyEndpointChanged,
    KernelSwitched,
    KernelDrained,
    NetworkChanged,
    TunRoutingBroken,
    TunRoutingRestored,
}

impl EventKind {
    /// Every kind (`subscribe(EventKind::ALL)`).
    pub const ALL: &'static [EventKind] = &[
        Self::StateChanged,
        Self::ProfileApplied,
        Self::ReloadFailed,
        Self::CoreStarted,
        Self::CoreStopped,
        Self::NodeSelected,
        Self::NodeEndpointChanged,
        Self::NodeIngressSwitched,
        Self::NodeIngressPinned,
        Self::NodeIngressPinCleared,
        Self::EntranceProbed,
        Self::AvailabilityProbed,
        Self::RuleSetChanged,
        Self::SystemProxyChanged,
        Self::LocalProxyEndpointChanged,
        Self::KernelSwitched,
        Self::KernelDrained,
        Self::NetworkChanged,
        Self::TunRoutingBroken,
        Self::TunRoutingRestored,
    ];
}

impl Event {
    pub fn kind(&self) -> EventKind {
        match self {
            Self::StateChanged { .. } => EventKind::StateChanged,
            Self::ProfileApplied { .. } => EventKind::ProfileApplied,
            Self::ReloadFailed { .. } => EventKind::ReloadFailed,
            Self::CoreStarted { .. } => EventKind::CoreStarted,
            Self::CoreStopped { .. } => EventKind::CoreStopped,
            Self::NodeSelected { .. } => EventKind::NodeSelected,
            Self::NodeEndpointChanged { .. } => EventKind::NodeEndpointChanged,
            Self::NodeIngressSwitched { .. } => EventKind::NodeIngressSwitched,
            Self::NodeIngressPinned { .. } => EventKind::NodeIngressPinned,
            Self::NodeIngressPinCleared { .. } => EventKind::NodeIngressPinCleared,
            Self::EntranceProbed { .. } => EventKind::EntranceProbed,
            Self::AvailabilityProbed { .. } => EventKind::AvailabilityProbed,
            Self::RuleSetChanged { .. } => EventKind::RuleSetChanged,
            Self::SystemProxyChanged { .. } => EventKind::SystemProxyChanged,
            Self::LocalProxyEndpointChanged { .. } => EventKind::LocalProxyEndpointChanged,
            Self::KernelSwitched { .. } => EventKind::KernelSwitched,
            Self::KernelDrained { .. } => EventKind::KernelDrained,
            Self::NetworkChanged { .. } => EventKind::NetworkChanged,
            Self::TunRoutingBroken { .. } => EventKind::TunRoutingBroken,
            Self::TunRoutingRestored { .. } => EventKind::TunRoutingRestored,
        }
    }
}

/// What a receiver yields: an event, or how many of a kind were dropped
/// because this receiver fell behind.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "item", rename_all = "snake_case")]
#[non_exhaustive]
pub enum EventItem {
    Event { event: Event },
    Lagged { kind: EventKind, dropped: u64 },
}

/// A subscription ([`crate::Engine::subscribe`]); dropping it unsubscribes.
/// Closed (`recv` returns `None`) after `shutdown`.
#[derive(Debug)]
pub struct EventReceiver {
    pub(crate) receiver: mpsc::Receiver<EventItem>,
}

impl EventReceiver {
    pub async fn recv(&mut self) -> Option<EventItem> {
        self.receiver.recv().await
    }
}

/// Log lines (`LogSink::Channel`), logfmt.
#[derive(Debug)]
pub struct LogReceiver {
    pub(crate) receiver: mpsc::Receiver<String>,
}

impl LogReceiver {
    pub async fn recv(&mut self) -> Option<String> {
        self.receiver.recv().await
    }
}
