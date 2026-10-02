//! The daemon's private control channel.
//!
//! A Unix socket (mode 0600, in a 0700 directory) carries one JSON request
//! per line and gets one JSON response per line. Every request carries the
//! session secret from `session.secret` (0600), so a socket another process
//! left behind, or a path swapped underneath, cannot be driven without it.
//! The protocol is private to one CLI version: the client and the daemon
//! are the same binary.

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::error::{from_core, CliError, Exit};

/// Profiles are a few hundred KiB at most; leave room.
pub const MAX_MESSAGE: usize = 16 << 20;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub secret: String,
    #[serde(flatten)]
    pub call: Call,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "method", content = "params", rename_all = "snake_case")]
pub enum Call {
    /// Liveness and identity: the daemon's PID and core version.
    Ping,
    Status,
    Apply(ppvpn_core::ApplyRequest),
    Start,
    /// Shuts the engine down and ends the daemon.
    Shutdown,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct WireError {
    pub code: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub retryable: bool,
    pub message: String,
}

impl From<&ppvpn_core::Error> for WireError {
    fn from(err: &ppvpn_core::Error) -> Self {
        WireError {
            code: err.code.to_string(),
            field: err.field.clone(),
            retryable: err.retryable,
            message: err.message.clone(),
        }
    }
}

impl From<&CliError> for WireError {
    fn from(err: &CliError) -> Self {
        WireError {
            code: err.code.clone(),
            field: err.field.clone(),
            retryable: err.retryable,
            message: err.message.clone(),
        }
    }
}

impl WireError {
    pub fn into_cli(self) -> CliError {
        if self.code == UNAUTHENTICATED {
            return CliError::new(Exit::Core, UNAUTHENTICATED, self.message);
        }
        from_core(&self.code, self.field, self.retryable, &self.message)
    }
}

pub const UNAUTHENTICATED: &str = "CONTROL_UNAUTHENTICATED";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum Response {
    Ok { ok: bool, data: Value },
    Err { ok: bool, error: WireError },
}

impl Response {
    pub fn ok(data: Value) -> Response {
        Response::Ok { ok: true, data }
    }

    pub fn err(error: WireError) -> Response {
        Response::Err { ok: false, error }
    }
}

/// Compares secrets in time independent of where they differ.
pub fn secrets_match(a: &str, b: &str) -> bool {
    a.len() == b.len()
        && a.bytes()
            .zip(b.bytes())
            .fold(0u8, |acc, (x, y)| acc | (x ^ y))
            == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn requests_round_trip_with_the_secret() {
        let request = Request {
            secret: "s".repeat(64),
            call: Call::Apply(ppvpn_core::ApplyRequest::new(b"{}".to_vec())),
        };
        let text = serde_json::to_string(&request).unwrap();
        assert!(text.contains("\"method\":\"apply\""), "{text}");
        let back: Request = serde_json::from_str(&text).unwrap();
        assert!(matches!(back.call, Call::Apply(_)));
        let ping: Request = serde_json::from_str(r#"{"secret":"x","method":"ping"}"#).unwrap();
        assert!(matches!(ping.call, Call::Ping));
    }

    #[test]
    fn responses_distinguish_success_and_error() {
        let ok: Response = serde_json::from_str(r#"{"ok":true,"data":{"pid":1}}"#).unwrap();
        assert!(matches!(ok, Response::Ok { ok: true, .. }));
        let err: Response = serde_json::from_str(r#"{"ok":false,"error":{"code":"PROFILE_NOT_APPLIED","retryable":false,"message":"m"}}"#).unwrap();
        let Response::Err { error, .. } = err else {
            panic!("expected an error")
        };
        assert_eq!(error.into_cli().exit_code(), 5);
    }

    #[test]
    fn secret_comparison() {
        assert!(secrets_match("abc", "abc"));
        assert!(!secrets_match("abc", "abd"));
        assert!(!secrets_match("abc", "abcd"));
    }
}
