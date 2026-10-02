//! CLI errors and their exit codes.
//!
//! The exit code says which part failed, so scripts can tell a bad argument
//! from an expired login or an unavailable backend. Codes and their meaning
//! are a contract (see `docs/cli.md`):
//!
//! | exit | category |
//! | --- | --- |
//! | 1 | other or internal error |
//! | 2 | invalid argument or build configuration |
//! | 3 | login missing, expired or not permitted |
//! | 4 | backend unavailable |
//! | 5 | core not running or core operation failed |
//! | 6 | incompatible core or feature unavailable |
//! | 7 | the backend's profile could not be applied |
//! | 8 | local environment (files, directories, keychain) |

use std::fmt;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Exit {
    Other = 1,
    Argument = 2,
    Auth = 3,
    Backend = 4,
    Core = 5,
    Incompatible = 6,
    Profile = 7,
    Environment = 8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CliError {
    pub exit: Exit,
    /// Stable, upper-case code for `--json` output.
    pub code: String,
    /// One line for people; never contains credentials.
    pub message: String,
    pub retryable: bool,
    /// The input field at fault, when core names one (`--json` only).
    pub field: Option<String>,
}

impl CliError {
    pub fn new(exit: Exit, code: &str, message: impl Into<String>) -> Self {
        CliError {
            exit,
            code: code.to_string(),
            message: message.into(),
            retryable: false,
            field: None,
        }
    }

    pub fn retryable(mut self) -> Self {
        self.retryable = true;
        self
    }

    pub fn argument(message: impl Into<String>) -> Self {
        CliError::new(Exit::Argument, "INVALID_ARGUMENT", message)
    }

    pub fn environment(code: &str, message: impl Into<String>) -> Self {
        CliError::new(Exit::Environment, code, message)
    }

    pub fn exit_code(&self) -> i32 {
        self.exit as i32
    }
}

impl fmt::Display for CliError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for CliError {}

pub type Result<T> = std::result::Result<T, CliError>;

/// Maps an error from ppvpn-core to the CLI's exit categories. Core's
/// runtime codes are listed; every other code is a profile or request
/// validation failure (exit 7).
pub fn from_core(code: &str, field: Option<String>, retryable: bool, message: &str) -> CliError {
    let exit = match code {
        "NODE_NOT_FOUND" | "INGRESS_NOT_FOUND" | "PINS_INVALID" | "ROUTING_MODE_INVALID" => {
            Exit::Argument
        }
        "PROFILE_NOT_APPLIED"
        | "CORE_NOT_RUNNING"
        | "CORE_OPERATION_FAILED"
        | "CORE_PANICKED"
        | "ENGINE_FATAL"
        | "ENGINE_SHUT_DOWN"
        | "NO_DEFAULT_INTERFACE" => Exit::Core,
        "LOCAL_PROXY_DISABLED"
        | "SYSTEM_PROXY_UNAVAILABLE"
        | "SYSTEM_PROXY_START_FAILED"
        | "TUN_INSTANCE_EXISTS"
        | "WINTUN_UNAVAILABLE" => Exit::Incompatible,
        "STATE_DIR_IN_USE" | "PERMISSION_DENIED" => Exit::Environment,
        _ => Exit::Profile,
    };
    CliError {
        exit,
        code: code.to_string(),
        message: message.to_string(),
        retryable,
        field,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn core_codes_map_to_exit_categories() {
        let exit = |code: &str| from_core(code, None, false, "m").exit_code();
        assert_eq!(exit("NODE_NOT_FOUND"), 2);
        assert_eq!(exit("PROFILE_NOT_APPLIED"), 5);
        assert_eq!(exit("ENGINE_SHUT_DOWN"), 5);
        assert_eq!(exit("LOCAL_PROXY_DISABLED"), 6);
        assert_eq!(exit("STATE_DIR_IN_USE"), 8);
        assert_eq!(exit("PROFILE_EXPIRED"), 7);
        assert_eq!(exit("RULE_SET_HOST_NOT_ALLOWED"), 7);
        let err = from_core(
            "DEFAULT_NODE_NOT_FOUND",
            Some("selection.default_node_id".into()),
            false,
            "m",
        );
        assert_eq!(err.field.as_deref(), Some("selection.default_node_id"));
    }
}
