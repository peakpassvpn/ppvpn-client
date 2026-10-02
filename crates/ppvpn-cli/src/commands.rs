//! Command implementations.
//!
//! Commands that need the account crate (`ppvpn-account`) or a daemon call
//! that is not wired yet report `NOT_IMPLEMENTED` (see `docs/cli.md`).
//! Their arguments are still parsed and checked, so the syntax and exit
//! codes are fixed now.

use clap::CommandFactory;
use serde_json::{json, Value};

use crate::buildinfo::{self, BuildConfig, Profile};
use crate::cli::{Cli, Command};
use crate::client::{self, Client};
use crate::control::Call;
use crate::daemon::Daemon;
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
        Command::Start { foreground } => start(env, out, *foreground, false),
        Command::Restart { foreground } => start(env, out, *foreground, true),
        Command::Stop => stop(env, out),
        Command::Status => status(env, out),
        Command::Daemon => daemon(env),
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

fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::new(Exit::Other, "RUNTIME_FAILED", e.to_string()))
}

fn output_error(err: std::io::Error) -> CliError {
    CliError::new(Exit::Other, "OUTPUT_FAILED", err.to_string())
}

/// The profile to apply. Downloading it needs `ppvpn-account`; until that is
/// wired in, only dev builds can supply one from `PPVPN_PROFILE_FILE`.
fn load_profile(env: &Env, config: &BuildConfig) -> Result<Vec<u8>> {
    let path = match (config.profile, env.var("PPVPN_PROFILE_FILE")) {
        (Profile::Dev, Some(path)) if path.starts_with('/') => path,
        (Profile::Dev, Some(_)) => {
            return Err(CliError::argument(
                "PPVPN_PROFILE_FILE must be an absolute path",
            ))
        }
        _ => {
            return Err(CliError::new(
                Exit::Other,
                "NOT_IMPLEMENTED",
                "downloading the profile is not implemented yet (needs ppvpn-account)",
            ))
        }
    };
    std::fs::read(path).map_err(|e| {
        CliError::environment(
            "PROFILE_FILE_UNREADABLE",
            format!("cannot read {path}: {e}"),
        )
    })
}

fn apply_request(
    profile: Vec<u8>,
    settings: &Settings,
    config: &BuildConfig,
) -> ppvpn_core::ApplyRequest {
    let mode = match settings.routing_mode {
        RoutingMode::Rules => ppvpn_core::RoutingMode::Rules,
        RoutingMode::Global => ppvpn_core::RoutingMode::Global,
    };
    let pins = settings
        .ingress_pins
        .iter()
        .map(|(node, key)| ppvpn_core::Pin::new(node.clone(), key.clone()))
        .collect();
    let mut request = ppvpn_core::ApplyRequest::new(profile)
        .with_routing_mode(mode)
        .with_pins(pins)
        .with_allowed_rule_set_hosts(buildinfo::rule_set_hosts(&config.api_base));
    if let Some(node) = &settings.selected_node_id {
        request = request.with_selected_node_id(node.clone());
    }
    request
}

/// Applies and starts, then records what core cleared or reset.
async fn apply_and_start(
    client: &Client,
    request: ppvpn_core::ApplyRequest,
    paths: &Paths,
    out: &mut Printer<'_>,
) -> Result<Value> {
    let result = client.call(Call::Apply(request)).await?;
    let mut settings = Settings::load(&paths.settings)?;
    let mut changed = false;
    if let Some(cleared) = result["cleared_pins"].as_array() {
        for pin in cleared {
            if let (Some(node), Some(key)) = (pin["node_id"].as_str(), pin["endpoint_key"].as_str())
            {
                if settings.ingress_pins.remove(node).is_some() {
                    changed = true;
                    let _ = out.progress(&format!(
                        "Warning: ingress {key} of node {node} is no longer in the profile; the node is back to automatic failover."
                    ));
                }
            }
        }
    }
    if result["selection_reset"].as_bool() == Some(true) {
        settings.selected_node_id = result["selected_node_id"].as_str().map(str::to_string);
        changed = true;
        let _ = out.progress("Warning: the selected node is no longer in the profile; using the profile's default node.");
    }
    if changed {
        settings.save(&paths.settings)?;
    }
    client.call(Call::Start).await?;
    client.call(Call::Status).await
}

fn status_output(status: Value, pid: Option<u32>) -> (Value, String) {
    let state = status["state"].as_str().unwrap_or("stopped").to_string();
    let mut human = format!("Core: {state}");
    if let Some(mode) = status["routing_mode"].as_str() {
        human.push_str(&format!("\nRouting mode: {mode}"));
    }
    let mut value = json!({"ok": true, "daemon_running": pid.is_some()});
    if let Some(pid) = pid {
        value["pid"] = json!(pid);
    }
    if let (Value::Object(target), Value::Object(fields)) = (&mut value, status) {
        target.extend(fields);
    }
    if value.get("state").is_none() {
        value["state"] = json!(state);
    }
    (value, human)
}

fn start(env: &Env, out: &mut Printer, foreground: bool, restart: bool) -> Result<()> {
    let config = buildinfo::resolve(env)?;
    let paths = Paths::resolve(env)?;
    let profile = load_profile(env, &config)?;
    let request = apply_request(profile, &Settings::load(&paths.settings)?, &config);
    runtime()?.block_on(async {
        if restart {
            stop_daemon(&paths).await?;
        }
        if foreground {
            let (daemon, listener) = Daemon::bind(&paths, env.os).await?;
            let pid = std::process::id();
            let serve = tokio::spawn(daemon.serve(listener));
            let client = Client::new(&paths)?;
            match apply_and_start(&client, request, &paths, out).await {
                Ok(status) => {
                    let (value, human) = status_output(status, Some(pid));
                    out.success(&value, &human).map_err(output_error)?;
                }
                Err(err) => {
                    let _ = client.call(Call::Shutdown).await;
                    let _ = serve.await;
                    return Err(err);
                }
            }
            return serve
                .await
                .map_err(|e| CliError::new(Exit::Other, "INTERNAL_ERROR", e.to_string()))?;
        }
        let (client, launched) = client::connect_or_launch(&paths).await?;
        match apply_and_start(&client, request, &paths, out).await {
            Ok(status) => {
                let (value, human) =
                    status_output(status, client::running_daemon(&paths).map(|r| r.pid));
                out.success(&value, &human).map_err(output_error)
            }
            Err(err) => {
                // Do not leave a daemon behind that this call started.
                if launched {
                    let _ = stop_daemon(&paths).await;
                }
                Err(err)
            }
        }
    })
}

/// Stops the recorded daemon; returns whether one was running.
async fn stop_daemon(paths: &Paths) -> Result<bool> {
    let Some(record) = client::running_daemon(paths) else {
        return Ok(false);
    };
    let asked = match Client::new(paths) {
        Ok(client) => client.call(Call::Shutdown).await.is_ok(),
        Err(_) => false,
    };
    if !asked {
        // The record matched PID, executable and start time, so this is our
        // daemon even though its control channel is gone.
        unsafe { libc::kill(record.pid as i32, libc::SIGTERM) };
    }
    if client::wait_for_exit(&record, client::EXIT_TIMEOUT).await {
        Ok(true)
    } else {
        Err(CliError::new(
            Exit::Core,
            "DAEMON_STOP_TIMEOUT",
            "the daemon did not exit in time",
        )
        .retryable())
    }
}

fn stop(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let stopped = runtime()?.block_on(stop_daemon(&paths))?;
    out.success(&json!({"ok": true, "stopped": stopped}), "Core: stopped")
        .map_err(output_error)
}

fn status(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let Some(record) = client::running_daemon(&paths) else {
        let (value, human) = status_output(json!({"state": "stopped"}), None);
        return out.success(&value, &human).map_err(output_error);
    };
    let status = runtime()?.block_on(async { Client::new(&paths)?.call(Call::Status).await })?;
    let (value, human) = status_output(status, Some(record.pid));
    out.success(&value, &human).map_err(output_error)
}

fn daemon(env: &Env) -> Result<()> {
    let paths = Paths::resolve(env)?;
    runtime()?.block_on(async {
        let (daemon, listener) = Daemon::bind(&paths, env.os).await?;
        daemon.serve(listener).await
    })
}
