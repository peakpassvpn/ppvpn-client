//! Text and `--json` output.
//!
//! With `--json`, stdout carries exactly one JSON value per invocation, for
//! errors too; progress and warnings go to stderr. Without it, results go to
//! stdout and errors to stderr as `Error: <message>`.

use std::io::Write;

use serde_json::{json, Value};

use crate::error::CliError;

pub struct Printer<'a> {
    pub json: bool,
    pub stdout: &'a mut dyn Write,
    pub stderr: &'a mut dyn Write,
}

impl Printer<'_> {
    /// Prints a successful result: `value` (which must have `"ok": true`)
    /// with `--json`, `human` otherwise.
    pub fn success(&mut self, value: &Value, human: &str) -> std::io::Result<()> {
        if self.json {
            writeln!(self.stdout, "{}", serde_json::to_string(value)?)
        } else {
            writeln!(self.stdout, "{human}")
        }
    }

    pub fn failure(&mut self, error: &CliError) -> std::io::Result<()> {
        if self.json {
            let value = json!({
                "ok": false,
                "code": error.code,
                "message": error.message,
                "retryable": error.retryable,
            });
            writeln!(self.stdout, "{}", serde_json::to_string(&value)?)
        } else {
            writeln!(self.stderr, "Error: {}", error.message)
        }
    }

    /// Progress and warnings: always stderr, so `--json` stdout stays clean.
    pub fn progress(&mut self, line: &str) -> std::io::Result<()> {
        writeln!(self.stderr, "{line}")
    }
}
