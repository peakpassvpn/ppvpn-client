//! Commands that read from or change the running instance through the
//! daemon: nodes, selection, probes, traffic, connections, the local proxy
//! and ingress pins. Without a daemon they report `CORE_NOT_RUNNING`.

use std::time::Duration;

use serde_json::{json, Value};

use crate::cli::ProbeType;
use crate::client::{self, Client};
use crate::commands::{output_error, runtime};
use crate::control::Call;
use crate::env::Env;
use crate::error::{from_core, CliError, Exit, Result};
use crate::output::Printer;
use crate::paths::Paths;
use crate::settings::{RoutingMode, Settings};

/// On top of the probes' own timeouts: the control round trip and core's
/// bookkeeping.
const PROBE_MARGIN: Duration = Duration::from_secs(30);

fn not_running() -> CliError {
    CliError::new(
        Exit::Core,
        "CORE_NOT_RUNNING",
        "the local proxy is not running; run ppvpn start",
    )
    // As core's own CORE_NOT_RUNNING: it works once the instance runs.
    .retryable()
}

fn connect(paths: &Paths) -> Result<Client> {
    if client::running_daemon(paths).is_none() {
        return Err(not_running());
    }
    Client::new(paths)
}

fn call(paths: &Paths, call: Call) -> Result<Value> {
    let client = connect(paths)?;
    runtime()?.block_on(client.call(call))
}

fn text(value: &Value) -> &str {
    value.as_str().unwrap_or_default()
}

/// Merges `fields` (a JSON object from core) into `{"ok": true}`.
fn ok_with(fields: Value) -> Value {
    let mut value = json!({"ok": true});
    if let (Value::Object(target), Value::Object(fields)) = (&mut value, fields) {
        target.extend(fields);
    }
    value
}

pub fn nodes(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let data = call(&paths, Call::Nodes)?;
    let human = nodes_text(&data);
    out.success(&ok_with(data), &human).map_err(output_error)
}

fn nodes_text(data: &Value) -> String {
    let selected = text(&data["selected_node_id"]);
    let lines: Vec<String> = data["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|node| {
            let id = text(&node["id"]);
            let marker = if id == selected { '*' } else { ' ' };
            let mut line = format!("{marker} {id}\t{}", text(&node["name"]));
            for extra in [&node["region"], &node["entry_label"], &node["protocol"]] {
                if !text(extra).is_empty() {
                    line.push('\t');
                    line.push_str(text(extra));
                }
            }
            line
        })
        .collect();
    if lines.is_empty() {
        "No nodes.".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn use_node(env: &Env, out: &mut Printer, node_id: &str) -> Result<()> {
    let paths = Paths::resolve(env)?;
    call(
        &paths,
        Call::SelectNode {
            node_id: node_id.to_string(),
        },
    )?;
    // Core does not persist the selection: the next apply passes it.
    let mut settings = Settings::load(&paths.settings)?;
    settings.selected_node_id = Some(node_id.to_string());
    settings.save(&paths.settings)?;
    out.success(
        &json!({"ok": true, "selected_node_id": node_id}),
        &format!("Selected node: {node_id} (new connections use it)"),
    )
    .map_err(output_error)
}

/// Applies `mode` to the running instance. `false`: nothing is running or
/// nothing is applied, so the mode only takes effect with the next start.
pub fn set_routing_mode(paths: &Paths, mode: RoutingMode) -> Result<bool> {
    if client::running_daemon(paths).is_none() {
        return Ok(false);
    }
    let routing_mode = match mode {
        RoutingMode::Rules => ppvpn_core::RoutingMode::Rules,
        RoutingMode::Global => ppvpn_core::RoutingMode::Global,
    };
    match call(paths, Call::SetRoutingMode { routing_mode }) {
        Ok(_) => Ok(true),
        Err(err) if err.code == "PROFILE_NOT_APPLIED" => Ok(false),
        Err(err) => Err(err),
    }
}

pub fn traffic(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let data = call(&paths, Call::Traffic)?;
    let human = format!(
        "Upload: {}\nDownload: {}",
        bytes_text(data["upload_bytes"].as_u64().unwrap_or(0)),
        bytes_text(data["download_bytes"].as_u64().unwrap_or(0)),
    );
    out.success(&ok_with(data), &human).map_err(output_error)
}

/// Decimal units, as the backend counts traffic: 1 KB = 1000 bytes, up to
/// 1 TB = 10^12. `1.5 MB (1500000 bytes)`; plain bytes below 1 KB.
fn bytes_text(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["KB", "MB", "GB", "TB"];
    if bytes < 1000 {
        return format!("{bytes} B");
    }
    let mut value = bytes as f64 / 1000.0;
    let mut unit = 0;
    // Shown to one decimal: 999.96 KB would read "1000.0 KB", so it is 1.0 MB.
    while value >= 999.95 && unit + 1 < UNITS.len() {
        value /= 1000.0;
        unit += 1;
    }
    format!("{value:.1} {} ({bytes} bytes)", UNITS[unit])
}

pub fn connections(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let data = call(&paths, Call::Connections)?;
    let lines: Vec<String> = data
        .as_array()
        .into_iter()
        .flatten()
        .map(|c| {
            format!(
                "{}\t{}\t{}\tup {}\tdown {}",
                text(&c["node_id"]),
                text(&c["network"]),
                text(&c["destination"]),
                c["upload_bytes"].as_u64().unwrap_or(0),
                c["download_bytes"].as_u64().unwrap_or(0),
            )
        })
        .collect();
    let human = if lines.is_empty() {
        "No active connections.".to_string()
    } else {
        lines.join("\n")
    };
    out.success(&json!({"ok": true, "connections": data}), &human)
        .map_err(output_error)
}

pub fn probe(
    env: &Env,
    out: &mut Printer,
    probe: ProbeType,
    node_id: Option<&str>,
    target: &str,
    timeout: Duration,
    concurrency: u32,
) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let client = connect(&paths)?;
    let timeout_ms = timeout.as_millis() as u64;
    match probe {
        ProbeType::Availability => {
            let request = ppvpn_core::ProbeAvailabilityRequest::new(
                node_id.unwrap_or_default(),
                target,
                timeout_ms,
            );
            let data = runtime()?.block_on(
                client.call_within(Call::ProbeAvailability(request), timeout + PROBE_MARGIN),
            )?;
            let human = availability_text(&data);
            out.success(
                &json!({"ok": true, "type": "availability", "result": data}),
                &human,
            )
            .map_err(output_error)
        }
        ProbeType::Entrance => {
            let request = ppvpn_core::ProbeEntrancesRequest::new(
                ppvpn_core::ProbeMethod::Tcp,
                timeout_ms,
                concurrency,
            )
            .with_node_ids(node_id.map(str::to_string).into_iter().collect());
            let data = runtime()?.block_on(async {
                // Every node is probed in rounds of `concurrency`.
                let nodes = match node_id {
                    Some(_) => 1,
                    None => client.call(Call::Status).await?["node_count"]
                        .as_u64()
                        .unwrap_or(1)
                        .max(1),
                };
                let rounds = nodes.div_ceil(u64::from(concurrency)) as u32;
                client
                    .call_within(
                        Call::ProbeEntrances(request),
                        timeout * rounds + PROBE_MARGIN,
                    )
                    .await
            })?;
            let human = entrances_text(&data);
            out.success(
                &json!({"ok": true, "type": "entrance", "results": data}),
                &human,
            )
            .map_err(output_error)
        }
    }
}

fn outcome(result: &Value) -> String {
    if result["success"].as_bool() == Some(true) {
        return "ok".to_string();
    }
    match text(&result["error_code"]) {
        "" => "failed".to_string(),
        code => format!("failed ({code})"),
    }
}

fn availability_text(result: &Value) -> String {
    let mut line = format!("{}\t{}", text(&result["node_id"]), outcome(result));
    if let Some(status) = result["http_status"].as_u64().filter(|s| *s != 0) {
        line.push_str(&format!("\tHTTP {status}"));
    }
    if result["success"].as_bool() == Some(true) {
        line.push_str(&format!(
            "\t{} ms",
            result["total_ms"].as_i64().unwrap_or(0)
        ));
    }
    line
}

fn entrances_text(results: &Value) -> String {
    let mut lines = Vec::new();
    for result in results.as_array().into_iter().flatten() {
        let mut line = format!("{}\t{}", text(&result["node_id"]), outcome(result));
        if result["success"].as_bool() == Some(true) {
            line.push_str(&format!(
                "\t{} ms\tvia {}",
                result["latency_ms"].as_i64().unwrap_or(0),
                text(&result["endpoint_key"]),
            ));
        }
        lines.push(line);
        for ingress in result["ingresses"].as_array().into_iter().flatten() {
            let mut line = format!(
                "  {}\t{}\t{}",
                text(&ingress["endpoint_key"]),
                text(&ingress["role"]),
                outcome(ingress),
            );
            if ingress["success"].as_bool() == Some(true) {
                line.push_str(&format!(
                    "\t{} ms",
                    ingress["latency_ms"].as_i64().unwrap_or(0)
                ));
            }
            lines.push(line);
        }
    }
    if lines.is_empty() {
        "No nodes.".to_string()
    } else {
        lines.join("\n")
    }
}

pub fn proxy(env: &Env, out: &mut Printer) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let data = call(&paths, Call::ProxyEndpoints)?;
    let lines: Vec<String> = data
        .as_array()
        .into_iter()
        .flatten()
        .map(|endpoint| {
            let protocols: Vec<&str> = endpoint["protocols"]
                .as_array()
                .into_iter()
                .flatten()
                .map(text)
                .collect();
            let who = match text(&endpoint["node_id"]) {
                "" => "routed".to_string(),
                node => format!("node {node}"),
            };
            format!(
                "{who}\t{}\t{}",
                authority(text(&endpoint["listen"]), endpoint["port"].as_u64()),
                protocols.join(","),
            )
        })
        .collect();
    let human = if lines.is_empty() {
        "No local proxy endpoints.".to_string()
    } else {
        format!(
            "{}\n\nCredentials: ppvpn proxy credential [node-id]",
            lines.join("\n")
        )
    };
    out.success(&json!({"ok": true, "endpoints": data}), &human)
        .map_err(output_error)
}

/// The routed credential, or a node's. The password is the command's
/// output: it is what the user copies into another application.
pub fn proxy_credential(env: &Env, out: &mut Printer, node_id: Option<&str>) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let data = call(
        &paths,
        Call::ProxyCredential {
            node_id: node_id.map(str::to_string),
        },
    )?;
    let (http, socks) = proxy_urls(
        text(&data["listen"]),
        data["port"].as_u64(),
        text(&data["username"]),
        text(&data["password"]),
    );
    let human = format!(
        "HTTP:   {http}\nSOCKS5: {socks}\n\nUsername: {}\nPassword: {}",
        text(&data["username"]),
        text(&data["password"]),
    );
    let mut value = ok_with(data);
    value["http_url"] = json!(http);
    value["socks5_url"] = json!(socks);
    out.success(&value, &human).map_err(output_error)
}

fn authority(listen: &str, port: Option<u64>) -> String {
    let port = port.unwrap_or(0);
    if listen.contains(':') {
        format!("[{listen}]:{port}")
    } else {
        format!("{listen}:{port}")
    }
}

/// `http://` and `socks5h://` URLs (names resolve at the proxy, so rules
/// see domains).
fn proxy_urls(listen: &str, port: Option<u64>, username: &str, password: &str) -> (String, String) {
    let rest = format!(
        "{}:{}@{}",
        userinfo(username),
        userinfo(password),
        authority(listen, port)
    );
    (format!("http://{rest}"), format!("socks5h://{rest}"))
}

/// Percent-encodes everything outside RFC 3986's unreserved set.
fn userinfo(value: &str) -> String {
    let mut encoded = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            encoded.push(byte as char);
        } else {
            encoded.push_str(&format!("%{byte:02X}"));
        }
    }
    encoded
}

pub fn ingress(env: &Env, out: &mut Printer, node_id: Option<&str>) -> Result<()> {
    let paths = Paths::resolve(env)?;
    let status = call(&paths, Call::Status)?;
    let nodes: Vec<Value> = status["nodes"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|node| node_id.is_none_or(|id| text(&node["node_id"]) == id))
        .cloned()
        .collect();
    if let (Some(id), true) = (node_id, nodes.is_empty()) {
        return Err(from_core(
            "NODE_NOT_FOUND",
            Some("node_id".to_string()),
            false,
            &format!("node {id} is not in the applied profile"),
        ));
    }
    let human = ingress_text(&nodes);
    out.success(&json!({"ok": true, "nodes": nodes}), &human)
        .map_err(output_error)
}

fn ingress_text(nodes: &[Value]) -> String {
    let mut lines = Vec::new();
    for node in nodes {
        let pin = match node["pinned_endpoint_key"].as_str() {
            Some(key) => format!("pinned to {key}"),
            None => "automatic".to_string(),
        };
        lines.push(format!("{} ({pin})", text(&node["node_id"])));
        for ingress in node["ingresses"].as_array().into_iter().flatten() {
            let marker = if ingress["active"].as_bool() == Some(true) {
                '*'
            } else {
                ' '
            };
            let health = match ingress["healthy"].as_bool() {
                Some(true) => "healthy".to_string(),
                Some(false) => format!(
                    "unhealthy ({} failures)",
                    ingress["consecutive_failures"].as_u64().unwrap_or(0)
                ),
                None => "not checked".to_string(),
            };
            let mut line = format!(
                "  {marker} {}\t{}\t{health}",
                text(&ingress["endpoint_key"]),
                text(&ingress["role"]),
            );
            if !text(&ingress["label"]).is_empty() {
                line.push('\t');
                line.push_str(text(&ingress["label"]));
            }
            lines.push(line);
        }
    }
    if lines.is_empty() {
        "No nodes.".to_string()
    } else {
        lines.join("\n")
    }
}

/// Pins a node to one ingress, or back to automatic failover with `None`.
pub fn pin(env: &Env, out: &mut Printer, node_id: &str, endpoint_key: Option<&str>) -> Result<()> {
    let paths = Paths::resolve(env)?;
    call(
        &paths,
        Call::PinIngress {
            node_id: node_id.to_string(),
            endpoint_key: endpoint_key.map(str::to_string),
        },
    )?;
    // Core does not persist pins: the next apply passes them.
    let mut settings = Settings::load(&paths.settings)?;
    let human = match endpoint_key {
        Some(key) => {
            settings
                .ingress_pins
                .insert(node_id.to_string(), key.to_string());
            format!("Node {node_id}: pinned to ingress {key} (no failover while pinned)")
        }
        None => {
            settings.ingress_pins.remove(node_id);
            format!("Node {node_id}: automatic ingress failover")
        }
    };
    settings.save(&paths.settings)?;
    out.success(
        &json!({"ok": true, "node_id": node_id, "pinned_endpoint_key": endpoint_key}),
        &human,
    )
    .map_err(output_error)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn proxy_urls_encode_the_credential_and_bracket_ipv6() {
        assert_eq!(userinfo("a@b:c/d e"), "a%40b%3Ac%2Fd%20e");
        assert_eq!(userinfo("pp-ab12._~"), "pp-ab12._~");
        // Unique to this run; the punctuation must come out encoded.
        let secret = format!("{}@{}", std::process::id(), std::process::id());
        let encoded = secret.replace('@', "%40");
        let (http, socks) = proxy_urls("127.0.0.1", Some(7890), "pp-ab12", &secret);
        assert!(http == format!("http://pp-ab12:{encoded}@127.0.0.1:7890"));
        assert!(socks == format!("socks5h://pp-ab12:{encoded}@127.0.0.1:7890"));
        let (http, _) = proxy_urls("::1", Some(7890), "pp-ab12", &secret);
        assert!(http == format!("http://pp-ab12:{encoded}@[::1]:7890"));
    }

    #[test]
    fn byte_counts_are_readable_and_exact() {
        assert_eq!(bytes_text(0), "0 B");
        assert_eq!(bytes_text(999), "999 B");
        assert_eq!(bytes_text(1000), "1.0 KB (1000 bytes)");
        assert_eq!(bytes_text(1024), "1.0 KB (1024 bytes)");
        assert_eq!(bytes_text(1_500_000), "1.5 MB (1500000 bytes)");
        // Not "1000.0 KB": the next unit.
        assert_eq!(bytes_text(999_999), "1.0 MB (999999 bytes)");
        assert_eq!(bytes_text(999_949), "999.9 KB (999949 bytes)");
        assert_eq!(bytes_text(400_000_000_000), "400.0 GB (400000000000 bytes)");
        assert_eq!(
            bytes_text(1_000_000_000_000),
            "1.0 TB (1000000000000 bytes)"
        );
        assert_eq!(bytes_text(u64::MAX).split(' ').nth(1), Some("TB"));
    }

    #[test]
    fn nodes_mark_the_selected_one() {
        let data = json!({
            "selected_node_id": "jp-1",
            "nodes": [
                {"id": "hk-1", "name": "Hong Kong", "region": "HK", "protocol": "vless"},
                {"id": "jp-1", "name": "Tokyo", "protocol": "trojan"},
            ],
        });
        assert_eq!(
            nodes_text(&data),
            "  hk-1\tHong Kong\tHK\tvless\n* jp-1\tTokyo\ttrojan"
        );
        assert_eq!(nodes_text(&json!({"nodes": []})), "No nodes.");
    }

    #[test]
    fn probe_results_show_the_outcome_per_ingress() {
        let results = json!([{
            "node_id": "hk-1", "success": true, "latency_ms": 23, "endpoint_key": "9001",
            "ingresses": [
                {"endpoint_key": "9001", "role": "primary", "success": true, "latency_ms": 23},
                {"endpoint_key": "9002", "role": "backup", "success": false, "error_code": "TIMEOUT"},
            ],
        }]);
        assert_eq!(
            entrances_text(&results),
            "hk-1\tok\t23 ms\tvia 9001\n  9001\tprimary\tok\t23 ms\n  9002\tbackup\tfailed (TIMEOUT)"
        );
        let result =
            json!({"node_id": "hk-1", "success": true, "http_status": 204, "total_ms": 310});
        assert_eq!(availability_text(&result), "hk-1\tok\tHTTP 204\t310 ms");
        let result = json!({"node_id": "hk-1", "success": false, "error_code": "TIMEOUT"});
        assert_eq!(availability_text(&result), "hk-1\tfailed (TIMEOUT)");
    }

    #[test]
    fn ingress_health_shows_the_pin_and_the_active_ingress() {
        let nodes = [json!({
            "node_id": "hk-1", "pinned_endpoint_key": "9002",
            "ingresses": [
                {"endpoint_key": "9001", "role": "primary", "healthy": false, "consecutive_failures": 3, "active": false},
                {"endpoint_key": "9002", "role": "backup", "healthy": true, "consecutive_failures": 0, "active": true, "label": "B"},
                {"endpoint_key": "9003", "role": "backup", "consecutive_failures": 0, "active": false},
            ],
        })];
        assert_eq!(
            ingress_text(&nodes),
            "hk-1 (pinned to 9002)\n    9001\tprimary\tunhealthy (3 failures)\n  * 9002\tbackup\thealthy\tB\n    9003\tbackup\tnot checked"
        );
    }
}
