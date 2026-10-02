//! Command implementations.
//!
//! Commands that need the core library or the account crate report
//! `NOT_IMPLEMENTED` until those land: the core's public API skeleton and
//! `ppvpn-account` (see `docs/cli.md`). Their arguments are still parsed and
//! checked, so the syntax and exit codes are fixed now.

use clap::CommandFactory;
use serde_json::{json, Value};

use crate::buildinfo;
use crate::cli::{Cli, Command};
use crate::env::{Env, Os};
use crate::error::{CliError, Exit, Result};
use crate::output::Printer;
use crate::paths::Paths;
use crate::settings::{RoutingMode, Settings};

pub fn run(cli: &Cli, env: &Env, out: &mut Printer) -> Result<()> {
    cli.command.validate(cli.json)?;
    let io = |err: std::io::Error| CliError::new(Exit::Other, "OUTPUT_FAILED", err.to_string());
    match &cli.command {
        Command::Version => {
            let version = buildinfo::version();
            out.success(
                &json!({"ok": true, "cli_version": version}),
                &format!("ppvpn {version}"),
            )
            .map_err(io)
        }
        Command::Completion { shell } => completion(shell, out).map_err(io),
        Command::Mode { mode } => mode_command(mode.as_deref(), env, out),
        Command::Doctor => doctor(env, out),
        other => Err(not_implemented(other)),
    }
}

fn not_implemented(command: &Command) -> CliError {
    let name = format!("{command:?}");
    let name = name
        .split([' ', '{', '('])
        .next()
        .unwrap_or("command")
        .to_ascii_lowercase();
    CliError::new(
        Exit::Other,
        "NOT_IMPLEMENTED",
        format!("ppvpn {name} is not implemented yet"),
    )
}

fn completion(shell: &str, out: &mut Printer) -> std::io::Result<()> {
    let shell = match shell {
        "bash" => clap_complete::Shell::Bash,
        "zsh" => clap_complete::Shell::Zsh,
        _ => clap_complete::Shell::Fish,
    };
    clap_complete::generate(shell, &mut Cli::command(), "ppvpn", out.stdout);
    Ok(())
}

fn mode_command(mode: Option<&str>, env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let mut settings = Settings::load(&paths.settings)?;
    let io = |err: std::io::Error| CliError::new(Exit::Other, "OUTPUT_FAILED", err.to_string());
    let Some(mode) = mode.and_then(RoutingMode::parse) else {
        let mode = settings.routing_mode.as_str();
        return out
            .success(
                &json!({"ok": true, "routing_mode": mode}),
                &format!("Routing mode: {mode}"),
            )
            .map_err(io);
    };
    settings.routing_mode = mode;
    settings.save(&paths.settings)?;
    // Applying the new mode to a running instance comes with the daemon.
    out.success(
        &json!({"ok": true, "routing_mode": mode.as_str(), "applied": false}),
        &format!("Routing mode: {}", mode.as_str()),
    )
    .map_err(io)
}

fn doctor(env: &Env, out: &mut Printer) -> Result<()> {
    let config = buildinfo::resolve(env)?;
    let paths = Paths::resolve(env)?;
    let platform = platform(env.os);
    let value: Value = json!({
        "ok": true,
        "platform": platform,
        "cli_version": buildinfo::version(),
        "build_profile": config.profile,
        "api_origin": config.api_base,
        "runtime_directory": paths.runtime_dir,
        "state_directory": paths.state_dir,
        "settings_file": paths.settings,
    });
    let profile = serde_json::to_value(config.profile)
        .ok()
        .and_then(|v| v.as_str().map(str::to_string))
        .unwrap_or_default();
    let human = format!(
        "Platform: {platform}\nCLI: {}\nBuild profile: {profile}\nAPI origin: {}\nRuntime: {}\nState: {}\nSettings: {}",
        buildinfo::version(),
        config.api_base,
        paths.runtime_dir.display(),
        paths.state_dir.display(),
        paths.settings.display(),
    );
    out.success(&value, &human)
        .map_err(|err| CliError::new(Exit::Other, "OUTPUT_FAILED", err.to_string()))
}

/// `darwin/arm64` style, as the Go CLI and release archive names use.
fn platform(os: Os) -> String {
    let os = match os {
        Os::Macos => "darwin",
        Os::Linux => "linux",
    };
    let arch = match std::env::consts::ARCH {
        "x86_64" => "amd64",
        "aarch64" => "arm64",
        other => other,
    };
    format!("{os}/{arch}")
}
