//! Device registration for push: the install's stable id, the
//! `POST /devices/register` request, the Apple per-installation binding
//! state and the registration state of a signed-in session (#539).
//!
//! Windows and Linux installs register with a stable `installation_id`
//! (`<data_dir>/installation.json`, kept across sign-outs) and get the
//! self-hosted channel's push token back; what the host does with it (the
//! desktop hands it to its push agent) stays with the host. Android registers
//! its FCM token through `POST /devices/register`, while iOS and macOS bind
//! an installation proof (`<data_dir>/apple-push-installation.json`) to the
//! active login session through `/devices/apple/installations` (#539).

use std::path::Path;

use base64::Engine;
use serde::{Deserialize, Serialize};

/// Holds the installation id in the host's data directory.
pub const INSTALLATION_FILE: &str = "installation.json";
/// Holds the Apple per-installation push proof and latest CAS identifiers.
pub const APPLE_INSTALLATION_FILE: &str = "apple-push-installation.json";

pub const PROBLEM_APPLE_TOKEN_OWNERSHIP_CONFLICT: &str = "DEVICE_TOKEN_OWNERSHIP_CONFLICT";
pub const PROBLEM_APPLE_BINDING_CONFLICT: &str = "DEVICE_BINDING_CONFLICT";
pub const PROBLEM_APPLE_REGISTRATION_CONFLICT: &str = "DEVICE_REGISTRATION_CONFLICT";
pub const PROBLEM_APPLE_REQUEST_CONFLICT: &str = "DEVICE_REQUEST_CONFLICT";
pub const PROBLEM_APPLE_SCOPE_CONFLICT: &str = "DEVICE_INSTALLATION_SCOPE_CONFLICT";
pub const PROBLEM_APPLE_PROOF_INVALID: &str = "DEVICE_INSTALLATION_PROOF_INVALID";
pub const PROBLEM_APPLE_BINDING_CLOSED: &str = "DEVICE_BINDING_CLOSED";
pub const PROBLEM_APPLE_NOT_CONFIGURED: &str = "DEVICE_APPLE_NOT_CONFIGURED";

/// Registration progress of the signed-in session.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DeviceState {
    /// Not registered yet (or the last attempt failed transiently).
    #[default]
    Pending,
    Registered,
    /// Refused because the access token lacks `device:register`; retried
    /// after the next access-token refresh.
    AwaitingRefresh,
    /// Stopped for the current launch / OS token (for example
    /// `DEVICE_TOKEN_OWNERSHIP_CONFLICT` or an unconfigured Apple backend);
    /// retried on the next launch, sign-in or OS token rotation.
    Deferred,
}

/// The `POST /devices/register` body (Windows, Linux and Android).
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct DeviceRegistrationRequest {
    /// [`push_platform`] of the host platform.
    pub platform: String,
    /// Desktop (self-hosted channel) only.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub installation_id: Option<String>,
    /// The FCM registration token (official non-Apple channel).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub token: Option<String>,
    /// The app's bundle id / package name, when the host gives one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bundle_id: Option<String>,
    pub app_version: String,
    /// Language tag, e.g. `zh-CN`.
    pub locale: String,
    /// IANA time zone, e.g. `Asia/Shanghai`.
    pub timezone: String,
}

/// `GET /devices/apple/config` answer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct ApplePushConfig {
    #[serde(default)]
    pub configured: bool,
    #[serde(default)]
    pub bundle_id_ios: String,
    #[serde(default)]
    pub bundle_id_macos: String,
    #[serde(default)]
    pub environments: Vec<String>,
}

impl ApplePushConfig {
    /// Configured APNs topic (`bundle_id_ios` or `bundle_id_macos`) for `platform`.
    pub fn topic(&self, platform: &str) -> Option<&str> {
        let topic = match platform {
            "ios" => self.bundle_id_ios.as_str(),
            "macos" => self.bundle_id_macos.as_str(),
            _ => return None,
        };
        (!topic.is_empty()).then_some(topic)
    }

    /// True when the backend has Apple push ready for `platform` and `environment`.
    pub fn supports(&self, platform: &str, environment: &str) -> bool {
        self.configured
            && self.topic(platform).is_some()
            && self.environments.iter().any(|item| item == environment)
    }
}

/// `POST /devices/apple/installations` body.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppleInstallationRequest {
    pub installation_id: String,
    pub installation_secret: String,
    pub platform: String,
    pub bundle_id: String,
    pub environment: String,
}

/// `POST /devices/apple/installations` and `.../bindings` answer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleInstallationResponse {
    pub installation_id: String,
    #[serde(default)]
    pub binding_id: Option<String>,
    #[serde(default)]
    pub registration_id: Option<String>,
    #[serde(default)]
    pub bound_to_current_user: bool,
    #[serde(default)]
    pub bound_to_current_session: bool,
}

/// `POST /devices/apple/installations/{id}/bindings` body.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppleBindingRequest {
    pub installation_secret: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_binding_id: Option<String>,
    pub request_id: String,
}

/// `POST /devices/apple/installations/{id}/registrations` body.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppleRegistrationRequest {
    pub installation_secret: String,
    pub binding_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_registration_id: Option<String>,
    pub request_id: String,
    pub token: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub app_version: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub locale: String,
    #[serde(skip_serializing_if = "String::is_empty")]
    pub timezone: String,
}

/// `POST /devices/apple/installations/{id}/registrations` answer.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AppleRegistrationResponse {
    pub id: u64,
    pub registration_id: String,
    #[serde(default)]
    pub token_hash_prefix: String,
}

/// Persisted per-installation Apple push proof and last observed CAS IDs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AppleInstallationState {
    pub installation_id: String,
    pub installation_secret: String,
    pub platform: String,
    pub bundle_id: String,
    pub environment: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub binding_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub registration_id: Option<String>,
}

/// `ios` / `macos` / `android` for APNs / FCM; Windows and Linux use
/// the `desktop-*` self-hosted channel.
pub fn push_platform(platform: &str) -> String {
    if official_push(platform) {
        platform.to_string()
    } else {
        format!("desktop-{platform}")
    }
}

/// The platform registers an OS push token (APNs, FCM) rather than a
/// self-hosted push agent.
pub fn official_push(platform: &str) -> bool {
    matches!(platform, "ios" | "macos" | "android")
}

/// The platform uses the Apple per-installation push binding endpoints.
pub fn apple_push(platform: &str) -> bool {
    matches!(platform, "ios" | "macos")
}

/// Maps an entitlement or wire APNs environment string to `sandbox` / `production`.
pub fn apple_environment(raw: Option<&str>) -> Option<&'static str> {
    match raw.map(str::trim) {
        Some("development" | "sandbox") => Some("sandbox"),
        Some("production") => Some("production"),
        _ => None,
    }
}

/// Normalizes an APNs hex device token (`2..=4096` even-length hex digits).
pub fn normalize_apple_token(raw: &str) -> Option<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty()
        || trimmed.len() > 4096
        || !trimmed.len().is_multiple_of(2)
        || !trimmed.bytes().all(|b| b.is_ascii_hexdigit())
    {
        return None;
    }
    Some(trimmed.to_ascii_lowercase())
}

/// Validates a backend push delivery identifier (`^push:[1-9][0-9]*$`, max 32 chars).
pub fn valid_apple_delivery_id(raw: &str) -> bool {
    let Some(digits) = raw.strip_prefix("push:") else {
        return false;
    };
    raw.len() <= 32
        && !digits.is_empty()
        && !digits.starts_with('0')
        && digits.bytes().all(|b| b.is_ascii_digit())
        && digits.parse::<u64>().is_ok_and(|id| id > 0)
}

pub fn valid_non_nil_uuid(raw: &str) -> bool {
    uuid::Uuid::parse_str(raw).is_ok_and(|id| !id.is_nil())
}

fn valid_installation_secret(secret: &str) -> bool {
    secret.len() == 43
        && base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(secret)
            .is_ok_and(|bytes| bytes.len() == 32)
}

fn new_installation_secret() -> Result<String, String> {
    let mut bytes = [0u8; 32];
    tokio_rustls::rustls::crypto::ring::default_provider()
        .secure_random
        .fill(&mut bytes)
        .map_err(|_| "system random unavailable".to_string())?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes))
}

fn valid_apple_installation(state: &AppleInstallationState) -> bool {
    valid_non_nil_uuid(&state.installation_id)
        && valid_installation_secret(&state.installation_secret)
        && apple_push(&state.platform)
        && !state.bundle_id.is_empty()
        && state.bundle_id.len() <= 200
        && matches!(state.environment.as_str(), "sandbox" | "production")
        && state.binding_id.as_deref().is_none_or(valid_non_nil_uuid)
        && state
            .registration_id
            .as_deref()
            .is_none_or(valid_non_nil_uuid)
}

/// Reads the saved Apple installation record when valid.
pub fn read_apple_installation(data_dir: &str) -> Option<AppleInstallationState> {
    let path = Path::new(data_dir).join(APPLE_INSTALLATION_FILE);
    std::fs::read(&path)
        .ok()
        .and_then(|raw| serde_json::from_slice::<AppleInstallationState>(&raw).ok())
        .filter(valid_apple_installation)
}

/// Persists `state` atomically with `0600` permissions.
pub fn save_apple_installation(
    data_dir: &str,
    state: &AppleInstallationState,
) -> Result<(), String> {
    if !valid_apple_installation(state) {
        return Err("invalid Apple push installation state".into());
    }
    let path = Path::new(data_dir).join(APPLE_INSTALLATION_FILE);
    let raw = serde_json::to_vec(state).map_err(|error| error.to_string())?;
    write_private(&path, &raw)
}

/// Creates and persists a fresh `installation_id` and 32-byte base64url proof.
pub fn rotate_apple_installation(
    data_dir: &str,
    platform: &str,
    bundle_id: &str,
    environment: &str,
) -> Result<AppleInstallationState, String> {
    let state = AppleInstallationState {
        installation_id: uuid::Uuid::new_v4().to_string(),
        installation_secret: new_installation_secret()?,
        platform: platform.to_string(),
        bundle_id: bundle_id.to_string(),
        environment: environment.to_string(),
        binding_id: None,
        registration_id: None,
    };
    save_apple_installation(data_dir, &state)?;
    Ok(state)
}

/// Loads the saved Apple installation proof, or creates a new one if absent,
/// corrupt, or scoped to a different `(platform, bundle_id, environment)`.
pub fn load_or_create_apple_installation(
    data_dir: &str,
    platform: &str,
    bundle_id: &str,
    environment: &str,
) -> Result<AppleInstallationState, String> {
    if let Some(saved) = read_apple_installation(data_dir) {
        if saved.platform == platform
            && saved.bundle_id == bundle_id
            && saved.environment == environment
        {
            return Ok(saved);
        }
    }
    rotate_apple_installation(data_dir, platform, bundle_id, environment)
}

#[derive(Serialize, Deserialize)]
struct InstallationFile {
    installation_id: String,
}

/// Writes `bytes` to `path` atomically (temp file + rename), private to the
/// user on Unix (0600); Windows inherits the profile directory's ACL.
pub fn write_private(path: &Path, bytes: &[u8]) -> Result<(), String> {
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
pub fn installation_id(data_dir: &str) -> Result<String, String> {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(prefix: &str) -> String {
        let dir = std::env::temp_dir().join(format!("{prefix}-{}", uuid::Uuid::new_v4().simple()));
        std::fs::create_dir_all(&dir).unwrap();
        dir.to_string_lossy().into_owned()
    }

    #[test]
    fn installation_id_is_stable_and_private() {
        let dir = temp_dir("ppvpn-install-test");
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
        // An unusable file is replaced by a new id.
        std::fs::write(
            Path::new(&dir).join(INSTALLATION_FILE),
            r#"{"installation_id":"not-a-uuid"}"#,
        )
        .unwrap();
        let second = installation_id(&dir).unwrap();
        assert_ne!(second, first);
        assert!(uuid::Uuid::parse_str(&second).is_ok());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn private_writes_replace_the_file_and_leave_no_temp_files() {
        let dir = temp_dir("ppvpn-write-test");
        let path = Path::new(&dir).join("nested").join("file.json");
        write_private(&path, b"one").unwrap();
        write_private(&path, b"two").unwrap();
        assert_eq!(std::fs::read(&path).unwrap(), b"two");
        let entries = std::fs::read_dir(path.parent().unwrap()).unwrap().count();
        assert_eq!(entries, 1);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn registration_body_and_platform() {
        assert_eq!(push_platform("windows"), "desktop-windows");
        assert_eq!(push_platform("linux"), "desktop-linux");
        assert_eq!(push_platform("ios"), "ios");
        assert_eq!(push_platform("macos"), "macos");
        assert_eq!(push_platform("android"), "android");
        assert!(apple_push("ios"));
        assert!(apple_push("macos"));
        assert!(!apple_push("android"));
        assert!(!apple_push("windows"));
        let body = serde_json::to_value(DeviceRegistrationRequest {
            platform: push_platform("windows"),
            installation_id: Some("id".into()),
            token: None,
            bundle_id: None,
            app_version: "1.2.3".into(),
            locale: "zh-CN".into(),
            timezone: "UTC".into(),
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "platform": "desktop-windows",
                "installation_id": "id",
                "app_version": "1.2.3",
                "locale": "zh-CN",
                "timezone": "UTC",
            })
        );
        let body = serde_json::to_value(DeviceRegistrationRequest {
            platform: push_platform("android"),
            installation_id: None,
            token: Some("fcm-token".into()),
            bundle_id: Some("com.example.app".into()),
            app_version: "1.2.3".into(),
            locale: "zh-CN".into(),
            timezone: "UTC".into(),
        })
        .unwrap();
        assert_eq!(
            body,
            serde_json::json!({
                "platform": "android",
                "token": "fcm-token",
                "bundle_id": "com.example.app",
                "app_version": "1.2.3",
                "locale": "zh-CN",
                "timezone": "UTC",
            })
        );
    }

    #[test]
    fn apple_installation_proof_is_stable_private_and_rotates_on_scope_change() {
        let dir = temp_dir("ppvpn-apple-install-test");
        let first =
            load_or_create_apple_installation(&dir, "ios", "com.peakpassvpn.ppvpn", "production")
                .unwrap();
        assert!(valid_non_nil_uuid(&first.installation_id));
        assert!(valid_installation_secret(&first.installation_secret));
        let mut updated = first.clone();
        updated.binding_id = Some(uuid::Uuid::new_v4().to_string());
        updated.registration_id = Some(uuid::Uuid::new_v4().to_string());
        save_apple_installation(&dir, &updated).unwrap();

        let reloaded =
            load_or_create_apple_installation(&dir, "ios", "com.peakpassvpn.ppvpn", "production")
                .unwrap();
        assert_eq!(reloaded, updated);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(Path::new(&dir).join(APPLE_INSTALLATION_FILE))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o777, 0o600);
        }

        // Changing environment or bundle creates a fresh installation proof.
        let switched =
            load_or_create_apple_installation(&dir, "ios", "com.peakpassvpn.ppvpn", "sandbox")
                .unwrap();
        assert_ne!(switched.installation_id, first.installation_id);
        assert_ne!(switched.installation_secret, first.installation_secret);
        assert_eq!(switched.environment, "sandbox");
        assert!(switched.binding_id.is_none());
        assert!(switched.registration_id.is_none());

        // Corrupt content is replaced cleanly.
        std::fs::write(Path::new(&dir).join(APPLE_INSTALLATION_FILE), b"{corrupt").unwrap();
        let recovered =
            load_or_create_apple_installation(&dir, "ios", "com.peakpassvpn.ppvpn", "sandbox")
                .unwrap();
        assert_ne!(recovered.installation_id, switched.installation_id);
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn apple_token_environment_and_delivery_id_validators() {
        assert_eq!(apple_environment(Some("development")), Some("sandbox"));
        assert_eq!(apple_environment(Some("sandbox")), Some("sandbox"));
        assert_eq!(apple_environment(Some("production")), Some("production"));
        assert_eq!(apple_environment(Some("staging")), None);
        assert_eq!(apple_environment(None), None);

        assert_eq!(
            normalize_apple_token(" 01AbCdEf "),
            Some("01abcdef".to_string())
        );
        assert_eq!(normalize_apple_token(""), None);
        assert_eq!(normalize_apple_token("abc"), None);
        assert_eq!(normalize_apple_token("zz11"), None);

        assert!(valid_apple_delivery_id("push:1"));
        assert!(valid_apple_delivery_id("push:987654321"));
        assert!(!valid_apple_delivery_id("push:0"));
        assert!(!valid_apple_delivery_id("push:01"));
        assert!(!valid_apple_delivery_id("push:"));
        assert!(!valid_apple_delivery_id("push:1/../../users/me"));
        assert!(!valid_apple_delivery_id("42"));
    }
}
