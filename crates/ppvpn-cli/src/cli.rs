//! Command-line syntax. Value checks that clap cannot express live in
//! [`Command::validate`], so they run before any file, keychain, backend or
//! core access.

use std::time::Duration;

use clap::{Parser, Subcommand};

use crate::error::{CliError, Result};
use crate::settings::RoutingMode;

pub const DEFAULT_AVAILABILITY_TARGET: &str = "https://www.gstatic.com/generate_204";

#[derive(Debug, Parser)]
#[command(
    name = "ppvpn",
    about = "PeakPass VPN terminal client",
    disable_help_subcommand = true
)]
pub struct Cli {
    /// Output one JSON value on stdout.
    #[arg(long, global = true)]
    pub json: bool,
    /// Disable colored output.
    #[arg(long, global = true)]
    pub no_color: bool,
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Debug, Subcommand)]
pub enum Command {
    /// Show the CLI version.
    Version,
    /// Authorize this device in a browser.
    Login {
        /// Do not open a browser automatically.
        #[arg(long)]
        no_browser: bool,
    },
    /// Show the authorized account.
    Account,
    /// Revoke and remove this device credential.
    Logout,
    /// Fetch a profile and start the local proxy.
    Start {
        /// Stay attached to this terminal.
        #[arg(long)]
        foreground: bool,
    },
    /// Fetch a profile and restart the local proxy.
    Restart {
        /// Stay attached to this terminal.
        #[arg(long)]
        foreground: bool,
    },
    /// Stop the local proxy.
    Stop,
    /// Show status.
    Status,
    /// List nodes from the active profile.
    Nodes,
    /// Select the node used for new connections.
    Use { node_id: String },
    /// Probe node entrances or end-to-end availability.
    Probe {
        node_id: Option<String>,
        /// Probe every node.
        #[arg(long)]
        all: bool,
        /// entrance or availability.
        #[arg(long = "type", default_value = "entrance")]
        probe_type: String,
        /// Per-probe timeout, such as 5s or 500ms.
        #[arg(long, default_value = "5s")]
        timeout: String,
        /// Entrance probe concurrency.
        #[arg(long, default_value_t = 4)]
        concurrency: i64,
        /// Availability probe target URL.
        #[arg(long, default_value = DEFAULT_AVAILABILITY_TARGET)]
        target: String,
    },
    /// Show cumulative traffic.
    Traffic,
    /// Show active connections.
    Connections,
    /// Show local proxy endpoints.
    Proxy {
        #[command(subcommand)]
        command: Option<ProxyCommand>,
    },
    /// Show or set the routing mode (rules or global).
    Mode { mode: Option<String> },
    /// Show node ingresses, their health and pins.
    #[command(args_conflicts_with_subcommands = true)]
    Ingress {
        node_id: Option<String>,
        #[command(subcommand)]
        command: Option<IngressCommand>,
    },
    /// Run privacy-safe local diagnostics.
    Doctor,
    /// Generate a shell completion script (bash, zsh or fish).
    Completion { shell: String },
}

#[derive(Debug, Subcommand)]
pub enum ProxyCommand {
    /// Show a local proxy credential: the routed one, or a node's.
    Credential { node_id: Option<String> },
}

#[derive(Debug, Subcommand)]
pub enum IngressCommand {
    /// Use only one ingress for a node.
    Pin {
        node_id: String,
        endpoint_key: String,
    },
    /// Restore automatic ingress failover for a node.
    Auto { node_id: String },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeType {
    Entrance,
    Availability,
}

fn non_empty(value: &str, message: &str) -> Result<()> {
    if value.trim().is_empty() {
        Err(CliError::argument(message))
    } else {
        Ok(())
    }
}

impl Command {
    /// Checks argument values; `json` is the global `--json` flag.
    pub fn validate(&self, json: bool) -> Result<()> {
        match self {
            Command::Use { node_id } => non_empty(node_id, "use requires exactly one node ID"),
            Command::Probe { .. } => self.probe_settings().map(|_| ()),
            Command::Proxy {
                command: Some(ProxyCommand::Credential { node_id: Some(id) }),
            } => non_empty(id, "proxy credential accepts at most one node ID"),
            Command::Mode { mode: Some(mode) } => RoutingMode::parse(mode)
                .map(|_| ())
                .ok_or_else(|| CliError::argument("mode must be rules or global")),
            Command::Ingress {
                node_id: Some(id), ..
            } => non_empty(id, "ingress accepts at most one node ID"),
            Command::Ingress {
                command:
                    Some(IngressCommand::Pin {
                        node_id,
                        endpoint_key,
                    }),
                ..
            } => {
                non_empty(
                    node_id,
                    "ingress pin requires a node ID and an endpoint key",
                )?;
                non_empty(
                    endpoint_key,
                    "ingress pin requires a node ID and an endpoint key",
                )
            }
            Command::Ingress {
                command: Some(IngressCommand::Auto { node_id }),
                ..
            } => non_empty(node_id, "ingress auto requires exactly one node ID"),
            Command::Completion { shell } => {
                if json {
                    return Err(CliError::argument(
                        "completion scripts cannot be generated with --json",
                    ));
                }
                match shell.as_str() {
                    "bash" | "zsh" | "fish" => Ok(()),
                    _ => Err(CliError::argument(
                        "supported shells are bash, zsh and fish",
                    )),
                }
            }
            _ => Ok(()),
        }
    }

    /// The checked probe settings: type, timeout and concurrency.
    pub fn probe_settings(&self) -> Result<(ProbeType, Duration, u32)> {
        let Command::Probe {
            node_id,
            all,
            probe_type,
            timeout,
            concurrency,
            ..
        } = self
        else {
            return Err(CliError::argument("not a probe command"));
        };
        let timeout = parse_duration(timeout)
            .filter(|t| !t.is_zero() && *t <= Duration::from_secs(120))
            .ok_or_else(|| CliError::argument("probe timeout must be between 1ms and 2m"))?;
        match probe_type.as_str() {
            "entrance" => {
                if *all == node_id.is_some() {
                    return Err(CliError::argument(
                        "entrance probe requires one node ID or --all",
                    ));
                }
                if !(1..=32).contains(concurrency) {
                    return Err(CliError::argument(
                        "probe concurrency must be between 1 and 32",
                    ));
                }
                Ok((ProbeType::Entrance, timeout, *concurrency as u32))
            }
            "availability" => {
                if *all || node_id.is_none() {
                    return Err(CliError::argument(
                        "availability probe requires exactly one node ID",
                    ));
                }
                Ok((ProbeType::Availability, timeout, 1))
            }
            _ => Err(CliError::argument(
                "probe type must be entrance or availability",
            )),
        }
    }
}

/// Parses durations written like `5s`, `500ms`, `2m` or `1m30s`.
pub fn parse_duration(text: &str) -> Option<Duration> {
    let mut total = Duration::ZERO;
    let mut rest = text.trim();
    if rest.is_empty() {
        return None;
    }
    while !rest.is_empty() {
        let digits = rest
            .find(|c: char| !c.is_ascii_digit() && c != '.')
            .unwrap_or(rest.len());
        let value: f64 = rest[..digits].parse().ok()?;
        rest = &rest[digits..];
        let unit_end = rest
            .find(|c: char| c.is_ascii_digit() || c == '.')
            .unwrap_or(rest.len());
        let seconds = match &rest[..unit_end] {
            "ms" => value / 1000.0,
            "s" => value,
            "m" => value * 60.0,
            "h" => value * 3600.0,
            _ => return None,
        };
        total += Duration::try_from_secs_f64(seconds).ok()?;
        rest = &rest[unit_end..];
    }
    Some(total)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn durations_parse_like_the_go_cli() {
        assert_eq!(parse_duration("5s"), Some(Duration::from_secs(5)));
        assert_eq!(parse_duration("500ms"), Some(Duration::from_millis(500)));
        assert_eq!(parse_duration("1m30s"), Some(Duration::from_secs(90)));
        assert_eq!(parse_duration("1.5s"), Some(Duration::from_millis(1500)));
        for bad in ["", "5", "5x", "s", "-1s"] {
            assert_eq!(parse_duration(bad), None, "{bad}");
        }
    }
}
