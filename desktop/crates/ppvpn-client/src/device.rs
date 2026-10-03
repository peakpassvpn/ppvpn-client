//! Desktop push registration (main-app side of the push agent).
//!
//! Each install has a stable `installation_id` (`<data_dir>/installation.json`,
//! kept across sign-outs). While signed in, the device is registered with the
//! backend on sign-in and on every launch; the returned push token goes to
//! `<data_dir>/push-agent.json`, which the separate push agent
//! ([`crate::PushAgent`]) watches. Sign-out deletes the device (best effort)
//! and the agent files (including the pushes recorded for notification
//! clicks).

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::api::ApiError;
use crate::api::ApiErrorExt;
use crate::errors::ClientError;
use crate::session::ClientRef;
use crate::Client;

const INSTALLATION_FILE: &str = "installation.json";
pub(crate) const AGENT_FILE: &str = "push-agent.json";
pub(crate) const AGENT_CURSOR_FILE: &str = "push-agent-cursor.json";

/// What the push agent needs; written by the main app only.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub(crate) struct AgentFile {
    pub api_base: String,
    pub device_id: u64,
    pub push_token: String,
    pub installation_id: String,
}

/// Registration progress of the signed-in session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) enum DeviceState {
    /// Not registered yet (or the last attempt failed transiently).
    #[default]
    Pending,
    Registered,
    /// Refused because the access token lacks `device:register`; retried
    /// after the next access-token refresh.
    AwaitingRefresh,
}

#[derive(Serialize, Deserialize)]
struct InstallationFile {
    installation_id: String,
}

/// Writes `bytes` to `path` atomically (temp file + rename), private to the
/// user on Unix (0600); Windows inherits the profile directory's ACL.
pub(crate) fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
    let dir = path.parent().ok_or("no parent directory")?;
    std::fs::create_dir_all(dir).map_err(|error| format!("create {}: {error}", dir.display()))?;
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    let result = (|| {
        #[cfg(unix)]
        {
            use std::io::Write;
            use std::os::unix::fs::OpenOptionsExt;
            let mut file = std::fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .mode(0o600)
                .open(&tmp)?;
            file.write_all(bytes)?;
            file.sync_all()?;
        }
        #[cfg(not(unix))]
        std::fs::write(&tmp, bytes)?;
        std::fs::rename(&tmp, path)
    })();
    result.map_err(|error: std::io::Error| {
        let _ = std::fs::remove_file(&tmp);
        format!("write {}: {error}", path.display())
    })
}

/// The stable id of this install, created on first use.
pub(crate) fn installation_id(data_dir: &str) -> Result<String, String> {
    let path = Path::new(data_dir).join(INSTALLATION_FILE);
    if let Some(id) = std::fs::read(&path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<InstallationFile>(&raw).ok())
        .map(|file| file.installation_id)
        .filter(|id| uuid::Uuid::parse_str(id).is_ok())
    {
        return Ok(id);
    }
    let id = uuid::Uuid::new_v4().to_string();
    let raw = serde_json::to_vec(&InstallationFile {
        installation_id: id.clone(),
    })
    .map_err(|error| error.to_string())?;
    write_private(&path, &raw)?;
    Ok(id)
}

pub(crate) fn agent_file_path(data_dir: &str) -> PathBuf {
    Path::new(data_dir).join(AGENT_FILE)
}

pub(crate) fn read_agent_file(data_dir: &str) -> Option<AgentFile> {
    let raw = std::fs::read(agent_file_path(data_dir)).ok()?;
    serde_json::from_slice(&raw).ok()
}

/// Sign-out: the agent must stop pulling for this account, and its pushes
/// no longer resolve notification clicks.
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

/// `desktop-macos` / `desktop-windows` / `desktop-linux`.
pub(crate) fn push_platform(platform: &str) -> String {
    format!("desktop-{platform}")
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
    /// Registers when the session wants it; called by the refresh loop.
    pub(crate) async fn ensure_device_registered(&self, session: u64) {
        let wanted = {
            let state = self.session_state();
            state.is_session(session) && state.device == DeviceState::Pending
        };
        if wanted {
            self.register_device(session).await;
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

    async fn register_device(&self, session: u64) {
        let installation = match installation_id(&self.config.data_dir) {
            Ok(id) => id,
            Err(detail) => {
                tracing::warn!("push registration: installation id unavailable: {detail}");
                return;
            }
        };
        let body = serde_json::json!({
            "platform": push_platform(&self.config.platform),
            "installation_id": installation,
            "app_version": self.config.app_version,
            "locale": locale(),
            "timezone": timezone(),
        });
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
                self.set_device_state(session, DeviceState::AwaitingRefresh);
                return;
            }
            Err(error) => {
                tracing::info!("push registration failed, retrying later: {error}");
                return;
            }
        };
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
                self.set_device_state(session, DeviceState::Registered);
            }
            Err(detail) => tracing::warn!("push registration: {detail}"),
        }
    }

    fn set_device_state(&self, session: u64, device: DeviceState) {
        let mut state = self.session_state();
        if state.is_session(session) {
            state.device = device;
        }
    }

    /// Sign-out: deletes the device on the backend (best effort, while the
    /// session can still authenticate).
    pub(crate) async fn unregister_device(&self) {
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
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn installation_id_is_stable_and_private() {
        let dir = crate::test_backend::temp_dir("ppvpn-install-test");
        let first = installation_id(&dir).unwrap();
        assert_eq!(installation_id(&dir).unwrap(), first);
        assert!(uuid::Uuid::parse_str(&first).is_ok());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Path::new(&dir).join(INSTALLATION_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }
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
        Client::with_parts(
            ClientConfig {
                api_base: base.to_string(),
                data_dir: data_dir.to_string(),
                log_dir: data_dir.to_string(),
                core_bin_dir: data_dir.to_string(),
                platform: "windows".into(),
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
        assert_eq!(push_platform("windows"), "desktop-windows");
        assert!(!locale().is_empty());
        assert!(!timezone().is_empty());
    }
}
