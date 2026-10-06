//! Command implementations: the account, the daemon's lifecycle and the
//! local ones. Commands that read from or change the running instance are
//! in [`crate::queries`].

use clap::CommandFactory;
use serde_json::{json, Value};

use crate::account;
use crate::buildinfo::{self, BuildConfig, Profile};
use crate::cli::{Cli, Command, IngressCommand, ProxyCommand};
use crate::client::{self, Client};
use crate::control::Call;
use crate::daemon::Daemon;
use crate::env::{Env, Os};
use crate::error::{CliError, Exit, Result};
use crate::output::Printer;
use crate::paths::Paths;
use crate::queries;
use crate::settings::{RoutingMode, Settings};
use crate::Hooks;

fn build_config(env: &Env, hooks: &Hooks) -> Result<BuildConfig> {
    match &hooks.build {
        Some(config) => Ok(config.clone()),
        None => buildinfo::resolve(env),
    }
}

fn auth(config: &BuildConfig, env: &Env, hooks: &Hooks) -> ppvpn_account::auth::Auth {
    account::auth(config, env, store(hooks))
}

fn store(hooks: &Hooks) -> std::sync::Arc<dyn ppvpn_account::auth::CredentialStore> {
    hooks
        .store
        .clone()
        .unwrap_or_else(crate::keystore::platform_store)
}

/// The saved credential, or why the secret store cannot be read.
fn load_saved(store: &dyn ppvpn_account::auth::CredentialStore) -> Result<Option<Vec<u8>>> {
    store.load().map_err(|failure| {
        let code = if failure.locked {
            "CREDENTIAL_STORE_LOCKED"
        } else {
            "CREDENTIAL_STORE_UNAVAILABLE"
        };
        CliError::environment(code, failure.message)
    })
}

pub fn run(cli: &Cli, env: &Env, hooks: &Hooks, out: &mut Printer) -> Result<()> {
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
        Command::Mode { mode } => mode_command(mode.as_deref(), env, hooks, out),
        Command::Doctor => doctor(env, hooks, out),
        Command::Login { no_browser } => login(env, hooks, out, *no_browser),
        Command::Account => account_command(env, hooks, out),
        Command::Logout => logout(env, hooks, out),
        Command::Start { foreground } => start(env, hooks, out, *foreground, false),
        Command::Restart { foreground } => start(env, hooks, out, *foreground, true),
        Command::Stop => stop(env, out),
        Command::Status => status(env, out),
        Command::Daemon => daemon(env),
        Command::Nodes => queries::nodes(env, out),
        Command::Use { node_id } => queries::use_node(env, out, node_id),
        Command::Probe {
            node_id, target, ..
        } => {
            let (probe, timeout, concurrency) = cli.command.probe_settings()?;
            queries::probe(
                env,
                out,
                probe,
                node_id.as_deref(),
                target,
                timeout,
                concurrency,
            )
        }
        Command::Traffic => queries::traffic(env, out),
        Command::Connections => queries::connections(env, out),
        Command::Proxy { command: None } => queries::proxy(env, out),
        Command::Proxy {
            command: Some(ProxyCommand::Credential { node_id }),
        } => queries::proxy_credential(env, out, node_id.as_deref()),
        Command::Ingress {
            command:
                Some(IngressCommand::Pin {
                    node_id,
                    endpoint_key,
                }),
            ..
        } => queries::pin(env, out, node_id, Some(endpoint_key)),
        Command::Ingress {
            command: Some(IngressCommand::Auto { node_id }),
            ..
        } => queries::pin(env, out, node_id, None),
        Command::Ingress {
            node_id,
            command: None,
        } => queries::ingress(env, out, node_id.as_deref()),
    }
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

fn mode_command(mode: Option<&str>, env: &Env, hooks: &Hooks, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let settings = Settings::load(&paths.settings)?;
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
    // A running instance takes the mode first; when it refuses, the saved
    // mode stays what is live.
    let applied = match queries::set_routing_mode(&paths, mode) {
        // The profile the daemon holds is past its expiry: fetch a new one
        // and apply that with the new mode.
        Err(err) if err.code == "PROFILE_EXPIRED" => {
            let config = build_config(env, hooks)?;
            let mut wanted = settings;
            wanted.routing_mode = mode;
            runtime()?.block_on(async {
                let profile = load_profile(env, hooks, &config).await?;
                let request = apply_request(profile, &wanted, &config);
                apply_and_start(&Client::new(&paths)?, request, &paths, out).await
            })?;
            true
        }
        other => other?,
    };
    // Applying may have rewritten the settings (cleared pins, a reset selection).
    let mut settings = Settings::load(&paths.settings)?;
    settings.routing_mode = mode;
    settings.save(&paths.settings)?;
    out.success(
        &json!({"ok": true, "routing_mode": mode.as_str(), "applied": applied}),
        &format!("Routing mode: {}", mode.as_str()),
    )
    .map_err(io)
}

fn doctor(env: &Env, hooks: &Hooks, out: &mut Printer) -> Result<()> {
    let config = build_config(env, hooks)?;
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

pub(crate) fn runtime() -> Result<tokio::runtime::Runtime> {
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .map_err(|e| CliError::new(Exit::Other, "RUNTIME_FAILED", e.to_string()))
}

pub(crate) fn output_error(err: std::io::Error) -> CliError {
    CliError::new(Exit::Other, "OUTPUT_FAILED", err.to_string())
}

/// The profile to apply: downloaded with the saved login, or, in a dev
/// build only, read from the absolute path in `PPVPN_PROFILE_FILE`.
async fn load_profile(env: &Env, hooks: &Hooks, config: &BuildConfig) -> Result<Vec<u8>> {
    if config.profile == Profile::Dev {
        if let Some(path) = env.var("PPVPN_PROFILE_FILE") {
            if !path.starts_with('/') {
                return Err(CliError::argument(
                    "PPVPN_PROFILE_FILE must be an absolute path",
                ));
            }
            return std::fs::read(path).map_err(|e| {
                CliError::environment(
                    "PROFILE_FILE_UNREADABLE",
                    format!("cannot read {path}: {e}"),
                )
            });
        }
    }
    account::download_profile(&auth(config, env, hooks)).await
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
    let proxy = &status["local_proxy"];
    if let (Some(listen), Some(port)) = (proxy["listen"].as_str(), proxy["port"].as_u64()) {
        let listening = match proxy["listening"].as_bool() {
            Some(false) => " (not listening)",
            _ => "",
        };
        human.push_str(&format!("\nLocal proxy: {listen}:{port}{listening}"));
    }
    // Set for the instance's lifetime: core does not track who has seen it.
    if let Some(reason) = proxy["credentials_reset"].as_str() {
        let why = match reason {
            "corrupt" => "its state file was damaged",
            "insecure_permissions" => "its state file was readable by other users",
            other => other,
        };
        human.push_str(&format!(
            "\nNote: the local proxy credential was replaced when this instance started ({why}). Applications using the old one need the new one: ppvpn proxy credential"
        ));
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

fn start(
    env: &Env,
    hooks: &Hooks,
    out: &mut Printer,
    foreground: bool,
    restart: bool,
) -> Result<()> {
    let config = build_config(env, hooks)?;
    let paths = Paths::resolve(env)?;
    runtime()?.block_on(async {
        // Fetch the profile before touching the daemon, so a missing login
        // or an unreachable backend never stops a running instance.
        let profile = load_profile(env, hooks, &config).await?;
        let request = apply_request(profile, &Settings::load(&paths.settings)?, &config);
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

fn hostname() -> String {
    let mut buf = [0u8; 256];
    // SAFETY: the buffer is valid for its length; gethostname writes at most that.
    let ok = unsafe { libc::gethostname(buf.as_mut_ptr().cast(), buf.len()) } == 0;
    let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
    let name = String::from_utf8_lossy(&buf[..len]).trim().to_string();
    if ok && !name.is_empty() {
        name
    } else {
        "terminal".to_string()
    }
}

/// Opens the authorization page; failing to is fine, the URL is printed.
fn open_browser(os: Os, url: &str) {
    let program = match os {
        Os::Macos => "open",
        Os::Linux => "xdg-open",
    };
    let _ = std::process::Command::new(program)
        .arg(url)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
}

fn login(env: &Env, hooks: &Hooks, out: &mut Printer, no_browser: bool) -> Result<()> {
    let config = build_config(env, hooks)?;
    // A login is only worth starting when it can be saved: without a
    // usable secret store, say so now rather than after the user has
    // confirmed the code in the browser.
    let store = store(hooks);
    load_saved(store.as_ref())?;
    let auth = account::auth(&config, env, store);
    let platform = match env.os {
        Os::Macos => "macos",
        Os::Linux => "linux",
    };
    runtime()?.block_on(async {
        let started = auth
            .start(&hostname(), platform, buildinfo::version())
            .await
            .map_err(|e| account::auth_error(&e))?;
        // The account password is only ever typed into the browser page.
        let _ = out.progress(&format!(
            "Authorize this device at:\n{}\n\nConfirmation code: {}",
            started.verification_url, started.user_code
        ));
        if !no_browser {
            open_browser(env.os, &started.verification_url);
        }
        let _ = out.progress("Waiting for authorization...");
        let pending = auth
            .poll_until_done(started.generation)
            .await
            .map_err(|e| account::auth_error(&e))?;
        let _guard = auth.lock().await;
        auth.activate(pending, Some(started.generation))
            .await
            .map_err(|e| account::auth_error(&e))?;
        out.success(
            &json!({"ok": true, "status": "authorized", "user_code": started.user_code}),
            "Authorized this device.",
        )
        .map_err(output_error)
    })
}

fn account_command(env: &Env, hooks: &Hooks, out: &mut Printer) -> Result<()> {
    let config = build_config(env, hooks)?;
    let auth = auth(&config, env, hooks);
    runtime()?.block_on(async {
        let token = account::access_token(&auth).await?;
        let user = auth
            .api()
            .account(&token)
            .await
            .map_err(|e| account::api_error(&e))?;
        let mut value = json!({"ok": true, "account": {"id": user.id, "name": user.name}});
        if let Some(email) = &user.email {
            value["account"]["email"] = json!(email);
        }
        out.success(&value, &format!("{} ({})", user.name, user.id))
            .map_err(output_error)
    })
}

fn logout(env: &Env, hooks: &Hooks, out: &mut Printer) -> Result<()> {
    let config = build_config(env, hooks)?;
    let store = store(hooks);
    if load_saved(store.as_ref())?.is_none() {
        return Err(account::not_logged_in());
    }
    let auth = account::auth(&config, env, store);
    runtime()?.block_on(async {
        // Revoking on the backend is best effort; removing the local
        // credential is what logs this device out.
        auth.logout().await.map_err(|e| account::auth_error(&e))?;
        out.success(&json!({"ok": true, "local_removed": true}), "Logged out.")
            .map_err(output_error)
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn status_text_shows_the_local_proxy_and_a_credential_reset() {
        let (value, human) = status_output(
            json!({
                "state": "running",
                "routing_mode": "rules",
                "local_proxy": {"listen": "127.0.0.1", "port": 7890, "listening": true},
            }),
            Some(7),
        );
        assert_eq!(
            human,
            "Core: running\nRouting mode: rules\nLocal proxy: 127.0.0.1:7890"
        );
        assert_eq!(value["pid"], 7);

        let (value, human) = status_output(
            json!({
                "state": "degraded",
                "local_proxy": {
                    "listen": "127.0.0.1", "port": 7890, "listening": false,
                    "credentials_reset": "corrupt",
                },
            }),
            Some(7),
        );
        // --json passes core's field through as it is.
        assert_eq!(value["local_proxy"]["credentials_reset"], "corrupt");
        let lines: Vec<&str> = human.lines().collect();
        assert_eq!(lines[1], "Local proxy: 127.0.0.1:7890 (not listening)");
        assert!(lines[2].starts_with("Note: the local proxy credential was replaced"));
        assert!(lines[2].contains("its state file was damaged"));
        assert!(lines[2].ends_with("ppvpn proxy credential"));

        let (_, human) = status_output(json!({"state": "stopped"}), None);
        assert_eq!(human, "Core: stopped");
    }
}
