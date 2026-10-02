//! Backend HTTP client and response models.
//!
//! A desktop or CLI access token may only call a handful of bearer
//! endpoints; everything else goes through the public device-authorization
//! endpoints ([`crate::auth`]).
//!
//! Errors never carry response bodies or credentials: only the HTTP status and
//! the backend problem `code` travel into [`ApiError`].

use std::time::Duration;

use reqwest::{StatusCode, Url};
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Value;

pub const PATH_DEVICE_CODE: &str = "/api/v1/auth/device/code";
pub const PATH_DEVICE_TOKEN: &str = "/api/v1/auth/device/token";
pub const PATH_DEVICE_ACTIVATE: &str = "/api/v1/auth/device/activate";
pub const PATH_DEVICE_REFRESH: &str = "/api/v1/auth/device/refresh";
pub const PATH_DEVICE_REFRESH_COMMIT: &str = "/api/v1/auth/device/refresh/commit";
pub const PATH_DEVICE_REVOKE: &str = "/api/v1/auth/device/revoke";
pub const PATH_USERS_ME: &str = "/api/v1/users/me";
pub const PATH_PROXY_PROFILE: &str = "/api/v1/me/proxy-profile";
pub const PATH_TEAMS: &str = "/api/v1/me/teams";
pub const PATH_SWITCH_TEAM: &str = "/api/v1/me/switch-team";
pub const PATH_MESSAGES: &str = "/api/v1/messages";
pub const PATH_MESSAGES_UNREAD: &str = "/api/v1/messages/unread-count";
pub const PATH_MESSAGES_READ_ALL: &str = "/api/v1/messages/read-all";
pub const PATH_DEVICES: &str = "/api/v1/devices";
pub const PATH_DEVICES_REGISTER: &str = "/api/v1/devices/register";

/// Longest backend problem code copied into an error detail.
const MAX_PROBLEM_CODE_LEN: usize = 64;

// ---------------------------------------------------------------------------
// Errors
// ---------------------------------------------------------------------------

/// A failed backend call, kept structured until the caller decides whether it
/// is terminal for the saved credential.
#[derive(Debug)]
pub enum ApiError {
    /// No HTTP response (DNS, TCP, TLS, timeout).
    Transport(reqwest::Error),
    /// Non-success status with the backend problem code, if any.
    Status {
        /// `METHOD /path` of the request (no query, no credentials).
        request: String,
        status: StatusCode,
        code: Option<String>,
        /// RFC 3339 end of the subscription, on `SUBSCRIPTION_EXPIRED`
        /// problems that carry one.
        expired_at: Option<String>,
    },
    /// A success status with a body we could not decode.
    Decode(&'static str),
    /// The configured API base cannot be used.
    Config(&'static str),
}

impl ApiError {
    /// 401/403 from a credential endpoint: the saved login can never work
    /// again and must be cleared.
    pub fn is_terminal_credential(&self) -> bool {
        // A disabled team is not a dead login: keep the credential so the
        // user can switch teams.
        !self.is_team_disabled()
            && matches!(
                self,
                Self::Status { status, .. }
                    if *status == StatusCode::UNAUTHORIZED || *status == StatusCode::FORBIDDEN
            )
    }

    /// 403 with problem code `403012`: the current team is disabled.
    pub fn is_team_disabled(&self) -> bool {
        matches!(
            self,
            Self::Status { status, code: Some(code), .. }
                if *status == StatusCode::FORBIDDEN && code == PROBLEM_TEAM_DISABLED
        )
    }

    pub fn is_unauthorized(&self) -> bool {
        matches!(self, Self::Status { status, .. } if *status == StatusCode::UNAUTHORIZED)
    }

    /// Worth retrying on the next poll or refresh tick.
    pub fn is_transient(&self) -> bool {
        match self {
            Self::Transport(_) => true,
            Self::Status { status, .. } => {
                *status == StatusCode::TOO_MANY_REQUESTS || status.is_server_error()
            }
            Self::Decode(_) | Self::Config(_) => false,
        }
    }

    pub fn is_rate_limited(&self) -> bool {
        matches!(self, Self::Status { status, .. } if *status == StatusCode::TOO_MANY_REQUESTS)
    }
}

/// `"<METHOD> <path> -> HTTP <status> <problem code>"`; the backend reuses
/// problem codes across endpoints, so the request tells them apart.
pub fn status_detail(request: &str, status: StatusCode, code: Option<&str>) -> String {
    match code {
        Some(code) => format!("{request} -> HTTP {} {code}", status.as_u16()),
        None => format!("{request} -> HTTP {}", status.as_u16()),
    }
}

/// Backend problem code for a suspended team (e.g. switching into one).
pub const PROBLEM_TEAM_DISABLED: &str = "403012";
/// Backend problem code for a dissolved team; shown like a suspended one.
pub const PROBLEM_TEAM_DISSOLVED: &str = "404011";

#[derive(Deserialize)]
struct ApiProblem {
    #[serde(default)]
    code: Option<String>,
    /// Subscription end on `SUBSCRIPTION_EXPIRED` (accepted as `expired_at`
    /// or `expires_at`).
    #[serde(default, alias = "expires_at")]
    expired_at: Option<String>,
    /// Human-readable backend explanation; logged, never put in details.
    #[serde(default)]
    message: Option<String>,
}

// ---------------------------------------------------------------------------
// Client
// ---------------------------------------------------------------------------

/// What a host configures an [`Api`] with.
#[derive(Clone, Debug)]
pub struct ApiConfig {
    /// Backend base URL, e.g. `https://api.example.com` (a path prefix is
    /// kept). HTTPS only, except loopback hosts in debug builds or with
    /// `trust_local_backend`.
    pub base: String,
    /// Product audience sent as `X-Product-Aud` on device-authorization
    /// calls; access tokens must carry exactly this audience.
    pub audience: String,
    /// `Accept-Language` value (e.g. `zh-CN`): the backend localises display
    /// names by it. `None` sends no header.
    pub accept_language: Option<String>,
    /// Test relaxations for a local mock backend: plain `http` and
    /// self-signed TLS, the backend's own `http` verification page, short
    /// device codes. Applied only when `base` is a loopback host.
    pub trust_local_backend: bool,
}

/// HTTP client bound to one configured API base.
pub struct Api {
    http: reqwest::Client,
    base: String,
    audience: String,
    /// `trust_local_backend` and a loopback base.
    trust_local: bool,
}

/// `Accept-Language` with the system locale (e.g. `zh-CN`): the backend
/// localises display names (node entry and line labels) by it, zh for any
/// zh-*, else en, as the apps pick their UI language.
fn language_headers(locale: &str) -> reqwest::header::HeaderMap {
    let mut headers = reqwest::header::HeaderMap::new();
    if let Ok(value) = reqwest::header::HeaderValue::from_str(locale) {
        headers.insert(reqwest::header::ACCEPT_LANGUAGE, value);
    }
    headers
}

/// `base` names this machine (loopback address or `localhost`).
pub fn is_loopback_base(base: &str) -> bool {
    let Ok(url) = Url::parse(base.trim()) else {
        return false;
    };
    let Some(host) = url.host_str() else {
        return false;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

impl Api {
    pub fn new(config: ApiConfig) -> Self {
        let trust_local = config.trust_local_backend && is_loopback_base(&config.base);
        let headers = config
            .accept_language
            .as_deref()
            .map(language_headers)
            .unwrap_or_default();
        // Building only fails when the TLS backend cannot initialise; fall
        // back to the default client instead of panicking (hosts may call
        // this across an FFI boundary).
        let http = reqwest::Client::builder()
            .default_headers(headers)
            .redirect(reqwest::redirect::Policy::none())
            .timeout(Duration::from_secs(20))
            // A local mock backend may use a self-signed certificate; only
            // ever for a loopback base under the test switch.
            .danger_accept_invalid_certs(trust_local)
            .build()
            .unwrap_or_default();
        Self {
            http,
            base: config.base.trim().to_string(),
            audience: config.audience,
            trust_local,
        }
    }

    /// The product audience tokens must carry.
    pub fn audience(&self) -> &str {
        &self.audience
    }

    /// The local-backend test relaxations apply.
    pub fn trusts_local_backend(&self) -> bool {
        self.trust_local
    }

    pub fn base(&self) -> &str {
        &self.base
    }

    /// `base` + `path`, keeping any path prefix of the base. HTTPS only,
    /// except loopback hosts in debug builds or under the test switch.
    pub fn endpoint(&self, path: &str) -> Result<Url, ApiError> {
        let mut url =
            Url::parse(&self.base).map_err(|_| ApiError::Config("API base URL is invalid"))?;
        let local_http = url.scheme() == "http"
            && (self.trust_local || (cfg!(debug_assertions) && is_loopback_base(&self.base)));
        if url.scheme() != "https" && !local_http {
            return Err(ApiError::Config("API base URL must use HTTPS"));
        }
        let prefix = url.path().trim_end_matches('/').to_string();
        url.set_path(&format!("{prefix}{path}"));
        url.set_query(None);
        url.set_fragment(None);
        Ok(url)
    }

    /// Unauthenticated device-authorization call.
    pub async fn post_public<T: DeserializeOwned>(
        &self,
        path: &str,
        body: &Value,
    ) -> Result<T, ApiError> {
        let request = self
            .http
            .post(self.endpoint(path)?)
            .header("X-Product-Aud", &self.audience)
            .json(body);
        decode("POST", request.send().await).await
    }

    async fn get_bearer<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
    ) -> Result<T, ApiError> {
        let request = self.http.get(self.endpoint(path)?).bearer_auth(token);
        decode("GET", request.send().await).await
    }

    /// `GET /users/me`.
    pub async fn account(&self, token: &str) -> Result<UserInfo, ApiError> {
        let user: UserResponse = self.get_bearer(PATH_USERS_ME, token).await?;
        Ok(user.into_user_info())
    }

    /// `GET /me/teams`.
    pub async fn teams(&self, token: &str) -> Result<Vec<TeamEntry>, ApiError> {
        let list: TeamListResponse = self.get_bearer(PATH_TEAMS, token).await?;
        Ok(list.items)
    }

    /// `POST /me/switch-team`; returns the team and a fresh access token
    /// bound to it.
    pub async fn switch_team(
        &self,
        token: &str,
        team_id: &str,
    ) -> Result<SwitchTeamResponse, ApiError> {
        let request = self
            .http
            .post(self.endpoint(PATH_SWITCH_TEAM)?)
            .bearer_auth(token)
            .json(&serde_json::json!({ "team_id": team_id }));
        decode("POST", request.send().await).await
    }

    async fn get_query<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let mut url = self.endpoint(path)?;
        url.query_pairs_mut()
            .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        let request = self.http.get(url).bearer_auth(token);
        decode("GET", request.send().await).await
    }

    async fn put_query<T: DeserializeOwned>(
        &self,
        path: &str,
        token: &str,
        query: &[(&str, String)],
    ) -> Result<T, ApiError> {
        let mut url = self.endpoint(path)?;
        if !query.is_empty() {
            url.query_pairs_mut()
                .extend_pairs(query.iter().map(|(key, value)| (*key, value.as_str())));
        }
        let request = self.http.put(url).bearer_auth(token);
        decode("PUT", request.send().await).await
    }

    /// `GET /messages?page=&page_size=`: newest first.
    pub async fn messages(
        &self,
        token: &str,
        page: u32,
        page_size: u32,
    ) -> Result<MessagePage, ApiError> {
        self.get_query(
            PATH_MESSAGES,
            token,
            &[
                ("page", page.to_string()),
                ("page_size", page_size.to_string()),
            ],
        )
        .await
    }

    /// `GET /messages/unread-count`.
    pub async fn unread_count(&self, token: &str) -> Result<u64, ApiError> {
        let count: UnreadCount = self.get_bearer(PATH_MESSAGES_UNREAD, token).await?;
        Ok(count.count)
    }

    /// `PUT /messages/{id}/read` (idempotent).
    pub async fn mark_message_read(&self, token: &str, id: u64) -> Result<(), ApiError> {
        let _: Value = self
            .put_query(&format!("{PATH_MESSAGES}/{id}/read"), token, &[])
            .await?;
        Ok(())
    }

    /// `POST /devices/register`: registers (or re-registers) this install for
    /// desktop push; the push token is only in this answer.
    pub async fn register_device(
        &self,
        token: &str,
        body: &Value,
    ) -> Result<DeviceRegistration, ApiError> {
        let request = self
            .http
            .post(self.endpoint(PATH_DEVICES_REGISTER)?)
            .bearer_auth(token)
            .json(body);
        decode("POST", request.send().await).await
    }

    /// `DELETE /devices/{id}`; an unknown device counts as deleted.
    pub async fn delete_device(&self, token: &str, id: u64) -> Result<(), ApiError> {
        let response = self
            .http
            .delete(self.endpoint(&format!("{PATH_DEVICES}/{id}"))?)
            .bearer_auth(token)
            .send()
            .await
            .map_err(ApiError::Transport)?;
        match check_status("DELETE", response).await {
            Ok(_) => Ok(()),
            Err(ApiError::Status { status, .. }) if status == StatusCode::NOT_FOUND => Ok(()),
            Err(error) => Err(error),
        }
    }

    /// `PUT /messages/{id}/unread`.
    // TODO(backend): endpoint requested, contract not yet confirmed.
    pub async fn mark_message_unread(&self, token: &str, id: u64) -> Result<(), ApiError> {
        let _: Value = self
            .put_query(&format!("{PATH_MESSAGES}/{id}/unread"), token, &[])
            .await?;
        Ok(())
    }

    /// `PUT /messages/read-all?up_to_id=`.
    pub async fn mark_all_messages_read(&self, token: &str, up_to_id: u64) -> Result<(), ApiError> {
        let _: Value = self
            .put_query(
                PATH_MESSAGES_READ_ALL,
                token,
                &[("up_to_id", up_to_id.to_string())],
            )
            .await?;
        Ok(())
    }

    /// `GET /me/proxy-profile` as raw bytes; the core validates the document.
    pub async fn proxy_profile(&self, token: &str) -> Result<Vec<u8>, ApiError> {
        let response = self
            .http
            .get(self.endpoint(PATH_PROXY_PROFILE)?)
            .bearer_auth(token)
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .map_err(ApiError::Transport)?;
        let response = check_status("GET", response).await?;
        let bytes = response.bytes().await.map_err(ApiError::Transport)?;
        Ok(bytes.to_vec())
    }
}

async fn check_status(
    method: &str,
    response: reqwest::Response,
) -> Result<reqwest::Response, ApiError> {
    let status = response.status();
    if status.is_success() {
        return Ok(response);
    }
    // Path only: queries (paging cursors) are left out, and credentials
    // travel in headers or bodies, never in the URL.
    let request = format!("{method} {}", response.url().path());
    let problem = response.json::<ApiProblem>().await.ok();
    let (code, message, expired_at) = match problem {
        Some(problem) => (problem.code, problem.message, problem.expired_at),
        None => (None, None, None),
    };
    let expired_at = expired_at.filter(|at| is_timestamp(at));
    let code = code.filter(|code| code.len() <= MAX_PROBLEM_CODE_LEN);
    if let Some(message) = message.filter(|message| !message.is_empty()) {
        tracing::info!(
            %request,
            status = status.as_u16(),
            code = code.as_deref().unwrap_or(""),
            "backend problem: {}",
            message.chars().take(300).collect::<String>()
        );
    }
    Err(ApiError::Status {
        request,
        status,
        code,
        expired_at,
    })
}

/// Marks the subscription end inside a `SubscriptionExpired` error detail.
pub const EXPIRED_AT_TAG: &str = "expired_at=";

/// A plausible RFC 3339 timestamp (only its characters and length are
/// checked; it is shown, never computed with).
fn is_timestamp(value: &str) -> bool {
    (10..=40).contains(&value.len())
        && value
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b':' | b'+' | b'.'))
}

/// The subscription end carried in a `SubscriptionExpired` detail.
pub fn expired_at_from_detail(detail: &str) -> Option<String> {
    detail
        .split(' ')
        .find_map(|part| part.strip_prefix(EXPIRED_AT_TAG))
        .filter(|at| is_timestamp(at))
        .map(str::to_string)
}

async fn decode<T: DeserializeOwned>(
    method: &str,
    sent: Result<reqwest::Response, reqwest::Error>,
) -> Result<T, ApiError> {
    let response = check_status(method, sent.map_err(ApiError::Transport)?).await?;
    let bytes = response.bytes().await.map_err(ApiError::Transport)?;
    serde_json::from_slice(&bytes)
        .map_err(|_| ApiError::Decode("backend returned an invalid response"))
}

// ---------------------------------------------------------------------------
// Response models
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// Messages (notifications)
// ---------------------------------------------------------------------------

/// One backend message, parsed leniently (old messages may lack fields).
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct Message {
    pub id: u64,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, rename = "type")]
    pub kind: String,
    #[serde(default)]
    pub event_key: String,
    #[serde(default)]
    pub severity: String,
    #[serde(default)]
    pub deep_link: String,
    #[serde(default)]
    pub push: bool,
    #[serde(default)]
    pub is_read: bool,
    #[serde(default)]
    pub created_at: String,
}

/// `POST /devices/register` answer.
#[derive(Deserialize, Debug)]
pub struct DeviceRegistration {
    pub id: u64,
    pub push_token: String,
}

#[derive(Deserialize, Debug, Default)]
pub struct MessagePage {
    #[serde(default)]
    pub items: Vec<Message>,
    #[serde(default)]
    pub total: u64,
}

#[derive(Deserialize)]
struct UnreadCount {
    #[serde(default)]
    count: u64,
}

#[derive(Deserialize)]
struct UserResponse {
    id: String,
    #[serde(default)]
    name: String,
    #[serde(default)]
    avatar: Option<String>,
    /// Not in the backend's `/users/me` today (identifiers live in
    /// credentials); read when it is added.
    #[serde(default)]
    email: Option<String>,
}

/// The signed-in user (`GET /users/me`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct UserInfo {
    pub id: String,
    pub name: String,
    pub avatar_url: Option<String>,
    pub email: Option<String>,
}

impl UserResponse {
    fn into_user_info(self) -> UserInfo {
        UserInfo {
            id: self.id,
            name: self.name,
            avatar_url: self.avatar.filter(|url| !url.is_empty()),
            email: self.email.filter(|email| !email.trim().is_empty()),
        }
    }
}

#[derive(Deserialize)]
struct TeamListResponse {
    #[serde(default)]
    items: Vec<TeamEntry>,
}

/// One `GET /me/teams` item.
#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct TeamEntry {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_personal: bool,
    /// The team the current access token is bound to.
    #[serde(default)]
    pub is_default: bool,
    /// `active`, `disabled` or `deleted`; absent means active.
    #[serde(default)]
    pub status: Option<String>,
}

impl TeamEntry {
    /// `active`, or no status at all.
    pub fn is_active(&self) -> bool {
        matches!(self.status.as_deref(), None | Some("active"))
    }
}

#[derive(Deserialize)]
pub struct SwitchTeamResponse {
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub is_personal: bool,
    #[serde(default)]
    pub token: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn api(base: &str, trust_local_backend: bool) -> Api {
        Api::new(ApiConfig {
            base: base.to_string(),
            audience: "app".to_string(),
            accept_language: None,
            trust_local_backend,
        })
    }

    #[test]
    fn requests_carry_the_configured_language() {
        let headers = language_headers("zh-CN");
        assert_eq!(headers[reqwest::header::ACCEPT_LANGUAGE], "zh-CN");
        // Not a valid header value: no header rather than a failed client.
        assert!(language_headers("zh\nCN").is_empty());
    }

    #[test]
    fn team_status_marks_disabled_teams_inactive() {
        let entries: Vec<TeamEntry> = serde_json::from_str(
            r#"[{"id":"a","name":"A","status":"active"},{"id":"b","name":"B","status":"disabled"},{"id":"c","name":"C"},{"id":"d","name":"D","status":"deleted"}]"#,
        )
        .unwrap();
        let active: Vec<bool> = entries.iter().map(TeamEntry::is_active).collect();
        assert_eq!(active, [true, false, true, false]);
    }

    #[test]
    fn user_email_is_optional() {
        let user: UserResponse =
            serde_json::from_str(r#"{"id":"u1","name":"N","avatar":"","email":"a@example.com"}"#)
                .unwrap();
        let info = user.into_user_info();
        assert_eq!(info.email.as_deref(), Some("a@example.com"));
        assert_eq!(info.avatar_url, None);
        let user: UserResponse = serde_json::from_str(r#"{"id":"u1","name":"N"}"#).unwrap();
        assert_eq!(user.into_user_info().email, None);
    }

    #[test]
    fn expired_at_survives_the_detail_round_trip() {
        let detail =
            format!("GET /x -> HTTP 400 SUBSCRIPTION_EXPIRED {EXPIRED_AT_TAG}2026-09-01T00:00:00Z");
        assert_eq!(
            expired_at_from_detail(&detail).as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
        assert_eq!(
            expired_at_from_detail("HTTP 400 SUBSCRIPTION_EXPIRED"),
            None
        );
        let problem: ApiProblem = serde_json::from_str(
            r#"{"code":"SUBSCRIPTION_EXPIRED","expires_at":"2026-09-01T00:00:00Z"}"#,
        )
        .unwrap();
        assert_eq!(problem.expired_at.as_deref(), Some("2026-09-01T00:00:00Z"));
        assert!(!is_timestamp("x y"));
    }

    #[test]
    fn credential_errors_are_classified() {
        let status = |status: u16, problem: Option<&str>| ApiError::Status {
            expired_at: None,
            request: "GET /api/v1/users/me".to_string(),
            status: StatusCode::from_u16(status).unwrap(),
            code: problem.map(str::to_string),
        };
        assert!(status(401, None).is_terminal_credential());
        assert!(status(403, None).is_terminal_credential());
        assert!(!status(403, Some(PROBLEM_TEAM_DISABLED)).is_terminal_credential());
        assert!(status(403, Some(PROBLEM_TEAM_DISABLED)).is_team_disabled());
        assert!(!status(500, None).is_terminal_credential());
        assert!(status(503, None).is_transient());
        assert!(status(429, None).is_rate_limited());
    }

    #[test]
    fn only_loopback_hosts_are_local() {
        for base in [
            "http://127.0.0.1:8080",
            "http://localhost",
            "http://[::1]:9",
        ] {
            assert!(is_loopback_base(base), "{base}");
        }
        for base in [
            "http://192.0.2.10",
            "https://api.example.com",
            "http://127.0.0.1.nip.io",
            "http://localhost.example.com",
            "not a url",
        ] {
            assert!(!is_loopback_base(base), "{base}");
        }
        // The switch never relaxes a remote backend.
        assert!(!api("http://api.example.com", true).trusts_local_backend());
        assert!(api("http://127.0.0.1:9", true).trusts_local_backend());
    }

    #[test]
    fn endpoint_keeps_base_prefix_and_requires_https() {
        let prefixed = api("https://api.example.com/prefix/", false);
        assert_eq!(
            prefixed.endpoint(PATH_USERS_ME).unwrap().as_str(),
            "https://api.example.com/prefix/api/v1/users/me"
        );
        assert!(api("http://api.example.com", false)
            .endpoint(PATH_USERS_ME)
            .is_err());
        assert!(api("http://api.example.com", true)
            .endpoint(PATH_USERS_ME)
            .is_err());
        // Under the switch plain http works for a loopback base in release
        // builds too; debug builds allow it anyway.
        assert!(api("http://127.0.0.1:8080", true)
            .endpoint(PATH_USERS_ME)
            .is_ok());
    }
}
