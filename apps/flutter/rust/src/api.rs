//! What the Dart side calls; flutter_rust_bridge generates its Dart from
//! this module only. The state stays in the client (see `bridge`); the types
//! here are flat copies of what the two pages show, without enums that carry
//! data (those would need freezed on the Dart side).

use flutter_rust_bridge::frb;

use crate::bridge;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiAuth {
    Restoring,
    SignedOut,
    AwaitingBrowser,
    SignedIn,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiProfile {
    Loading,
    Ready,
    NoSubscription,
    SubscriptionExpired,
    TeamDisabled,
    Invalid,
    NodesNotReady,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiPhase {
    Off,
    Preparing,
    WaitingPermission,
    Connecting,
    /// `ConnectionPhase::On` (Dart cannot name a constant `on`).
    Connected,
    Reconnecting,
    Contended,
    Disconnecting,
    Error,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiError {
    /// `ErrorCode` as its Rust name, e.g. `NetworkUnreachable`.
    pub code: String,
    /// For logs, not for display.
    pub detail: String,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiDeviceCode {
    pub user_code: String,
    pub verification_url: String,
    /// From when the code arrived.
    pub expires_in_secs: u32,
    pub browser_opened: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiSnapshot {
    pub auth: UiAuth,
    /// While `auth` is `AwaitingBrowser`.
    pub device_code: Option<UiDeviceCode>,
    pub account_name: Option<String>,
    pub team_name: Option<String>,
    pub profile: UiProfile,
    /// Changes when the node list does: read [`nodes`] again.
    pub profile_revision: Option<String>,
    /// Service paid-until, RFC 3339.
    pub expires_at: Option<String>,
    pub phase: UiPhase,
    /// Line (replica) in use: its label, else its key.
    pub line: Option<String>,
    pub latency_ms: Option<u32>,
    /// The standard-mode core runs: local proxies and speed tests work.
    pub standard_ready: bool,
    pub selected_node_id: Option<String>,
    /// Why the connection failed, else the latest failure of any action.
    pub error: Option<UiError>,
    /// Unread backend messages (the sidebar badge); 0 while signed out.
    pub unread: u32,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiNode {
    pub id: String,
    pub name: String,
    pub exit_region: Option<String>,
    pub exit_country_code: Option<String>,
    pub entry_label: Option<String>,
    /// Entry key, e.g. `cn-optimized`: picks the tier tag's colour.
    pub entry_key: String,
    pub udp: bool,
    /// Lines (replicas) in failover order: label, else key.
    pub lines: Vec<String>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiProbeMethod {
    Icmp,
    Tcp,
    Connect,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiProbe {
    pub node_id: String,
    pub success: bool,
    pub latency_ms: Option<u32>,
    /// `Timeout`, when it failed by timing out.
    pub timed_out: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiLocalProxy {
    pub node_id: String,
    pub host: String,
    pub port: u16,
    pub username: String,
    pub password: String,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct UiTraffic {
    /// Bytes per second.
    pub up: i64,
    pub down: i64,
}

#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct UiEvents {
    pub snapshot: Option<UiSnapshot>,
    pub traffic: Option<UiTraffic>,
    /// Speed-test results since the previous call, the latest per node.
    pub probes: Vec<UiProbe>,
}

/// Creates the client on the first call and returns the current state.
#[frb(sync)]
pub fn start() -> Result<UiSnapshot, String> {
    Ok(bridge::ui_snapshot(&bridge::start()?.client.snapshot()))
}

/// Waits for the next change and returns the latest snapshot and traffic
/// sample since the previous call (either may be absent).
pub async fn next_events() -> Result<UiEvents, String> {
    let events = bridge::get()?.events.clone();
    loop {
        {
            let mut pending = events.pending.lock().unwrap();
            if pending.snapshot.is_some() || pending.traffic.is_some() || !pending.probes.is_empty()
            {
                return Ok(std::mem::take(&mut *pending));
            }
        }
        events.ready.notified().await;
    }
}

#[frb(sync)]
pub fn nodes() -> Result<Vec<UiNode>, String> {
    Ok(bridge::get()?
        .client
        .nodes()
        .into_iter()
        .map(bridge::ui_node)
        .collect())
}

/// Downloads the node list again ("refresh nodes").
pub async fn refresh_nodes() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.refresh_profile().await })
        .await
}

/// Tests every node; results arrive in [`next_events`]. Fails until the
/// standard-mode core is ready.
pub async fn probe_all(method: UiProbeMethod) -> Result<(), String> {
    let method = bridge::probe_method(method);
    bridge::get()?
        .call(move |client| async move { client.probe(method, Vec::new()).await })
        .await
}

/// Each node's loopback proxy with its credentials (empty until the
/// standard-mode core is ready).
pub async fn local_proxies() -> Result<Vec<UiLocalProxy>, String> {
    bridge::get()?.local_proxies().await
}

/// Starts the device login; the code arrives in the snapshot
/// (`UiAuth::AwaitingBrowser`) and the browser opens on its own.
pub async fn sign_in() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.auth_start().await.map(|_| ()) })
        .await
}

/// Opens `url` in the default browser ("reopen the browser").
#[frb(sync)]
pub fn open_url(url: String) -> bool {
    crate::hooks::open_url(&url)
}

#[frb(sync)]
pub fn cancel_sign_in() -> Result<(), String> {
    bridge::get()?.client.auth_cancel();
    Ok(())
}

pub async fn sign_out() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.logout().await })
        .await
}

pub async fn select_node(node_id: String) -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.select_node(node_id).await })
        .await
}

pub async fn connect() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.connect().await })
        .await
}

pub async fn disconnect() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.disconnect().await })
        .await
}

pub async fn retry() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.retry().await })
        .await
}

/// Before the app quits: puts the system proxy back and stops the core
/// (blocks for at most ~10 s).
#[frb(sync)]
pub fn shutdown() {
    if let Some(bridge) = bridge::running() {
        bridge.client.shutdown();
    }
}

// --- logs ---

/// The client's log directory: crates/desktop writes
/// `ppvpn-client.YYYY-MM-DD.log` and `ppvpn-core.YYYY-MM-DD.log` there, one
/// per UTC day. The logs page reads them itself.
#[frb(sync)]
pub fn log_dir() -> Result<String, String> {
    Ok(bridge::get()?.client.log_dir())
}

// --- messages ---

/// `MessageCategory`: what a backend message is about (its icon and type
/// label).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMessageCategory {
    SubscriptionExpiring,
    SubscriptionExpired,
    Billing,
    Order,
    Route,
    Announcement,
    Other,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum UiMessageSeverity {
    Critical,
    Important,
    Normal,
    /// Old messages without a severity.
    Unspecified,
}

/// A backend message (inbox entry).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiMessage {
    /// The backend's id (a `u64`; Dart ints are signed).
    pub id: i64,
    pub title: String,
    pub content: String,
    pub category: UiMessageCategory,
    pub severity: UiMessageSeverity,
    /// Absolute URL behind 「查看详情」.
    pub deep_link: Option<String>,
    pub read: bool,
    /// RFC 3339.
    pub created_at: String,
}

/// One page of messages, newest first.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UiMessagePage {
    pub items: Vec<UiMessage>,
    /// All messages of the account.
    pub total: u32,
}

/// Page `page` (1-based) of the messages, `page_size` to a page (1–100).
/// Also refreshes the snapshot's `unread`.
pub async fn notifications(page: u32, page_size: u32) -> Result<UiMessagePage, String> {
    let inbox = bridge::get()?
        .fetch(move |client| async move { client.notifications(page, page_size).await })
        .await?;
    Ok(crate::messages::ui_page(inbox))
}

pub async fn mark_notification_read(id: i64) -> Result<(), String> {
    let id = crate::messages::backend_id(id)?;
    bridge::get()?
        .call(move |client| async move { client.mark_notification_read(id).await })
        .await
}

pub async fn mark_notification_unread(id: i64) -> Result<(), String> {
    let id = crate::messages::backend_id(id)?;
    bridge::get()?
        .call(move |client| async move { client.mark_notification_unread(id).await })
        .await
}

pub async fn mark_all_notifications_read() -> Result<(), String> {
    bridge::get()?
        .call(|client| async move { client.mark_all_notifications_read().await })
        .await
}
