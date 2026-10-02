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
}

impl CliError {
    pub fn new(exit: Exit, code: &str, message: impl Into<String>) -> Self {
        CliError {
            exit,
            code: code.to_string(),
            message: message.into(),
            retryable: false,
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
