//! Browser device login and credential persistence: [`ppvpn_account::auth`]
//! wired to this client's platform hooks, product audience and error codes.

use std::sync::Arc;

pub(crate) use ppvpn_account::auth::{AccessToken, Auth, AuthError, SignedIn};
use ppvpn_account::auth::{
    AuthConfig, CredentialStore, StoreError, StoreFailure, VERIFICATION_PATHS,
};

use crate::api::{Api, ApiErrorExt, PRODUCT_AUDIENCE};
use crate::errors::{ClientError, ErrorCode};
use crate::{PlatformError, PlatformHooks};

/// Production verification page; any other host is rejected outside debug
/// builds (the configured API base's own host aside).
const VERIFICATION_HOST: &str = "www.peakpassvpn.com";

/// Device name shown in the user's device list (and the admin broadcast
/// picker): the app and the OS, so a user's machines can be told apart
/// without sending anything personal such as the host name.
pub(crate) fn device_name(platform: &str) -> String {
    let os = match platform.split(['-', '/']).next().unwrap_or("") {
        "macos" => "macOS",
        "windows" => "Windows",
        "linux" => "Linux",
        _ => return "PPVPN Desktop".to_string(),
    };
    format!("PPVPN Desktop · {os}")
}

/// The device-login coordinator for this client.
pub(crate) fn client_auth(api: Arc<Api>, platform: Arc<dyn PlatformHooks>) -> Auth {
    Auth::new(
        api,
        Arc::new(PlatformStore(platform)),
        AuthConfig {
            verification_host: VERIFICATION_HOST.to_string(),
            verification_paths: VERIFICATION_PATHS.iter().map(|p| p.to_string()).collect(),
        },
    )
}

/// [`ppvpn_account::auth::access_token_from`] for the desktop audience.
pub(crate) fn access_token_from(
    token: String,
    expires_in: Option<u64>,
) -> Result<AccessToken, AuthError> {
    ppvpn_account::auth::access_token_from(token, expires_in, PRODUCT_AUDIENCE)
}

/// The platform credential hooks as the account crate's store.
struct PlatformStore(Arc<dyn PlatformHooks>);

fn store_failure(error: PlatformError) -> StoreFailure {
    StoreFailure {
        locked: matches!(error, PlatformError::Locked { .. }),
        message: error.to_string(),
    }
}

impl CredentialStore for PlatformStore {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreFailure> {
        self.0.credential_load().map_err(store_failure)
    }
    fn save(&self, blob: Vec<u8>) -> Result<(), StoreFailure> {
        self.0.credential_save(blob).map_err(store_failure)
    }
    fn delete(&self) -> Result<(), StoreFailure> {
        self.0.credential_delete().map_err(store_failure)
    }
}

/// The client error code of a failed credential-store operation.
pub(crate) trait StoreErrorExt {
    fn code(&self) -> ErrorCode;
}

impl StoreErrorExt for StoreError {
    fn code(&self) -> ErrorCode {
        if self.locked {
            ErrorCode::CredentialStoreLocked
        } else {
            ErrorCode::CredentialStoreFailed
        }
    }
}

/// [`AuthError`] mapped to the client's error codes.
pub(crate) trait AuthErrorExt {
    fn into_client_error(self) -> ClientError;
}

impl AuthErrorExt for AuthError {
    fn into_client_error(self) -> ClientError {
        match self {
            Self::Api(error) => error.into_auth_error(),
            Self::Store(error) => ClientError::failed(error.code(), error.detail),
            Self::Cancelled => ClientError::Cancelled,
            Self::Denied => ClientError::failed(ErrorCode::AuthDenied, "access_denied"),
            Self::Expired => ClientError::failed(ErrorCode::AuthExpired, "expired_token"),
            Self::Untrusted(detail) => {
                ClientError::failed(ErrorCode::AuthUntrustedResponse, detail)
            }
            Self::NoCredential => {
                ClientError::failed(ErrorCode::AuthSessionInvalid, "no saved desktop login")
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Test support
// ---------------------------------------------------------------------------

#[cfg(test)]
pub(crate) mod test_support {
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};

    use crate::{PlatformError, PlatformHooks};

    /// In-memory platform with a credential store.
    #[derive(Default)]
    pub(crate) struct MemoryPlatform {
        pub(crate) blob: Mutex<Option<Vec<u8>>>,
        pub(crate) opened: Mutex<Vec<String>>,
        /// How long the uninstall hook takes (admin prompt, service stop).
        pub(crate) uninstall_delay: Mutex<Option<std::time::Duration>>,
    }

    impl MemoryPlatform {
        pub(crate) fn with_blob(json: &str) -> Arc<Self> {
            let platform = Self::default();
            *platform.blob.lock().unwrap() = Some(json.as_bytes().to_vec());
            Arc::new(platform)
        }

        pub(crate) fn stored(&self) -> Option<serde_json::Value> {
            let blob = self.blob.lock().unwrap();
            blob.as_ref()
                .map(|raw| serde_json::from_slice(raw).unwrap())
        }
    }

    impl PlatformHooks for MemoryPlatform {
        fn credential_load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
            Ok(self.blob.lock().unwrap().clone())
        }
        fn credential_save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
            *self.blob.lock().unwrap() = Some(blob);
            Ok(())
        }
        fn credential_delete(&self) -> Result<(), PlatformError> {
            *self.blob.lock().unwrap() = None;
            Ok(())
        }
        fn open_url(&self, url: String) -> bool {
            self.opened.lock().unwrap().push(url);
            true
        }
        fn privileged_service_installed(&self) -> bool {
            false
        }
        fn install_privileged_service(&self) -> Result<(), PlatformError> {
            Ok(())
        }
        fn uninstall_privileged_service(&self) -> Result<(), PlatformError> {
            let delay = *self.uninstall_delay.lock().unwrap();
            if let Some(delay) = delay {
                std::thread::sleep(delay);
            }
            Ok(())
        }
    }

    /// Serves `responses` (status line, JSON body) to successive connections
    /// on a background thread; returns the base URL and the captured
    /// `METHOD /path` lines.
    pub(crate) fn serve(
        responses: Vec<(&'static str, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        serve_with(move |_| responses)
    }

    /// [`serve`] with responses that may embed the server's own base URL.
    pub(crate) fn serve_with(
        responses: impl FnOnce(&str) -> Vec<(&'static str, String)>,
    ) -> (String, Arc<Mutex<Vec<String>>>) {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let responses = responses(&base);
        let seen = Arc::new(Mutex::new(Vec::new()));
        let captured = seen.clone();
        std::thread::spawn(move || {
            let mut responses = responses.into_iter();
            loop {
                let Ok((mut socket, _)) = listener.accept() else {
                    return;
                };
                let mut request = Vec::new();
                let mut chunk = [0_u8; 4096];
                loop {
                    let Ok(read) = socket.read(&mut chunk) else {
                        return;
                    };
                    if read == 0 {
                        break;
                    }
                    request.extend_from_slice(&chunk[..read]);
                    let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") else {
                        continue;
                    };
                    let headers = String::from_utf8_lossy(&request[..end]).to_ascii_lowercase();
                    let length = headers
                        .lines()
                        .find_map(|line| line.strip_prefix("content-length:"))
                        .and_then(|value| value.trim().parse::<usize>().ok())
                        .unwrap_or(0);
                    if request.len() >= end + 4 + length {
                        break;
                    }
                }
                let text = String::from_utf8_lossy(&request);
                let line = text.lines().next().unwrap_or_default().to_string();
                let mut parts = line.split(' ');
                let method = parts.next().unwrap_or_default().to_string();
                let path = parts.next().unwrap_or_default().to_string();
                // A connection closed before sending anything (a cancelled
                // background request) takes no scripted response.
                if method.is_empty() {
                    continue;
                }
                // The notification poll runs beside every signed-in test:
                // answer it with an empty inbox, outside the script.
                let (status, body) = if path.starts_with("/api/v1/messages") {
                    (
                        "200 OK",
                        r#"{"items":[],"has_more":false,"total":0,"count":0}"#.to_string(),
                    )
                } else if path.starts_with("/api/v1/devices") {
                    // Push registration runs beside every sign-in, outside
                    // the script too.
                    if method == "DELETE" {
                        ("204 No Content", String::new())
                    } else {
                        (
                            "200 OK",
                            r#"{"id":42,"platform":"desktop-macos","push_token":"ppd_test"}"#
                                .to_string(),
                        )
                    }
                } else {
                    let Some(next) = responses.next() else {
                        return;
                    };
                    captured.lock().unwrap().push(format!("{method} {path}"));
                    next
                };
                let response = format!(
                    "HTTP/1.1 {status}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                    body.len()
                );
                let _ = socket.write_all(response.as_bytes());
            }
        });
        (base, seen)
    }

    /// A `/auth/device/activate` or `/refresh/commit` body.
    pub(crate) fn token_set(refresh: &str) -> String {
        format!(
            r#"{{"access_token":"{}","refresh_token":"{refresh}","expires_in":900}}"#,
            jwt(r#"{"aud":["ppvpn"]}"#)
        )
    }

    /// `header.payload.sig` with the given JSON claims.
    pub(crate) fn jwt(claims: &str) -> String {
        use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
        format!("e30.{}.sig", URL_SAFE_NO_PAD.encode(claims))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_device_name_says_which_os() {
        assert_eq!(device_name("macos"), "PPVPN Desktop · macOS");
        assert_eq!(device_name("windows"), "PPVPN Desktop · Windows");
        assert_eq!(device_name("linux"), "PPVPN Desktop · Linux");
        assert_eq!(device_name("bsd"), "PPVPN Desktop");
    }

    #[test]
    fn store_failures_keep_the_locked_flag() {
        assert!(
            store_failure(PlatformError::Locked {
                message: "x".into()
            })
            .locked
        );
        assert!(
            !store_failure(PlatformError::Failed {
                message: "x".into()
            })
            .locked
        );
    }

    #[tokio::test]
    async fn terminal_restore_maps_to_an_invalid_session() {
        let (base, _) = test_support::serve(vec![(
            "401 Unauthorized",
            r#"{"code":"AUTH_DEVICE_CREDENTIAL_INVALID"}"#.into(),
        )]);
        let platform =
            test_support::MemoryPlatform::with_blob(r#"{"version":1,"pending_activation":"rt_x"}"#);
        let auth = client_auth(Arc::new(crate::api::client_api(&base)), platform.clone());
        let error = auth.restore().await.unwrap_err();
        assert!(matches!(
            error.into_client_error(),
            ClientError::Failed {
                code: ErrorCode::AuthSessionInvalid,
                ..
            }
        ));
        assert!(platform.stored().is_none());
    }
}
