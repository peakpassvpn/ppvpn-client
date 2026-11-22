//! Push registration, on [`ppvpn_account::device`].
//!
//! iOS and macOS (#539) bind a stable installation proof
//! (`<data_dir>/apple-push-installation.json`) to the active login session
//! via `/devices/apple/installations`, register or rotate the OS APNs token on
//! that binding, and call `DELETE .../bindings/{binding_id}` at sign-out.
//! Same-account legacy registrations and logged-out sessions are taken over
//! directly on the backend without deleting old devices first.
//!
//! Android (#539) registers the FCM token the host hands over via
//! `POST /devices/register` and unregisters it by token at sign-out.
//!
//! Windows and Linux register for the self-hosted channel (main-app side of the
//! push agent): each install has a stable `installation_id`
//! (`<data_dir>/installation.json`, kept across sign-outs), writes the returned
//! `push_token` to `<data_dir>/push-agent.json`, and deletes the device and
//! agent files at sign-out.

use std::path::{Path, PathBuf};
use std::sync::atomic::Ordering;

use ppvpn_account::device::{
    apple_environment, apple_push, installation_id, load_or_create_apple_installation,
    normalize_apple_token, official_push, push_platform, read_apple_installation,
    rotate_apple_installation, save_apple_installation, AppleBindingRequest,
    AppleInstallationRequest, AppleRegistrationRequest, DeviceRegistrationRequest,
    PROBLEM_APPLE_BINDING_CLOSED, PROBLEM_APPLE_BINDING_CONFLICT, PROBLEM_APPLE_PROOF_INVALID,
    PROBLEM_APPLE_REGISTRATION_CONFLICT, PROBLEM_APPLE_REQUEST_CONFLICT,
    PROBLEM_APPLE_SCOPE_CONFLICT, PROBLEM_APPLE_TOKEN_OWNERSHIP_CONFLICT,
};
pub(crate) use ppvpn_account::device::{write_private, DeviceState};
use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::api::ApiErrorExt;
use crate::errors::ClientError;
use crate::session::{BearerApiError, ClientRef};
use crate::Client;

pub(crate) const AGENT_FILE: &str = "push-agent.json";
pub(crate) const AGENT_CURSOR_FILE: &str = "push-agent-cursor.json";
pub(crate) const PUSH_TOKEN_FILE: &str = "push-token.json";

/// The OS push token (APNs, FCM) the host handed over.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct OsPushToken {
    pub token: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
}

pub(crate) fn read_os_token(data_dir: &str) -> Option<OsPushToken> {
    let raw = std::fs::read(Path::new(data_dir).join(PUSH_TOKEN_FILE)).ok()?;
    serde_json::from_slice(&raw).ok()
}

fn default_apple_bundle_id(platform: &str) -> &'static str {
    match platform {
        "ios" => "com.peakpassvpn.ppvpn",
        "macos" => "com.peakpassvpn.ppvpn.desktop",
        _ => "",
    }
}

fn clamp_chars(raw: &str, max_len: usize) -> String {
    if raw.len() <= max_len {
        return raw.to_string();
    }
    let mut out = String::new();
    for ch in raw.chars() {
        if out.len() + ch.len_utf8() > max_len {
            break;
        }
        out.push(ch);
    }
    out
}

fn is_apple_conflict(error: &ApiError, expected_code: &str) -> bool {
    error.status() == Some(reqwest::StatusCode::CONFLICT)
        && error.problem_code() == Some(expected_code)
}

/// What the push agent needs; written by the main app only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentFile {
    pub api_base: String,
    pub device_id: u64,
    pub push_token: String,
    pub installation_id: String,
}

pub(crate) fn agent_file_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join(AGENT_FILE)
}

pub(crate) fn read_agent_file(data_dir: &str) -> Option<AgentFile> {
    let raw = std::fs::read(agent_file_path(data_dir)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Sign-out (or macOS legacy-agent cleanup): the agent must stop pulling for
/// this account, and its pushes no longer resolve notification clicks.
pub(crate) fn remove_agent_files(data_dir: &str) {
    for name in [AGENT_FILE, AGENT_CURSOR_FILE, crate::push_shown::SHOWN_FILE] {
        let path = Path::new(data_dir).join(name);
        match std::fs::remove_file(&path) {
            Ok(()) => {}
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
            Err(error) => tracing::warn!("remove {}: {error}", path.display()),
        }
    }
}

pub(crate) fn locale() -> String {
    sys_locale::get_locale()
        .map(|locale| locale.replace('_', "-"))
        .filter(|locale| !locale.is_empty())
        .unwrap_or_else(|| "en-US".to_string())
}

fn timezone() -> String {
    iana_time_zone::get_timezone()
        .ok()
        .filter(|zone| !zone.is_empty())
        .unwrap_or_else(|| "UTC".to_string())
}

/// 403 (missing `device:register` scope, or not a desktop session).
fn needs_new_token(error: &ClientError) -> bool {
    matches!(error, ClientError::Failed { detail, .. } if detail.contains("-> HTTP 403"))
}

impl Client {
    /// The APNs device token (iOS/macOS) or FCM registration token (Android)
    /// the OS gave the app, and the app's bundle id / package name when known.
    pub fn set_push_token(&self, token: String, bundle_id: Option<String>) {
        self.set_push_token_with_environment(token, bundle_id, None);
    }

    /// Like [`Self::set_push_token`], carrying the signed APNs environment
    /// (`sandbox` or `production`) when known on Apple platforms (#539).
    pub fn set_push_token_with_environment(
        &self,
        token: String,
        bundle_id: Option<String>,
        environment: Option<String>,
    ) {
        if !official_push(&self.config.platform) {
            tracing::warn!(
                "push registration: OS push tokens are for official-push platforms only"
            );
            return;
        }
        let new = if apple_push(&self.config.platform) {
            let Some(normalized) = normalize_apple_token(&token) else {
                tracing::warn!("apple push registration: ignoring malformed hex device token");
                return;
            };
            let resolved_env = match environment.as_deref() {
                Some(raw) => match apple_environment(Some(raw)) {
                    Some(env) => env,
                    None => {
                        tracing::warn!(
                            "apple push registration: ignoring unknown APNs environment"
                        );
                        return;
                    }
                },
                None => "production",
            };
            let resolved_bundle = bundle_id
                .map(|id| id.trim().to_string())
                .filter(|id| !id.is_empty())
                .unwrap_or_else(|| default_apple_bundle_id(&self.config.platform).to_string());
            OsPushToken {
                token: normalized,
                bundle_id: Some(resolved_bundle),
                environment: Some(resolved_env.to_string()),
            }
        } else {
            let trimmed = token.trim().to_string();
            if trimmed.is_empty() {
                return;
            }
            OsPushToken {
                token: trimmed,
                bundle_id: bundle_id
                    .map(|id| id.trim().to_string())
                    .filter(|id| !id.is_empty()),
                environment: None,
            }
        };

        let old = read_os_token(&self.config.data_dir);
        let changed = old.as_ref() != Some(&new);
        if changed {
            let raw = serde_json::to_vec(&new).unwrap_or_default();
            let path = Path::new(&self.config.data_dir).join(PUSH_TOKEN_FILE);
            if let Err(detail) = write_private(&path, &raw) {
                tracing::warn!("push registration: {detail}");
                return;
            }
            self.push_generation.fetch_add(1, Ordering::SeqCst);
        }
        let Ok(session) = self.current_session() else {
            return;
        };
        let replaced = old.filter(|old| old.token != new.token);
        let was_registered = {
            let mut state = self.session_state();
            let registered = state.is_session(session) && state.device == DeviceState::Registered;
            if state.is_session(session)
                && changed
                && matches!(
                    state.device,
                    DeviceState::Registered | DeviceState::Deferred
                )
            {
                state.device = DeviceState::Pending;
            }
            registered
        };
        let is_apple = apple_push(&self.config.platform);
        let weak = self.this.clone();
        self.runtime.spawn(async move {
            let Some(client) = ClientRef::upgrade(&weak) else {
                return;
            };
            if !is_apple {
                if let (Some(old), true) = (replaced, was_registered) {
                    client.unregister_os_token(session, &old.token).await;
                }
            }
            client.ensure_device_registered(session).await;
        });
    }

    /// Registers when the session wants it; called by the refresh loop and
    /// token callbacks, serialized with sign-out.
    pub(crate) async fn ensure_device_registered(&self, session: u64) {
        let _guard = self.device_registration.lock().await;
        let wanted = {
            let state = self.session_state();
            state.is_session(session) && state.device == DeviceState::Pending
        };
        if !wanted {
            return;
        }
        let generation = self.push_generation.load(Ordering::SeqCst);
        if apple_push(&self.config.platform) {
            self.register_apple_device(session, generation).await;
        } else {
            self.register_device(session, generation).await;
        }
    }

    /// After an access-token refresh: retry a registration the old token was
    /// not allowed to make.
    pub(crate) fn on_access_refreshed(&self, session: u64) {
        let retry = {
            let mut state = self.session_state();
            let retry = state.is_session(session) && state.device == DeviceState::AwaitingRefresh;
            if retry {
                state.device = DeviceState::Pending;
            }
            retry
        };
        if retry {
            let weak = self.this.clone();
            self.runtime.spawn(async move {
                if let Some(client) = ClientRef::upgrade(&weak) {
                    client.ensure_device_registered(session).await;
                }
            });
        }
    }

    async fn register_apple_device(&self, session: u64, generation: u64) {
        let Some(os_token) = read_os_token(&self.config.data_dir) else {
            // Registered once the host hands the APNs token over.
            return;
        };
        let Some(token_hex) = normalize_apple_token(&os_token.token) else {
            self.finish_device_state(session, generation, DeviceState::Deferred);
            return;
        };
        let platform = self.config.platform.as_str();
        let bundle_id = os_token
            .bundle_id
            .as_deref()
            .filter(|id| !id.is_empty())
            .unwrap_or_else(|| default_apple_bundle_id(platform));
        let Some(environment) =
            apple_environment(os_token.environment.as_deref().or(Some("production")))
        else {
            self.finish_device_state(session, generation, DeviceState::Deferred);
            return;
        };

        let config = match self
            .bearer_api(session, |bearer| async move {
                self.auth.api().apple_push_config(&bearer).await
            })
            .await
        {
            Ok(config) => config,
            Err(error) => {
                self.handle_apple_non_cas_error(session, generation, error);
                return;
            }
        };
        if !config.supports(platform, environment) {
            tracing::info!(
                platform,
                environment,
                configured = config.configured,
                "apple push registration skipped: backend capability not configured"
            );
            self.finish_device_state(session, generation, DeviceState::Deferred);
            return;
        }

        let mut install = match load_or_create_apple_installation(
            &self.config.data_dir,
            platform,
            bundle_id,
            environment,
        ) {
            Ok(install) => install,
            Err(detail) => {
                tracing::warn!("apple push installation unavailable: {detail}");
                return;
            }
        };

        let mut scope_rotations_left = 1usize;
        let mut stale_conflicts_left = 1usize;
        let mut request_conflicts_left = 1usize;

        loop {
            if !self.session_is(session) || self.is_shut_down() {
                return;
            }
            let resolve_body = AppleInstallationRequest {
                installation_id: install.installation_id.clone(),
                installation_secret: install.installation_secret.clone(),
                platform: platform.to_string(),
                bundle_id: bundle_id.to_string(),
                environment: environment.to_string(),
            };
            let resolved = match self
                .bearer_api(session, |bearer| {
                    let body = resolve_body.clone();
                    async move {
                        self.auth
                            .api()
                            .resolve_apple_installation(&bearer, &body)
                            .await
                    }
                })
                .await
            {
                Ok(resolved) => resolved,
                Err(BearerApiError::Api(error))
                    if is_apple_conflict(&error, PROBLEM_APPLE_SCOPE_CONFLICT)
                        && scope_rotations_left > 0 =>
                {
                    scope_rotations_left -= 1;
                    tracing::info!(
                        "apple push installation scope changed; creating a new installation id"
                    );
                    match rotate_apple_installation(
                        &self.config.data_dir,
                        platform,
                        bundle_id,
                        environment,
                    ) {
                        Ok(rotated) => {
                            install = rotated;
                            continue;
                        }
                        Err(detail) => {
                            tracing::warn!("apple push installation rotation failed: {detail}");
                            return;
                        }
                    }
                }
                Err(error) => {
                    self.handle_apple_non_cas_error(session, generation, error);
                    return;
                }
            };

            install.binding_id = resolved.binding_id.clone();
            install.registration_id = resolved.registration_id.clone();
            let _ = save_apple_installation(&self.config.data_dir, &install);

            let (binding_id, expected_registration_id) = match (
                resolved.bound_to_current_session,
                resolved.binding_id.clone(),
            ) {
                (true, Some(binding_id)) => (binding_id, resolved.registration_id),
                (_, expected_binding_id) => {
                    let bound = loop {
                        let bind_body = AppleBindingRequest {
                            installation_secret: install.installation_secret.clone(),
                            expected_binding_id: expected_binding_id.clone(),
                            request_id: uuid::Uuid::new_v4().to_string(),
                        };
                        match self
                            .bearer_api(session, |bearer| {
                                let id = install.installation_id.clone();
                                let body = bind_body.clone();
                                async move {
                                    self.auth
                                        .api()
                                        .bind_apple_installation(&bearer, &id, &body)
                                        .await
                                }
                            })
                            .await
                        {
                            Ok(bound) => break Some(bound),
                            Err(BearerApiError::Api(error))
                                if is_apple_conflict(&error, PROBLEM_APPLE_REQUEST_CONFLICT)
                                    && request_conflicts_left > 0 =>
                            {
                                request_conflicts_left -= 1;
                                tracing::warn!(
                                    "apple push bind request_id conflict; resending with a new request_id"
                                );
                                continue;
                            }
                            Err(BearerApiError::Api(error))
                                if is_apple_conflict(&error, PROBLEM_APPLE_BINDING_CONFLICT)
                                    && stale_conflicts_left > 0 =>
                            {
                                stale_conflicts_left -= 1;
                                tracing::info!(
                                    "apple push binding changed; re-reading installation and retrying once"
                                );
                                break None;
                            }
                            Err(BearerApiError::Api(error))
                                if is_apple_conflict(&error, PROBLEM_APPLE_SCOPE_CONFLICT)
                                    && scope_rotations_left > 0 =>
                            {
                                scope_rotations_left -= 1;
                                tracing::info!(
                                    "apple push scope changed on bind; creating a new installation id"
                                );
                                match rotate_apple_installation(
                                    &self.config.data_dir,
                                    platform,
                                    bundle_id,
                                    environment,
                                ) {
                                    Ok(rotated) => {
                                        install = rotated;
                                        break None;
                                    }
                                    Err(detail) => {
                                        tracing::warn!(
                                            "apple push installation rotation failed: {detail}"
                                        );
                                        return;
                                    }
                                }
                            }
                            Err(error) => {
                                self.handle_apple_non_cas_error(session, generation, error);
                                return;
                            }
                        }
                    };
                    let Some(bound) = bound else {
                        continue;
                    };
                    let Some(binding_id) = bound.binding_id else {
                        tracing::warn!("apple push bind succeeded without binding_id");
                        self.finish_device_state(session, generation, DeviceState::Deferred);
                        return;
                    };
                    install.binding_id = Some(binding_id.clone());
                    install.registration_id = bound.registration_id.clone();
                    let _ = save_apple_installation(&self.config.data_dir, &install);
                    (binding_id, bound.registration_id)
                }
            };

            let registered = loop {
                let reg_body = AppleRegistrationRequest {
                    installation_secret: install.installation_secret.clone(),
                    binding_id: binding_id.clone(),
                    expected_registration_id: expected_registration_id.clone(),
                    request_id: uuid::Uuid::new_v4().to_string(),
                    token: token_hex.clone(),
                    app_version: clamp_chars(&self.config.app_version, 50),
                    locale: clamp_chars(&locale(), 20),
                    timezone: clamp_chars(&timezone(), 50),
                };
                match self
                    .bearer_api(session, |bearer| {
                        let id = install.installation_id.clone();
                        let body = reg_body.clone();
                        async move {
                            self.auth
                                .api()
                                .register_apple_device(&bearer, &id, &body)
                                .await
                        }
                    })
                    .await
                {
                    Ok(registered) => break Some(registered),
                    Err(BearerApiError::Api(error))
                        if is_apple_conflict(&error, PROBLEM_APPLE_TOKEN_OWNERSHIP_CONFLICT) =>
                    {
                        tracing::info!(
                            "apple push registration deferred (DEVICE_TOKEN_OWNERSHIP_CONFLICT): \
                             token belongs to another active session"
                        );
                        self.finish_device_state(session, generation, DeviceState::Deferred);
                        return;
                    }
                    Err(BearerApiError::Api(error))
                        if is_apple_conflict(&error, PROBLEM_APPLE_REQUEST_CONFLICT)
                            && request_conflicts_left > 0 =>
                    {
                        request_conflicts_left -= 1;
                        tracing::warn!(
                            "apple push registration request_id conflict; resending with a new request_id"
                        );
                        continue;
                    }
                    Err(BearerApiError::Api(error))
                        if (is_apple_conflict(&error, PROBLEM_APPLE_BINDING_CONFLICT)
                            || is_apple_conflict(&error, PROBLEM_APPLE_REGISTRATION_CONFLICT))
                            && stale_conflicts_left > 0 =>
                    {
                        stale_conflicts_left -= 1;
                        tracing::info!(
                            code = error.problem_code().unwrap_or(""),
                            "apple push CAS identifier stale; re-reading installation and retrying once"
                        );
                        break None;
                    }
                    Err(BearerApiError::Api(error))
                        if is_apple_conflict(&error, PROBLEM_APPLE_SCOPE_CONFLICT)
                            && scope_rotations_left > 0 =>
                    {
                        scope_rotations_left -= 1;
                        tracing::info!(
                            "apple push scope changed on register; creating a new installation id"
                        );
                        match rotate_apple_installation(
                            &self.config.data_dir,
                            platform,
                            bundle_id,
                            environment,
                        ) {
                            Ok(rotated) => {
                                install = rotated;
                                break None;
                            }
                            Err(detail) => {
                                tracing::warn!("apple push installation rotation failed: {detail}");
                                return;
                            }
                        }
                    }
                    Err(error) => {
                        self.handle_apple_non_cas_error(session, generation, error);
                        return;
                    }
                }
            };

            let Some(registered) = registered else {
                continue;
            };
            install.binding_id = Some(binding_id);
            install.registration_id = Some(registered.registration_id);
            if let Err(detail) = save_apple_installation(&self.config.data_dir, &install) {
                tracing::warn!("apple push installation save failed: {detail}");
            }
            tracing::info!(
                "apple push registration: device {} registered",
                registered.id
            );
            self.finish_device_state(session, generation, DeviceState::Registered);
            return;
        }
    }

    fn handle_apple_non_cas_error(&self, session: u64, generation: u64, error: BearerApiError) {
        match error {
            BearerApiError::Session(error) => {
                tracing::info!("apple push registration aborted: {error}");
            }
            BearerApiError::Api(error) => {
                if error.status() == Some(reqwest::StatusCode::FORBIDDEN)
                    && !matches!(
                        error.problem_code(),
                        Some(PROBLEM_APPLE_PROOF_INVALID | PROBLEM_APPLE_BINDING_CLOSED)
                    )
                {
                    tracing::info!(
                        "apple push registration refused (HTTP 403); retrying after the next token refresh"
                    );
                    self.finish_device_state(session, generation, DeviceState::AwaitingRefresh);
                } else if error.is_transient() {
                    tracing::info!("apple push registration failed transiently, retrying later");
                } else {
                    tracing::warn!(
                        status = error.status().map(|s| s.as_u16()).unwrap_or(0),
                        code = error.problem_code().unwrap_or(""),
                        "apple push registration stopped for this session"
                    );
                    self.finish_device_state(session, generation, DeviceState::Deferred);
                }
            }
        }
    }

    async fn register_device(&self, session: u64, generation: u64) {
        let official = official_push(&self.config.platform);
        let (installation, os_token) = if official {
            match read_os_token(&self.config.data_dir) {
                Some(os_token) => (String::new(), Some(os_token)),
                // Registered once the host hands the token over.
                None => return,
            }
        } else {
            match installation_id(&self.config.data_dir) {
                Ok(id) => (id, None),
                Err(detail) => {
                    tracing::warn!("push registration: installation id unavailable: {detail}");
                    return;
                }
            }
        };
        let body = DeviceRegistrationRequest {
            platform: push_platform(&self.config.platform),
            installation_id: (!official).then(|| installation.clone()),
            token: os_token.as_ref().map(|os_token| os_token.token.clone()),
            bundle_id: os_token.and_then(|os_token| os_token.bundle_id),
            app_version: self.config.app_version.clone(),
            locale: locale(),
            timezone: timezone(),
        };
        let body = &body;
        let result = self
            .bearer(
                session,
                |token| async move { self.auth.api().register_device(&token, body).await },
                ApiError::into_client_error,
            )
            .await;
        let registration = match result {
            Ok(registration) => registration,
            Err(error) if needs_new_token(&error) => {
                tracing::info!(
                    "push registration refused ({error}); retrying after the next token refresh"
                );
                self.finish_device_state(session, generation, DeviceState::AwaitingRefresh);
                return;
            }
            Err(error) => {
                tracing::info!("push registration failed, retrying later: {error}");
                return;
            }
        };
        if official {
            tracing::info!("push registration: device {} registered", registration.id);
            self.finish_device_state(session, generation, DeviceState::Registered);
            return;
        }
        let file = AgentFile {
            api_base: self.config.api_base.clone(),
            device_id: registration.id,
            push_token: registration.push_token,
            installation_id: installation,
        };
        if !self.session_is(session) {
            return;
        }
        let raw = serde_json::to_vec_pretty(&file).unwrap_or_default();
        match write_private(&agent_file_path(&self.config.data_dir), &raw) {
            Ok(()) => {
                tracing::info!("push registration: device {} registered", file.device_id);
                self.finish_device_state(session, generation, DeviceState::Registered);
            }
            Err(detail) => tracing::warn!("push registration: {detail}"),
        }
    }

    fn finish_device_state(&self, session: u64, generation: u64, device: DeviceState) {
        let mut state = self.session_state();
        if state.is_session(session) && self.push_generation.load(Ordering::SeqCst) == generation {
            state.device = device;
        }
    }

    /// Sign-out: unbinds the Apple installation or deletes / unregisters the
    /// device on the backend (best effort, while the session can still
    /// authenticate).
    pub(crate) async fn unregister_device(&self) {
        if apple_push(&self.config.platform) {
            let Ok(session) = self.current_session() else {
                return;
            };
            let Some(mut install) = read_apple_installation(&self.config.data_dir) else {
                return;
            };
            let Some(binding_id) = install.binding_id.take() else {
                return;
            };
            install.registration_id = None;
            let _ = save_apple_installation(&self.config.data_dir, &install);
            let installation_id = install.installation_id;
            let secret = install.installation_secret;
            let unbound = tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.bearer_api(session, |token| {
                    let id = installation_id.clone();
                    let bid = binding_id.clone();
                    let sec = secret.clone();
                    async move {
                        self.auth
                            .api()
                            .unbind_apple_installation(&token, &id, &bid, &sec)
                            .await
                    }
                }),
            )
            .await;
            match unbound {
                Ok(Ok(())) => tracing::info!("apple push registration: installation unbound"),
                Ok(Err(BearerApiError::Api(error))) => tracing::info!(
                    status = error.status().map(|s| s.as_u16()).unwrap_or(0),
                    code = error.problem_code().unwrap_or(""),
                    "apple push unbind failed"
                ),
                Ok(Err(BearerApiError::Session(error))) => {
                    tracing::info!("apple push unbind skipped: {error}")
                }
                Err(_) => tracing::info!("apple push unbind timed out"),
            }
            return;
        }
        if official_push(&self.config.platform) {
            let Ok(session) = self.current_session() else {
                return;
            };
            let registered = self.session_state().device == DeviceState::Registered;
            if let (true, Some(os_token)) = (registered, read_os_token(&self.config.data_dir)) {
                self.unregister_os_token(session, &os_token.token).await;
            }
            return;
        }
        let Some(file) = read_agent_file(&self.config.data_dir) else {
            return;
        };
        let Ok(session) = self.current_session() else {
            return;
        };
        let id = file.device_id;
        let deleted = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.bearer(
                session,
                |token| async move { self.auth.api().delete_device(&token, id).await },
                ApiError::into_client_error,
            ),
        )
        .await;
        match deleted {
            Ok(Ok(())) => tracing::info!("push registration: device {id} deleted"),
            Ok(Err(error)) => tracing::info!("push registration: delete failed: {error}"),
            Err(_) => tracing::info!("push registration: delete timed out"),
        }
    }

    /// Disables the device holding the FCM `push_token` (best effort).
    async fn unregister_os_token(&self, session: u64, push_token: &str) {
        let unregistered = tokio::time::timeout(
            std::time::Duration::from_secs(5),
            self.bearer(
                session,
                |token| async move {
                    self.auth
                        .api()
                        .unregister_push_token(&token, push_token)
                        .await
                },
                ApiError::into_client_error,
            ),
        )
        .await;
        match unregistered {
            Ok(Ok(())) => tracing::info!("push registration: device unregistered"),
            Ok(Err(error)) => tracing::info!("push registration: unregister failed: {error}"),
            Err(_) => tracing::info!("push registration: unregister timed out"),
        }
    }
}

#[cfg(test)]
#[path = "device/apple_tests.rs"]
mod apple_tests;

#[cfg(test)]
mod tests {
    use super::*;

    use ppvpn_account::device::INSTALLATION_FILE;

    #[test]
    fn installation_id_survives_sign_out() {
        let dir = crate::test_backend::temp_dir("ppvpn-install-test");
        let first = installation_id(&dir).unwrap();
        // Survives removing the agent files (sign-out).
        remove_agent_files(&dir);
        assert_eq!(installation_id(&dir).unwrap(), first);
        let _ = std::fs::remove_dir_all(dir);
    }

    use crate::auth::test_support::MemoryPlatform;
    use crate::cores::test_support::{FakeLauncher, NoService};
    use crate::test_backend::{serve, temp_dir, wait_until, Backend, PUSH_TOKEN};
    use crate::{ClientConfig, ClientListener, ClientSnapshot, ProbeResult, TrafficSample};
    use std::sync::Arc;

    struct Quiet;

    impl ClientListener for Quiet {
        fn on_snapshot(&self, _: ClientSnapshot) {}
        fn on_probe_result(&self, _: ProbeResult) {}
        fn on_traffic(&self, _: TrafficSample) {}
    }

    fn client(base: &str, data_dir: &str) -> Arc<Client> {
        client_on(base, data_dir, "windows")
    }

    fn client_on(base: &str, data_dir: &str, platform: &str) -> Arc<Client> {
        Client::with_parts(
            ClientConfig {
                api_base: base.to_string(),
                data_dir: data_dir.to_string(),
                log_dir: data_dir.to_string(),
                platform: platform.into(),
                app_version: "1.2.3".into(),
            },
            MemoryPlatform::with_blob(r#"{"version":1,"active_refresh":"jyr_old"}"#),
            Arc::new(Quiet),
            Arc::new(NoService),
            FakeLauncher::new(),
            Arc::new(crate::sysproxy::tests::FakeWriter::default()),
        )
    }

    fn block_on<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }

    #[test]
    fn sign_in_registers_and_sign_out_unregisters() {
        let backend = Backend::new();
        let base = serve(backend.clone());
        let dir = temp_dir("ppvpn-device-test");
        let client = client(&base, &dir);

        wait_until(|| read_agent_file(&dir).is_some());
        let file = read_agent_file(&dir).unwrap();
        assert_eq!(file.device_id, 42);
        assert_eq!(file.push_token, PUSH_TOKEN);
        assert_eq!(file.api_base, base);
        assert_eq!(file.installation_id, installation_id(&dir).unwrap());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(agent_file_path(&dir))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        assert_eq!(
            backend.requests_matching("/devices/register"),
            vec!["POST /api/v1/devices/register"]
        );
        std::fs::write(Path::new(&dir).join(AGENT_CURSOR_FILE), "{}").unwrap();
        // What the push agent recorded resolves a notification click.
        let push = crate::PushMessage {
            id: 17,
            message_id: None,
            title: "t".into(),
            body: "b".into(),
            severity: crate::MessageSeverity::Normal,
            category: crate::MessageCategory::Announcement,
            event_key: "broadcast:3".into(),
            deep_link: None,
            created_at: "2026-09-29T00:00:00Z".into(),
        };
        crate::push_shown::record(Path::new(&dir), &push).unwrap();
        assert_eq!(client.shown_push(17), Some(push));
        assert_eq!(client.shown_push(18), None);

        block_on(client.logout()).unwrap();
        assert_eq!(
            backend.requests_matching("DELETE"),
            vec!["DELETE /api/v1/devices/42"]
        );
        assert!(read_agent_file(&dir).is_none());
        assert!(!Path::new(&dir).join(AGENT_CURSOR_FILE).exists());
        assert!(!Path::new(&dir).join(crate::push_shown::SHOWN_FILE).exists());
        assert_eq!(client.shown_push(17), None);
        assert!(Path::new(&dir).join(INSTALLATION_FILE).exists());
        client.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn android_registers_its_os_token_and_unregisters_it_at_sign_out() {
        let backend = Backend::new();
        let base = serve(backend.clone());
        let dir = temp_dir("ppvpn-device-test");
        let client = client_on(&base, &dir, "android");
        let bodies = || backend.device_bodies.lock().unwrap().clone();

        // Signed in, but no OS token yet: nothing to register.
        wait_until(|| client.snapshot().auth == crate::AuthState::SignedIn);
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert!(backend.requests_matching("/devices/").is_empty());

        client.set_push_token("fcm-1".into(), Some("com.example.app".into()));
        wait_until(|| !bodies().is_empty());
        let body = &bodies()[0];
        assert_eq!(body["platform"], "android");
        assert_eq!(body["token"], "fcm-1");
        assert_eq!(body["bundle_id"], "com.example.app");
        assert!(body.get("installation_id").is_none());
        assert!(read_agent_file(&dir).is_none(), "no push agent on Android");
        wait_until(|| client.session_state().device == DeviceState::Registered);

        // The same token again changes nothing.
        client.set_push_token("fcm-1".into(), Some("com.example.app".into()));
        std::thread::sleep(std::time::Duration::from_millis(300));
        assert_eq!(bodies().len(), 1);

        // A new token: the old one is unregistered, the new one registered.
        client.set_push_token("fcm-2".into(), None);
        wait_until(|| bodies().len() == 3);
        assert_eq!(bodies()[1], serde_json::json!({"token": "fcm-1"}));
        assert_eq!(bodies()[2]["token"], "fcm-2");
        assert!(bodies()[2].get("bundle_id").is_none());
        wait_until(|| client.session_state().device == DeviceState::Registered);

        block_on(client.logout()).unwrap();
        assert_eq!(bodies()[3], serde_json::json!({"token": "fcm-2"}));
        assert!(backend.requests_matching("DELETE").is_empty());
        // The token is the device's: it stays for the next sign-in.
        assert_eq!(read_os_token(&dir).unwrap().token, "fcm-2");
        client.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_token_without_the_scope_waits_for_the_next_refresh() {
        let backend = Backend::new();
        *backend.register_status.lock().unwrap() = Some("403 Forbidden");
        let base = serve(backend.clone());
        let dir = temp_dir("ppvpn-device-test");
        let client = client(&base, &dir);
        wait_until(|| !backend.requests_matching("/devices/register").is_empty());
        std::thread::sleep(std::time::Duration::from_millis(200));
        assert_eq!(backend.requests_matching("/devices/register").len(), 1);
        assert!(read_agent_file(&dir).is_none());
        assert!(client.snapshot().last_error.is_none());
        client.shutdown();
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn platform_and_environment_values() {
        assert!(!locale().is_empty());
        assert!(!timezone().is_empty());
    }
}
