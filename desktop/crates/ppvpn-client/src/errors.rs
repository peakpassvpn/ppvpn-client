//! Stable, user-facing error codes.
//!
//! [`ErrorCode`] is the only thing the native apps localise. Variants are
//! append-only: never rename or remove one, because every app's string catalog
//! keys on them. The precise upstream reason (a backend problem code, a core
//! `reason_code`, an HTTP status, an OS error) travels in the accompanying
//! `detail` string and is meant for logs and bug reports, not for display.

/// Why an action or a background task failed.
#[derive(uniffi::Enum, Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    // --- network & backend ------------------------------------------------
    /// DNS, TCP or TLS failure reaching the backend; check the connection.
    NetworkUnreachable,
    /// The backend answered 5xx or an unparseable body; try again later.
    ServerUnavailable,
    /// The backend answered 429.
    RateLimited,

    // --- sign-in ----------------------------------------------------------
    /// The device code expired before the user confirmed it in the browser.
    AuthExpired,
    /// The user rejected the device authorization in the browser.
    AuthDenied,
    /// The saved session was revoked or is no longer valid; sign in again.
    AuthSessionInvalid,
    /// The browser could not be opened; show the URL and code instead.
    AuthBrowserOpenFailed,
    /// Reading or writing the platform credential store failed.
    CredentialStoreFailed,

    // --- account & profile -------------------------------------------------
    /// The current team has no active subscription.
    NoSubscription,
    /// The subscription expired.
    SubscriptionExpired,
    /// The proxy profile could not be downloaded (non-5xx, non-network).
    ProfileFetchFailed,
    /// The core rejected the proxy profile; report a bug.
    ProfileInvalid,
    /// The selected node no longer exists in the profile.
    NodeNotFound,

    // --- standard mode (user-level core) -------------------------------------
    /// The bundled `ppvpn-core` binary is missing or not executable.
    CoreBinaryMissing,
    /// The standard-mode core did not start or exited unexpectedly.
    StandardCoreFailed,

    // --- speed tests ----------------------------------------------------------
    /// No answer within the probe deadline.
    Timeout,
    /// Host unreachable, connection refused or reset.
    Unreachable,
    /// ICMP is not permitted for unprivileged users on this system
    /// (Linux `net.ipv4.ping_group_range`); use TCP instead.
    IcmpNotPermitted,
    /// Any other probe failure.
    ProbeFailed,

    // --- enhanced mode (privileged service) ------------------------------------
    /// The user cancelled the administrator prompt.
    ServiceInstallCancelled,
    /// Installing or uninstalling the privileged service failed.
    ServiceInstallFailed,
    /// The privileged service is installed but not reachable or not running.
    ServiceUnavailable,
    /// The privileged service rejected this app (version or identity check).
    ServiceIncompatible,
    /// Another user session owns the enhanced-mode connection.
    ServiceBusy,
    /// Tunnel set-up or the health check through the node failed (the
    /// direct-path steps report [`ErrorCode::ConnectHealthCheckFailed`]).
    ConnectFailed,
    /// Another VPN or proxy took over the system route or DNS.
    NetworkPathContended,

    /// A bug; the detail string says where.
    Internal,

    // --- appended after the first release (append-only below) ---------------
    /// The backend rejected the request (a 4xx not covered by a more specific
    /// code); the detail carries the status and problem code.
    RequestRejected,
    /// Writing app state under `data_dir` failed (node selection, connection
    /// state); check disk space and permissions.
    LocalStorageFailed,
    /// The backend's sign-in response failed a safety check (untrusted
    /// verification URL, token for another audience, invalid device
    /// transaction); sign-in was aborted.
    AuthUntrustedResponse,
    /// The bundled `ppvpn-core` speaks an unsupported Core API version;
    /// reinstall or update the app.
    CoreIncompatible,
    /// The core rejected the proxy profile as expired; it is refetched
    /// automatically, or check the subscription.
    ProfileExpired,
    /// The user cancelled the administrator prompt while uninstalling the
    /// privileged service.
    ServiceUninstallCancelled,
    /// The privileged service refused this app's identity, usually because the
    /// app is not installed at the expected location (the detail says which).
    ServiceClientRejected,
    /// The current team is disabled by its owner or the operator; switch to
    /// another team (backend problem code `403012`).
    TeamDisabled,
    /// Enhanced mode is in use by another OS user on this computer; it cannot
    /// be taken over from this account.
    ServiceOwnedByAnotherUser,
    /// Compatible mode could not set up or restore the OS system proxy (the
    /// detail says why, e.g. `ADMIN_REQUIRED` on a macOS standard account).
    SystemProxyFailed,
    /// Enhanced mode set up the tunnel, but the post-connect health check
    /// could not reach the backend on the direct path (DNS failure, no answer,
    /// non-2xx) or the request bypassed TUN: DNS or the network is likely
    /// taken over by other software. The detail names the step
    /// (`HEALTH_DIRECT_PATH_FAILED`, `HEALTH_DIRECT_STATUS_FAILED: 404`,
    /// `HEALTH_CAPTURE_PATH_FAILED`).
    ConnectHealthCheckFailed,
    /// The platform credential store (keychain, keyring) is locked
    /// ([`crate::PlatformError::Locked`]). The saved login is kept and read
    /// again in the background, so the session comes back once the store is
    /// unlocked; `Client::retry_credential_restore` reads it again now.
    CredentialStoreLocked,
    /// Compatible mode: this desktop has no system proxy settings the app can
    /// write (Linux without `gsettings` or `kwriteconfig`, detail
    /// `NO_DESKTOP_PROXY_SETTINGS`). Retrying cannot help; use enhanced mode
    /// or point apps at the local proxy.
    SystemProxyUnavailable,
}

/// A failure the UI shows until the next successful action clears it.
#[derive(uniffi::Record, Clone, Debug, PartialEq, Eq)]
pub struct ClientErrorInfo {
    pub code: ErrorCode,
    /// Upstream reason for logs, e.g. `GET /api/v1/me/proxy-profile -> HTTP 403 403012`
    /// or `HEALTH_ENTRANCE_FAILED`.
    pub detail: String,
}

impl ClientErrorInfo {
    pub fn new(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self {
            code,
            detail: detail.into(),
        }
    }
}

/// Error returned by [`crate::Client`] methods.
#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum ClientError {
    #[error("not signed in")]
    NotSignedIn,
    #[error("standard core is not ready")]
    StandardNotReady,
    #[error("{code:?}: {detail}")]
    Failed { code: ErrorCode, detail: String },
    #[error("not implemented yet")]
    NotImplemented,
    /// The action was cancelled or superseded by a newer one (e.g. a second
    /// `auth_start`, or `auth_cancel`). Not a failure: apps ignore it and
    /// follow the snapshot.
    #[error("cancelled")]
    Cancelled,
}

impl ClientError {
    pub fn failed(code: ErrorCode, detail: impl Into<String>) -> Self {
        Self::Failed {
            code,
            detail: detail.into(),
        }
    }

    /// The snapshot-facing form, or `None` for errors that are not failures
    /// worth showing (sign-in state and readiness are already in the snapshot).
    pub fn info(&self) -> Option<ClientErrorInfo> {
        match self {
            Self::Failed { code, detail } => Some(ClientErrorInfo::new(*code, detail.clone())),
            Self::NotImplemented => {
                Some(ClientErrorInfo::new(ErrorCode::Internal, "not implemented"))
            }
            Self::NotSignedIn | Self::StandardNotReady | Self::Cancelled => None,
        }
    }
}

impl From<ClientErrorInfo> for ClientError {
    fn from(info: ClientErrorInfo) -> Self {
        Self::Failed {
            code: info.code,
            detail: info.detail,
        }
    }
}

/// Maps a reqwest transport error (no HTTP response) to a code.
pub(crate) fn transport_error(error: &reqwest::Error) -> ClientError {
    ClientError::failed(ErrorCode::NetworkUnreachable, error.to_string())
}

/// Maps a non-success HTTP status (with the backend problem code, if any) of
/// `request` (`METHOD /path`).
pub(crate) fn http_error(
    request: &str,
    status: reqwest::StatusCode,
    problem_code: Option<&str>,
) -> ClientError {
    let detail = crate::api::status_detail(request, status, problem_code);
    let code = match status.as_u16() {
        401 => ErrorCode::AuthSessionInvalid,
        429 => ErrorCode::RateLimited,
        500..=599 => ErrorCode::ServerUnavailable,
        _ => ErrorCode::RequestRejected,
    };
    ClientError::failed(code, detail)
}
