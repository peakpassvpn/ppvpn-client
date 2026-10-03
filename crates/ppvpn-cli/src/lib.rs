//! The `ppvpn` command-line client as a library: the binary in `main.rs` is
//! a thin wrapper, and tests drive [`run`] directly.

pub mod account;
pub mod buildinfo;
pub mod cli;
pub mod client;
pub mod commands;
pub mod control;
pub mod daemon;
pub mod env;
pub mod error;
pub mod identity;
pub mod keystore;
pub mod output;
pub mod paths;
pub mod queries;
pub mod settings;

use std::io::Write;

use clap::Parser;

use crate::cli::Cli;
use crate::env::Env;
use crate::error::CliError;
use crate::output::Printer;

/// What an embedder or a test may replace: the compiled-in build
/// configuration and the platform secret store.
#[derive(Clone, Default)]
pub struct Hooks {
    pub build: Option<buildinfo::BuildConfig>,
    pub store: Option<std::sync::Arc<dyn ppvpn_account::auth::CredentialStore>>,
}

/// Runs one invocation and returns its exit code. `args` includes the
/// program name.
pub fn run(
    args: &[std::ffi::OsString],
    env: &Env,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    run_with(args, env, &Hooks::default(), stdout, stderr)
}

/// [`run`] with replaceable build configuration and secret store.
pub fn run_with(
    args: &[std::ffi::OsString],
    env: &Env,
    hooks: &Hooks,
    stdout: &mut dyn Write,
    stderr: &mut dyn Write,
) -> i32 {
    let json = args.iter().skip(1).any(|a| a == "--json");
    let cli = match Cli::try_parse_from(args) {
        Ok(cli) => cli,
        Err(err) => {
            use clap::error::ErrorKind;
            if matches!(
                err.kind(),
                ErrorKind::DisplayHelp | ErrorKind::DisplayVersion
            ) {
                let _ = write!(stdout, "{}", err.render());
                return 0;
            }
            // clap renders "error: <message>" plus usage; keep the first line.
            let rendered = err.render().to_string();
            let message = rendered
                .lines()
                .next()
                .unwrap_or("invalid arguments")
                .trim_start_matches("error: ")
                .to_string();
            let error = CliError::argument(message);
            let _ = Printer {
                json,
                stdout,
                stderr,
            }
            .failure(&error);
            return error.exit_code();
        }
    };
    let mut printer = Printer {
        json: cli.json,
        stdout,
        stderr,
    };
    match commands::run(&cli, env, hooks, &mut printer) {
        Ok(()) => 0,
        Err(error) => {
            let _ = printer.failure(&error);
            error.exit_code()
        }
    }
}
