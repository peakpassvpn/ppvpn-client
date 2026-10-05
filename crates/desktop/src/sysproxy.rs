//! OS system-proxy settings for compatible mode, behind [`SystemProxyWriter`]
//! so tests never touch the real OS.
//!
//! - macOS: `networksetup` on every enabled network service (web, secure web
//!   and SOCKS proxies plus bypass domains). Works without `sudo` for
//!   accounts in the admin group; standard accounts get
//!   `ADMIN_REQUIRED`.
//! - Windows: the WinINet values under `HKCU\…\Internet Settings`
//!   (`ProxyEnable`, `ProxyServer`, `ProxyOverride`; switched off while the
//!   others change), then `InternetSetOption(SETTINGS_CHANGED, REFRESH)`.
//! - Linux: `gsettings org.gnome.system.proxy` (keys that were at their
//!   default are reset on restore) and, when present, `kwriteconfig6/5`
//!   (KDE `kioslaverc`).
//!
//! The writer returns the previous settings as an opaque JSON value; the
//! caller persists it before applying and hands it back to `restore`.

use std::sync::Arc;

use serde_json::Value;

/// Hosts that never go through the proxy.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) const BYPASS: &[&str] = &["localhost", "127.0.0.1", "::1", "*.local"];
/// Private IPv4 ranges, for OSes that accept CIDR / wildcard entries.
#[cfg_attr(windows, allow(dead_code))]
pub(crate) const PRIVATE_RANGES: &[&str] = &["10.0.0.0/8", "172.16.0.0/12", "192.168.0.0/16"];

/// KDE `NoProxyFor`: the same hosts and private ranges as GNOME's
/// `ignore-hosts` (KIO accepts CIDR), comma-separated.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) fn kde_no_proxy_for() -> String {
    let hosts: Vec<&str> = BYPASS.iter().chain(PRIVATE_RANGES).copied().collect();
    hosts.join(",")
}

/// Reads, applies and restores the OS system-proxy settings. Blocking; call
/// from `spawn_blocking`.
pub(crate) trait SystemProxyWriter: Send + Sync {
    /// The current settings, to be handed back to [`Self::restore`].
    fn snapshot(&self) -> Result<Value, String>;
    /// Points HTTP, HTTPS and SOCKS at `host:port` with the bypass list.
    fn apply(&self, host: &str, port: u16) -> Result<(), String>;
    /// Puts back what [`Self::snapshot`] returned.
    fn restore(&self, saved: &Value) -> Result<(), String>;
    /// Every proxy `host:port` the OS settings point at now (none when the
    /// OS proxy is off), to tell whether they are still ours.
    fn current_proxies(&self) -> Result<Vec<(String, u16)>, String> {
        self.snapshot().map(|saved| active_proxies(&saved))
    }
}

/// `proxies` (see [`SystemProxyWriter::current_proxies`]) include
/// `host:port`; any loopback host matches any other.
pub(crate) fn points_at(proxies: &[(String, u16)], host: &str, port: u16) -> bool {
    proxies.iter().any(|(proxy_host, proxy_port)| {
        *proxy_port == port
            && (proxy_host.eq_ignore_ascii_case(host)
                || (is_loopback_host(proxy_host) && is_loopback_host(host)))
    })
}

/// The data of a `reg query` line `    <name>    <REG_TYPE>    <data>`,
/// kept exactly (inner and trailing spaces included, only the line end
/// stripped).
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) fn parse_reg_value_line(line: &str, name: &str) -> Option<String> {
    let rest = line.trim_start().strip_prefix(name)?;
    if !rest.starts_with(char::is_whitespace) {
        return None; // a longer value name with the same prefix
    }
    let rest = rest.trim_start();
    let kind_end = rest.find(char::is_whitespace).unwrap_or(rest.len());
    if !rest[..kind_end].starts_with("REG_") {
        return None;
    }
    // reg.exe separates columns with four spaces.
    let data = rest[kind_end..]
        .strip_prefix("    ")
        .unwrap_or(rest[kind_end..].trim_start());
    Some(data.trim_end_matches(['\r', '\n']).to_string())
}

/// The proxy `host:port` a [`SystemProxyWriter::snapshot`] shows as active,
/// in any platform's format (`None` when the OS proxy was off).
pub(crate) fn active_proxy(saved: &Value) -> Option<(String, u16)> {
    active_proxies(saved).into_iter().next()
}

/// Every proxy `host:port` a [`SystemProxyWriter::snapshot`] shows as
/// active (all enabled services and kinds on macOS, every scheme elsewhere),
/// in the order [`active_proxy`] picks from.
pub(crate) fn active_proxies(saved: &Value) -> Vec<(String, u16)> {
    fn host_port(text: &str) -> Option<(String, u16)> {
        let text = text.trim().trim_matches('\'');
        let text = text.split("://").last().unwrap_or(text);
        let (host, port) = match text.rsplit_once(':') {
            Some((host, port)) => (host, port),
            None => text.rsplit_once(' ')?,
        };
        let port = port.trim().trim_end_matches('/').parse::<u16>().ok()?;
        let host = host.trim().trim_matches(['[', ']']);
        (!host.is_empty() && port > 0).then(|| (host.to_string(), port))
    }
    if let Some(services) = saved.get("macos").and_then(Value::as_object) {
        return services
            .values()
            .flat_map(|entry| {
                ["web", "secure", "socks"].iter().filter_map(move |kind| {
                    let proxy = &entry[*kind];
                    if !proxy["enabled"].as_bool().unwrap_or(false) {
                        return None;
                    }
                    let server = proxy["server"].as_str()?;
                    let port = proxy["port"].as_str()?.parse::<u16>().ok()?;
                    (!server.is_empty() && port > 0).then(|| (server.to_string(), port))
                })
            })
            .collect();
    }
    if let Some(windows) = saved.get("windows") {
        let enabled = windows["ProxyEnable"]
            .as_str()
            .map(|value| value.trim() != "0x0" && value.trim() != "0")
            .unwrap_or(false);
        if !enabled {
            return Vec::new();
        }
        // `host:port`, or `http=host:port;https=…;socks=…`.
        let Some(server) = windows["ProxyServer"].as_str() else {
            return Vec::new();
        };
        return server
            .split(';')
            .map(|entry| entry.split_once('=').map_or(entry, |(_, value)| value))
            .filter_map(host_port)
            .collect();
    }
    let mut found = Vec::new();
    if let Some(linux) = saved.get("linux") {
        let gnome = &linux["gnome"];
        let manual = gnome["org.gnome.system.proxy mode"]
            .as_str()
            .is_some_and(|mode| mode.contains("manual"));
        if manual {
            for scheme in ["http", "https", "socks"] {
                let host = gnome[format!("org.gnome.system.proxy.{scheme} host")]
                    .as_str()
                    .unwrap_or_default()
                    .trim_matches('\'');
                let port = gnome[format!("org.gnome.system.proxy.{scheme} port")]
                    .as_str()
                    .and_then(|port| port.trim().parse::<u16>().ok())
                    .unwrap_or(0);
                if !host.is_empty() && port > 0 {
                    found.push((host.to_string(), port));
                }
            }
        }
        let kde = &linux["kde"];
        if kde["ProxyType"].as_str() == Some("1") {
            found.extend(
                ["httpProxy", "httpsProxy", "socksProxy"]
                    .iter()
                    .filter_map(|key| kde[*key].as_str())
                    .filter_map(host_port),
            );
        }
    }
    found
}

/// Whether `host` is this machine.
pub(crate) fn is_loopback_host(host: &str) -> bool {
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

/// The writer for the OS this build targets.
pub(crate) fn platform_writer() -> Arc<dyn SystemProxyWriter> {
    Arc::new(os::OsWriter)
}

#[cfg(any(target_os = "macos", target_os = "linux", windows))]
fn run(program: &str, args: &[&str]) -> Result<String, String> {
    let mut command = std::process::Command::new(program);
    command.args(args);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    let output = command
        .output()
        .map_err(|error| format!("{program}: {error}"))?;
    let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
    let stderr = String::from_utf8_lossy(&output.stderr).into_owned();
    if output.status.success() && !stdout.contains("** Error") {
        Ok(stdout)
    } else {
        Err(format!(
            "{program} {}: {}{}",
            args.first().unwrap_or(&""),
            stdout.trim(),
            stderr.trim()
        ))
    }
}

// ---------------------------------------------------------------------------
// macOS
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
mod os {
    use super::{run, SystemProxyWriter, BYPASS, PRIVATE_RANGES};
    use serde_json::{json, Map, Value};

    const NETWORKSETUP: &str = "/usr/sbin/networksetup";
    /// (snapshot key, networksetup command stem)
    const KINDS: &[(&str, &str)] = &[
        ("web", "webproxy"),
        ("secure", "securewebproxy"),
        ("socks", "socksfirewallproxy"),
    ];

    pub(crate) struct OsWriter;

    fn networksetup(args: &[&str]) -> Result<String, String> {
        run(NETWORKSETUP, args).map_err(|error| {
            if error.contains("requires admin") {
                format!("ADMIN_REQUIRED: networksetup needs an administrator account ({error})")
            } else {
                error
            }
        })
    }

    /// Enabled network services (disabled ones are marked with `*`).
    pub(super) fn services(listing: &str) -> Vec<String> {
        listing
            .lines()
            .skip(1)
            .map(str::trim)
            .filter(|line| !line.is_empty() && !line.starts_with('*'))
            .map(str::to_string)
            .collect()
    }

    /// `Enabled`, `Server`, `Port` from a `-get…proxy` listing.
    pub(super) fn parse_proxy(listing: &str) -> Value {
        let field = |name: &str| {
            listing
                .lines()
                .find_map(|line| line.strip_prefix(&format!("{name}:")))
                .map(|value| value.trim().to_string())
                .unwrap_or_default()
        };
        json!({
            "enabled": field("Enabled") == "Yes",
            "server": field("Server"),
            "port": field("Port"),
        })
    }

    pub(super) fn parse_bypass(listing: &str) -> Vec<String> {
        if listing.contains("There aren't any") {
            return Vec::new();
        }
        listing
            .lines()
            .map(str::trim)
            .filter(|line| !line.is_empty())
            .map(str::to_string)
            .collect()
    }

    impl SystemProxyWriter for OsWriter {
        fn snapshot(&self) -> Result<Value, String> {
            let mut saved = Map::new();
            for service in services(&networksetup(&["-listallnetworkservices"])?) {
                let mut entry = Map::new();
                for (key, stem) in KINDS {
                    let listing = networksetup(&[&format!("-get{stem}"), &service])?;
                    entry.insert((*key).to_string(), parse_proxy(&listing));
                }
                let bypass = networksetup(&["-getproxybypassdomains", &service])?;
                entry.insert("bypass".into(), json!(parse_bypass(&bypass)));
                saved.insert(service, Value::Object(entry));
            }
            Ok(json!({ "macos": saved }))
        }

        fn apply(&self, host: &str, port: u16) -> Result<(), String> {
            let port = port.to_string();
            let mut bypass: Vec<&str> = BYPASS.to_vec();
            bypass.extend(PRIVATE_RANGES);
            for service in services(&networksetup(&["-listallnetworkservices"])?) {
                for (_, stem) in KINDS {
                    networksetup(&[&format!("-set{stem}"), &service, host, &port])?;
                    networksetup(&[&format!("-set{stem}state"), &service, "on"])?;
                }
                let mut args = vec!["-setproxybypassdomains", service.as_str()];
                args.extend(bypass.iter().copied());
                networksetup(&args)?;
            }
            Ok(())
        }

        fn restore(&self, saved: &Value) -> Result<(), String> {
            let Some(services) = saved.get("macos").and_then(Value::as_object) else {
                return Ok(());
            };
            let mut first_error = None;
            for (service, entry) in services {
                for (key, stem) in KINDS {
                    let proxy = &entry[*key];
                    let server = proxy["server"].as_str().unwrap_or_default();
                    let port = proxy["port"].as_str().unwrap_or_default();
                    let enabled = proxy["enabled"].as_bool().unwrap_or(false);
                    let mut result = Ok(String::new());
                    if !server.is_empty() && !port.is_empty() && port != "0" {
                        result = networksetup(&[&format!("-set{stem}"), service, server, port]);
                    }
                    let state = if enabled && !server.is_empty() {
                        "on"
                    } else {
                        "off"
                    };
                    let result = result
                        .and_then(|_| networksetup(&[&format!("-set{stem}state"), service, state]));
                    if let Err(error) = result {
                        first_error.get_or_insert(error);
                    }
                }
                let bypass: Vec<String> = entry["bypass"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|item| item.as_str().map(str::to_string))
                            .collect()
                    })
                    .unwrap_or_default();
                let mut args: Vec<&str> = vec!["-setproxybypassdomains", service];
                if bypass.is_empty() {
                    args.push("Empty");
                } else {
                    args.extend(bypass.iter().map(String::as_str));
                }
                if let Err(error) = networksetup(&args) {
                    first_error.get_or_insert(error);
                }
            }
            first_error.map_or(Ok(()), Err)
        }
    }
}

// ---------------------------------------------------------------------------
// Windows
// ---------------------------------------------------------------------------

/// The WinINet registry writes as steps, run by the Windows writer and kept
/// apart from the registry so their order is testable on any OS.
#[cfg_attr(not(windows), allow(dead_code))]
pub(crate) mod wininet {
    use serde_json::Value;

    pub(crate) const ENABLE: &str = "ProxyEnable";
    pub(crate) const SERVER: &str = "ProxyServer";
    pub(crate) const OVERRIDE: &str = "ProxyOverride";

    #[derive(Debug, Clone, PartialEq, Eq)]
    pub(crate) enum Step {
        /// `reg add` of one value under `Internet Settings`.
        Set {
            name: &'static str,
            kind: &'static str,
            data: String,
        },
        /// `reg delete` of one value.
        Delete(&'static str),
        /// `InternetSetOption(SETTINGS_CHANGED)` and `(REFRESH)`.
        Notify,
    }

    fn sz(name: &'static str, data: String) -> Step {
        Step::Set {
            name,
            kind: "REG_SZ",
            data,
        }
    }

    fn dword(name: &'static str, data: &str) -> Step {
        Step::Set {
            name,
            kind: "REG_DWORD",
            data: data.to_string(),
        }
    }

    /// WinINet bypass list: loopback, `.local`, the private ranges as
    /// wildcards, and `<local>` (plain host names).
    pub(crate) fn override_list() -> String {
        let mut entries = vec![
            "localhost".to_string(),
            "127.*".to_string(),
            "[::1]".to_string(),
            "*.local".to_string(),
            "10.*".to_string(),
            "192.168.*".to_string(),
        ];
        entries.extend((16..=31).map(|octet| format!("172.{octet}.*")));
        entries.push("<local>".to_string());
        entries.join(";")
    }

    /// Points WinINet at `host:port`. The proxy is switched off before the
    /// server and bypass list change and on again only after both are
    /// ours, so it is never on with our server and the user's bypass list
    /// (or the reverse). Stops at the first failure; notifies either way.
    pub(crate) fn apply(
        host: &str,
        port: u16,
        mut exec: impl FnMut(&Step) -> Result<(), String>,
    ) -> Result<(), String> {
        // One mixed endpoint: WinINet sends HTTPS as CONNECT to it. A
        // `socks=` entry would make WinINet speak SOCKS4, so none is set.
        let steps = [
            dword(ENABLE, "0"),
            sz(SERVER, format!("{host}:{port}")),
            sz(OVERRIDE, override_list()),
            dword(ENABLE, "1"),
        ];
        let result = steps.iter().try_for_each(&mut exec);
        let notified = exec(&Step::Notify);
        result.and(notified)
    }

    /// Puts back what the snapshot saved (`{ProxyEnable, ProxyServer,
    /// ProxyOverride}`, each the `reg query` data or null when absent),
    /// mirroring [`apply`]: off first, then the server and bypass list,
    /// then the saved `ProxyEnable` (0 when it was absent), then notify.
    /// Best effort: every step is tried and the first error returned. If
    /// the server or bypass list could not be put back the proxy is left
    /// off instead of switched on over a mix of ours and theirs (the
    /// backup is kept and the restore retried at the next launch).
    pub(crate) fn restore(
        saved: &Value,
        mut exec: impl FnMut(&Step) -> Result<(), String>,
    ) -> Result<(), String> {
        let mut first_error = None;
        let mut run = |step: &Step| match exec(step) {
            Ok(()) => true,
            Err(error) => {
                first_error.get_or_insert(error);
                false
            }
        };
        run(&dword(ENABLE, "0"));
        let mut values_back = true;
        for name in [SERVER, OVERRIDE] {
            let step = match saved[name].as_str() {
                Some(data) => sz(name, data.to_string()),
                None => Step::Delete(name),
            };
            values_back &= run(&step);
        }
        if values_back {
            // `reg query` prints DWORDs as 0x…; `reg add` wants decimal.
            let enable = saved[ENABLE].as_str().map_or_else(
                || "0".to_string(),
                |data| {
                    data.strip_prefix("0x")
                        .and_then(|hex| u32::from_str_radix(hex, 16).ok())
                        .map_or_else(|| data.to_string(), |value| value.to_string())
                },
            );
            run(&dword(ENABLE, &enable));
        }
        run(&Step::Notify);
        first_error.map_or(Ok(()), Err)
    }
}

#[cfg(windows)]
mod os {
    use super::wininet::{self, Step, ENABLE, OVERRIDE, SERVER};
    use super::{run, SystemProxyWriter};
    use serde_json::{json, Value};

    const KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Internet Settings";
    const INTERNET_OPTION_REFRESH: u32 = 37;
    const INTERNET_OPTION_SETTINGS_CHANGED: u32 = 39;

    #[link(name = "wininet")]
    extern "system" {
        fn InternetSetOptionW(
            internet: *mut core::ffi::c_void,
            option: u32,
            buffer: *mut core::ffi::c_void,
            length: u32,
        ) -> i32;
    }

    pub(crate) struct OsWriter;

    /// `reg query` value data, `None` when the value is absent.
    fn query(name: &str) -> Option<String> {
        let output = run("reg", &["query", KEY, "/v", name]).ok()?;
        output
            .lines()
            .find_map(|line| super::parse_reg_value_line(line, name))
    }

    fn refresh() {
        // SAFETY: documented global notifications; null handle and buffer.
        unsafe {
            InternetSetOptionW(
                std::ptr::null_mut(),
                INTERNET_OPTION_SETTINGS_CHANGED,
                std::ptr::null_mut(),
                0,
            );
            InternetSetOptionW(
                std::ptr::null_mut(),
                INTERNET_OPTION_REFRESH,
                std::ptr::null_mut(),
                0,
            );
        }
    }

    fn exec(step: &Step) -> Result<(), String> {
        match step {
            Step::Set { name, kind, data } => run(
                "reg",
                &["add", KEY, "/v", name, "/t", kind, "/d", data, "/f"],
            )
            .map(|_| ()),
            Step::Delete(name) => run("reg", &["delete", KEY, "/v", name, "/f"]).map(|_| ()),
            Step::Notify => {
                refresh();
                Ok(())
            }
        }
    }

    impl SystemProxyWriter for OsWriter {
        fn snapshot(&self) -> Result<Value, String> {
            Ok(json!({ "windows": {
                "ProxyEnable": query(ENABLE),
                "ProxyServer": query(SERVER),
                "ProxyOverride": query(OVERRIDE),
            }}))
        }

        fn apply(&self, host: &str, port: u16) -> Result<(), String> {
            wininet::apply(host, port, exec)
        }

        fn restore(&self, saved: &Value) -> Result<(), String> {
            let Some(saved) = saved.get("windows") else {
                return Ok(());
            };
            wininet::restore(saved, exec)
        }
    }
}

// ---------------------------------------------------------------------------
// Linux (GNOME, KDE)
// ---------------------------------------------------------------------------

/// The GNOME `gsettings` snapshot and restore, run by the Linux writer
/// through a `gsettings` runner so they are testable on any OS.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub(crate) mod gnome {
    use serde_json::{json, Map, Value};

    use super::{BYPASS, PRIVATE_RANGES};

    pub(crate) const KEYS: &[(&str, &str)] = &[
        ("org.gnome.system.proxy", "mode"),
        ("org.gnome.system.proxy", "ignore-hosts"),
        ("org.gnome.system.proxy.http", "host"),
        ("org.gnome.system.proxy.http", "port"),
        ("org.gnome.system.proxy.https", "host"),
        ("org.gnome.system.proxy.https", "port"),
        ("org.gnome.system.proxy.socks", "host"),
        ("org.gnome.system.proxy.socks", "port"),
    ];

    /// `ignore-hosts` while connected.
    pub(crate) fn ignore_hosts() -> String {
        let hosts: Vec<String> = BYPASS
            .iter()
            .chain(PRIVATE_RANGES)
            .map(|host| format!("'{host}'"))
            .collect();
        format!("[{}]", hosts.join(", "))
    }

    /// The backup's `gnome` (`"<schema> <key>"` → `gsettings get` output)
    /// and `gnome_defaults` (the keys whose value was the schema default,
    /// which restore resets instead of writing).
    ///
    /// `get` is `gsettings get`; `default` the schema default in the same
    /// format (`None` when unknown: the key is then written back as is).
    pub(crate) fn snapshot(
        mut get: impl FnMut(&str, &str) -> Result<String, String>,
        mut default: impl FnMut(&str, &str) -> Option<String>,
    ) -> Result<(Value, Value), String> {
        let mut values = Map::new();
        let mut defaults = Vec::new();
        for (schema, key) in KEYS {
            let value = get(schema, key)?.trim().to_string();
            let name = format!("{schema} {key}");
            if default(schema, key).is_some_and(|default| default.trim() == value) {
                defaults.push(json!(name));
            }
            values.insert(name, json!(value));
        }
        Ok((Value::Object(values), Value::Array(defaults)))
    }

    /// Puts `gnome` back through `gsettings` (called with its arguments),
    /// mode last so the proxy switches off only once the values are back.
    /// Keys listed in `defaults` are reset rather than written, so no user
    /// value equal to the default is left behind; when the reset does not
    /// yield the saved value (a site default, another backend) the saved
    /// value is written after all. Backups without `defaults` (earlier
    /// builds) write every key. Returns the first error.
    pub(crate) fn restore(
        gnome: &Map<String, Value>,
        defaults: Option<&Value>,
        mut gsettings: impl FnMut(&[&str]) -> Result<String, String>,
    ) -> Option<String> {
        let is_default = |key: &str| {
            defaults
                .and_then(Value::as_array)
                .is_some_and(|keys| keys.iter().any(|item| item.as_str() == Some(key)))
        };
        let mut entries: Vec<(&String, &Value)> = gnome.iter().collect();
        entries.sort_by_key(|(key, _)| key.ends_with(" mode"));
        let mut first_error = None;
        for (key, value) in entries {
            let Some((schema, name)) = key.split_once(' ') else {
                continue;
            };
            let Some(value) = value.as_str().filter(|value| !value.is_empty()) else {
                continue;
            };
            if is_default(key)
                && gsettings(&["reset", schema, name]).is_ok()
                && gsettings(&["get", schema, name]).is_ok_and(|now| now.trim() == value)
            {
                continue;
            }
            if let Err(error) = gsettings(&["set", schema, name, value]) {
                first_error.get_or_insert(error);
            }
        }
        first_error
    }
}

#[cfg(target_os = "linux")]
mod os {
    use super::{gnome, run, SystemProxyWriter};
    use serde_json::{json, Map, Value};

    const KDE_KEYS: &[&str] = &[
        "ProxyType",
        "httpProxy",
        "httpsProxy",
        "socksProxy",
        "NoProxyFor",
    ];

    pub(crate) struct OsWriter;

    fn has(program: &str) -> bool {
        std::process::Command::new(program)
            .arg("--help")
            .output()
            .is_ok()
    }

    fn kde_tools() -> Option<(&'static str, &'static str)> {
        [
            ("kreadconfig6", "kwriteconfig6"),
            ("kreadconfig5", "kwriteconfig5"),
        ]
        .into_iter()
        .find(|(_, write)| has(write))
    }

    fn kde_args(key: &str) -> [&str; 6] {
        [
            "--file",
            "kioslaverc",
            "--group",
            "Proxy Settings",
            "--key",
            key,
        ]
    }

    /// The schema default of `schema key`, as `gsettings get` prints it:
    /// the in-memory backend holds no user values.
    fn schema_default(schema: &str, key: &str) -> Option<String> {
        let output = std::process::Command::new("gsettings")
            .args(["get", schema, key])
            .env("GSETTINGS_BACKEND", "memory")
            .output()
            .ok()?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        (output.status.success() && !stdout.contains("** Error")).then(|| stdout.trim().to_string())
    }

    fn kde_reparse() {
        let _ = run(
            "dbus-send",
            &[
                "--type=signal",
                "/KIO/Scheduler",
                "org.kde.KIO.Scheduler.reparseSlaveConfiguration",
                "string:",
            ],
        );
    }

    impl SystemProxyWriter for OsWriter {
        fn snapshot(&self) -> Result<Value, String> {
            let (gnome, gnome_defaults) = if has("gsettings") {
                gnome::snapshot(
                    |schema, key| run("gsettings", &["get", schema, key]),
                    schema_default,
                )?
            } else {
                (Value::Null, Value::Null)
            };
            let kde = match kde_tools() {
                Some((read, _)) => {
                    let mut values = Map::new();
                    for key in KDE_KEYS {
                        let value = run(read, &kde_args(key)).unwrap_or_default();
                        values.insert((*key).to_string(), json!(value.trim()));
                    }
                    Value::Object(values)
                }
                None => Value::Null,
            };
            if gnome.is_null() && kde.is_null() {
                return Err(
                    "NO_DESKTOP_PROXY_SETTINGS: neither gsettings nor kwriteconfig found"
                        .to_string(),
                );
            }
            Ok(json!({ "linux": {
                "gnome": gnome,
                "gnome_defaults": gnome_defaults,
                "kde": kde,
            } }))
        }

        fn apply(&self, host: &str, port: u16) -> Result<(), String> {
            let port_text = port.to_string();
            let mut applied = false;
            if has("gsettings") {
                for scheme in ["http", "https", "socks"] {
                    let schema = format!("org.gnome.system.proxy.{scheme}");
                    run("gsettings", &["set", &schema, "host", &format!("'{host}'")])?;
                    run("gsettings", &["set", &schema, "port", &port_text])?;
                }
                run(
                    "gsettings",
                    &[
                        "set",
                        "org.gnome.system.proxy",
                        "ignore-hosts",
                        &gnome::ignore_hosts(),
                    ],
                )?;
                run(
                    "gsettings",
                    &["set", "org.gnome.system.proxy", "mode", "'manual'"],
                )?;
                applied = true;
            }
            if let Some((_, write)) = kde_tools() {
                let set = |key: &str, value: &str| -> Result<(), String> {
                    let mut args: Vec<&str> = kde_args(key).to_vec();
                    args.push(value);
                    run(write, &args).map(|_| ())
                };
                set("httpProxy", &format!("http://{host} {port}"))?;
                set("httpsProxy", &format!("http://{host} {port}"))?;
                set("socksProxy", &format!("socks://{host} {port}"))?;
                set("NoProxyFor", &super::kde_no_proxy_for())?;
                set("ProxyType", "1")?;
                kde_reparse();
                applied = true;
            }
            if applied {
                Ok(())
            } else {
                Err("NO_DESKTOP_PROXY_SETTINGS: neither gsettings nor kwriteconfig found".into())
            }
        }

        fn restore(&self, saved: &Value) -> Result<(), String> {
            let Some(saved) = saved.get("linux") else {
                return Ok(());
            };
            let mut first_error = None;
            if let Some(values) = saved["gnome"].as_object() {
                let defaults = saved.get("gnome_defaults");
                if let Some(error) = gnome::restore(values, defaults, |args| run("gsettings", args))
                {
                    first_error.get_or_insert(error);
                }
            }
            if let (Some(kde), Some((_, write))) = (saved["kde"].as_object(), kde_tools()) {
                // ProxyType last, as on apply.
                let mut entries: Vec<(&String, &Value)> = kde.iter().collect();
                entries.sort_by_key(|(key, _)| key.as_str() == "ProxyType");
                for (key, value) in entries {
                    let value = value.as_str().unwrap_or_default();
                    let mut args: Vec<&str> = kde_args(key).to_vec();
                    if value.is_empty() {
                        args.push("--delete");
                    } else {
                        args.push(value);
                    }
                    if let Err(error) = run(write, &args) {
                        first_error.get_or_insert(error);
                    }
                }
                kde_reparse();
            }
            first_error.map_or(Ok(()), Err)
        }
    }
}

#[cfg(not(any(target_os = "macos", target_os = "linux", windows)))]
mod os {
    use super::SystemProxyWriter;
    use serde_json::Value;

    pub(crate) struct OsWriter;

    impl SystemProxyWriter for OsWriter {
        fn snapshot(&self) -> Result<Value, String> {
            Err("SYSTEM_PROXY_UNSUPPORTED_OS".into())
        }
        fn apply(&self, _: &str, _: u16) -> Result<(), String> {
            Err("SYSTEM_PROXY_UNSUPPORTED_OS".into())
        }
        fn restore(&self, _: &Value) -> Result<(), String> {
            Ok(())
        }
    }
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records calls instead of touching the OS.
    #[derive(Default)]
    pub(crate) struct FakeWriter {
        pub(crate) calls: Mutex<Vec<String>>,
        pub(crate) fail_apply: Mutex<Option<String>>,
        /// What `snapshot` reports as the previous settings.
        pub(crate) previous: Mutex<Option<Value>>,
        /// What the OS points at now: the last `apply` unless set here
        /// (another app took the proxy over).
        pub(crate) current: Mutex<Option<Vec<(String, u16)>>>,
        applied: Mutex<Option<(String, u16)>>,
    }

    impl FakeWriter {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl SystemProxyWriter for FakeWriter {
        fn snapshot(&self) -> Result<Value, String> {
            self.calls.lock().unwrap().push("snapshot".into());
            Ok(self
                .previous
                .lock()
                .unwrap()
                .clone()
                .unwrap_or_else(|| serde_json::json!({"fake": "previous"})))
        }
        fn apply(&self, host: &str, port: u16) -> Result<(), String> {
            self.calls
                .lock()
                .unwrap()
                .push(format!("apply {host}:{port}"));
            match self.fail_apply.lock().unwrap().clone() {
                Some(error) => Err(error),
                None => {
                    *self.applied.lock().unwrap() = Some((host.to_string(), port));
                    Ok(())
                }
            }
        }
        fn restore(&self, saved: &Value) -> Result<(), String> {
            self.calls.lock().unwrap().push(format!("restore {saved}"));
            Ok(())
        }
        fn current_proxies(&self) -> Result<Vec<(String, u16)>, String> {
            if let Some(current) = self.current.lock().unwrap().clone() {
                return Ok(current);
            }
            Ok(self.applied.lock().unwrap().clone().into_iter().collect())
        }
    }

    #[test]
    fn active_proxy_is_read_from_every_format() {
        use serde_json::json;
        let macos = json!({"macos": {
            "Wi-Fi": {"web": {"enabled": false, "server": "", "port": "0"},
                      "secure": {"enabled": true, "server": "127.0.0.1", "port": "6152"},
                      "socks": {"enabled": false, "server": "", "port": "0"}, "bypass": []}}});
        assert_eq!(active_proxy(&macos), Some(("127.0.0.1".into(), 6152)));
        let windows = json!({"windows": {"ProxyEnable": "0x1",
            "ProxyServer": "http=10.0.0.5:3128;https=10.0.0.5:3128", "ProxyOverride": null}});
        assert_eq!(active_proxy(&windows), Some(("10.0.0.5".into(), 3128)));
        let windows_plain =
            json!({"windows": {"ProxyEnable": "0x1", "ProxyServer": "127.0.0.1:7890"}});
        assert_eq!(
            active_proxy(&windows_plain),
            Some(("127.0.0.1".into(), 7890))
        );
        let windows_off =
            json!({"windows": {"ProxyEnable": "0x0", "ProxyServer": "127.0.0.1:7890"}});
        assert_eq!(active_proxy(&windows_off), None);
        let gnome = json!({"linux": {"gnome": {
            "org.gnome.system.proxy mode": "'manual'",
            "org.gnome.system.proxy.http host": "'127.0.0.1'",
            "org.gnome.system.proxy.http port": "7897"}, "kde": null}});
        assert_eq!(active_proxy(&gnome), Some(("127.0.0.1".into(), 7897)));
        let kde = json!({"linux": {"gnome": null, "kde": {"ProxyType": "1", "httpProxy": "http://127.0.0.1 7890"}}});
        assert_eq!(active_proxy(&kde), Some(("127.0.0.1".into(), 7890)));
        assert_eq!(active_proxy(&json!({"fake": "previous"})), None);
        assert!(active_proxies(&windows_off).is_empty());
        assert!(
            is_loopback_host("localhost")
                && is_loopback_host("127.0.0.1")
                && is_loopback_host("::1")
        );
        assert!(!is_loopback_host("10.0.0.5"));
    }

    #[test]
    fn every_active_proxy_is_listed_and_ours_is_recognised() {
        use serde_json::json;
        // Ours still on one service, another app's on the other.
        let macos = json!({"macos": {
            "Ethernet": {"web": {"enabled": true, "server": "127.0.0.1", "port": "6152"},
                         "secure": {"enabled": false, "server": "", "port": "0"},
                         "socks": {"enabled": false, "server": "", "port": "0"}, "bypass": []},
            "Wi-Fi": {"web": {"enabled": true, "server": "127.0.0.1", "port": "51234"},
                      "secure": {"enabled": true, "server": "127.0.0.1", "port": "51234"},
                      "socks": {"enabled": false, "server": "127.0.0.1", "port": "51234"},
                      "bypass": []}}});
        let proxies = active_proxies(&macos);
        assert_eq!(proxies.len(), 3);
        assert!(points_at(&proxies, "127.0.0.1", 51234));
        assert!(points_at(&proxies, "localhost", 6152), "any loopback name");
        assert!(!points_at(&proxies, "127.0.0.1", 7890));
        let windows = json!({"windows": {"ProxyEnable": "0x1",
            "ProxyServer": "http=127.0.0.1:6152;https=127.0.0.1:6152;socks=127.0.0.1:6153"}});
        assert!(points_at(&active_proxies(&windows), "127.0.0.1", 6153));
        let gnome = json!({"linux": {"gnome": {
            "org.gnome.system.proxy mode": "'manual'",
            "org.gnome.system.proxy.http host": "'127.0.0.1'",
            "org.gnome.system.proxy.http port": "7897",
            "org.gnome.system.proxy.socks host": "'127.0.0.1'",
            "org.gnome.system.proxy.socks port": "51234"},
            "kde": {"ProxyType": "1", "httpProxy": "http://127.0.0.1 7890"}}});
        let proxies = active_proxies(&gnome);
        assert_eq!(proxies.len(), 3);
        assert!(points_at(&proxies, "127.0.0.1", 51234));
        assert!(points_at(&proxies, "127.0.0.1", 7890), "KDE too");
        let gnome_off = json!({"linux": {"gnome": {
            "org.gnome.system.proxy mode": "'none'",
            "org.gnome.system.proxy.http host": "'127.0.0.1'",
            "org.gnome.system.proxy.http port": "51234"}, "kde": null}});
        assert!(!points_at(&active_proxies(&gnome_off), "127.0.0.1", 51234));
    }

    #[test]
    fn reg_query_values_keep_their_spacing() {
        let line = "    ProxyOverride    REG_SZ    localhost;  <local>  \r";
        assert_eq!(
            parse_reg_value_line(line, "ProxyOverride").as_deref(),
            Some("localhost;  <local>  ")
        );
        assert_eq!(
            parse_reg_value_line("    ProxyEnable    REG_DWORD    0x1", "ProxyEnable").as_deref(),
            Some("0x1")
        );
        assert_eq!(
            parse_reg_value_line("    ProxyServer    REG_SZ    ", "ProxyServer").as_deref(),
            Some("")
        );
        assert_eq!(
            parse_reg_value_line("    ProxyEnableX    REG_SZ    1", "ProxyEnable"),
            None
        );
        assert_eq!(
            parse_reg_value_line(r"HKEY_CURRENT_USER\Software\…", "ProxyEnable"),
            None
        );
    }

    /// Runs `steps` through a recording executor that fails the steps
    /// matching `fail`, and returns what it saw in a compact form.
    fn wininet_trace(
        fail: &[&str],
        body: impl FnOnce(&mut dyn FnMut(&wininet::Step) -> Result<(), String>) -> Result<(), String>,
    ) -> (Vec<String>, Result<(), String>) {
        let mut seen = Vec::new();
        let result = body(&mut |step| {
            let line = match step {
                wininet::Step::Set { name, kind, data } => format!("set {name} {kind} {data}"),
                wininet::Step::Delete(name) => format!("delete {name}"),
                wininet::Step::Notify => "notify".to_string(),
            };
            let failed = fail.iter().any(|prefix| line.starts_with(prefix));
            seen.push(line.clone());
            if failed {
                Err(format!("failed: {line}"))
            } else {
                Ok(())
            }
        });
        (seen, result)
    }

    #[test]
    fn wininet_apply_switches_off_before_rewriting() {
        let bypass = wininet::override_list();
        let (seen, result) = wininet_trace(&[], |exec| wininet::apply("127.0.0.1", 7891, exec));
        assert_eq!(result, Ok(()));
        assert_eq!(
            seen,
            vec![
                "set ProxyEnable REG_DWORD 0".to_string(),
                "set ProxyServer REG_SZ 127.0.0.1:7891".to_string(),
                format!("set ProxyOverride REG_SZ {bypass}"),
                "set ProxyEnable REG_DWORD 1".to_string(),
                "notify".to_string(),
            ]
        );
        assert!(bypass.starts_with("localhost;127.*;[::1];*.local;10.*;192.168.*;172.16.*;"));
        assert!(bypass.ends_with(";172.31.*;<local>"));
    }

    #[test]
    fn wininet_apply_stops_at_a_failure_and_still_notifies() {
        let (seen, result) = wininet_trace(&["set ProxyOverride"], |exec| {
            wininet::apply("127.0.0.1", 7891, exec)
        });
        assert!(result.unwrap_err().contains("ProxyOverride"));
        assert_eq!(seen.len(), 4);
        assert_eq!(seen[0], "set ProxyEnable REG_DWORD 0");
        assert!(seen[2].starts_with("set ProxyOverride"));
        assert_eq!(
            seen[3], "notify",
            "never switched on over a partial rewrite"
        );
    }

    #[test]
    fn wininet_restore_is_symmetric() {
        let saved = serde_json::json!({
            "ProxyEnable": "0x1",
            "ProxyServer": "corp:8080",
            "ProxyOverride": "*.corp;  <local>  ",
        });
        let (seen, result) = wininet_trace(&[], |exec| wininet::restore(&saved, exec));
        assert_eq!(result, Ok(()));
        assert_eq!(
            seen,
            vec![
                "set ProxyEnable REG_DWORD 0",
                "set ProxyServer REG_SZ corp:8080",
                "set ProxyOverride REG_SZ *.corp;  <local>  ",
                "set ProxyEnable REG_DWORD 1",
                "notify",
            ]
        );
    }

    #[test]
    fn wininet_restore_deletes_absent_values() {
        let saved = serde_json::json!({
            "ProxyEnable": null,
            "ProxyServer": null,
            "ProxyOverride": null,
        });
        let (seen, result) = wininet_trace(&[], |exec| wininet::restore(&saved, exec));
        assert_eq!(result, Ok(()));
        assert_eq!(
            seen,
            vec![
                "set ProxyEnable REG_DWORD 0",
                "delete ProxyServer",
                "delete ProxyOverride",
                "set ProxyEnable REG_DWORD 0",
                "notify",
            ]
        );
    }

    #[test]
    fn wininet_restore_keeps_the_proxy_off_over_a_mix() {
        let saved = serde_json::json!({
            "ProxyEnable": "0x1",
            "ProxyServer": "corp:8080",
            "ProxyOverride": "<local>",
        });
        let (seen, result) =
            wininet_trace(&["set ProxyServer"], |exec| wininet::restore(&saved, exec));
        assert!(result.unwrap_err().contains("ProxyServer"));
        assert_eq!(
            seen,
            vec![
                "set ProxyEnable REG_DWORD 0",
                "set ProxyServer REG_SZ corp:8080",
                "set ProxyOverride REG_SZ <local>",
                "notify",
            ]
        );
        // A failed switch-off does not stop the rest.
        let (seen, result) = wininet_trace(&["set ProxyEnable REG_DWORD 0"], |exec| {
            wininet::restore(&saved, exec)
        });
        assert!(result.is_err());
        assert_eq!(seen.len(), 5);
        assert_eq!(seen[3], "set ProxyEnable REG_DWORD 1");
    }

    #[test]
    fn kde_bypasses_the_private_ranges_too() {
        assert_eq!(
            kde_no_proxy_for(),
            "localhost,127.0.0.1,::1,*.local,10.0.0.0/8,172.16.0.0/12,192.168.0.0/16"
        );
    }

    /// A fake dconf behind `gsettings`: user values over schema defaults.
    struct FakeGsettings {
        defaults: std::collections::BTreeMap<String, String>,
        user: std::collections::BTreeMap<String, String>,
        calls: Vec<String>,
        /// Keys whose reset lands on something else (a site default).
        site: std::collections::BTreeMap<String, String>,
    }

    impl FakeGsettings {
        fn new() -> Self {
            let defaults = [
                ("org.gnome.system.proxy mode", "'none'"),
                (
                    "org.gnome.system.proxy ignore-hosts",
                    "['localhost', '127.0.0.0/8', '::1']",
                ),
                ("org.gnome.system.proxy.http host", "''"),
                ("org.gnome.system.proxy.http port", "8080"),
                ("org.gnome.system.proxy.https host", "''"),
                ("org.gnome.system.proxy.https port", "0"),
                ("org.gnome.system.proxy.socks host", "''"),
                ("org.gnome.system.proxy.socks port", "0"),
            ];
            FakeGsettings {
                defaults: defaults
                    .iter()
                    .map(|(key, value)| (key.to_string(), value.to_string()))
                    .collect(),
                user: Default::default(),
                calls: Vec::new(),
                site: Default::default(),
            }
        }

        fn get(&self, key: &str) -> String {
            self.user
                .get(key)
                .or_else(|| self.site.get(key))
                .or_else(|| self.defaults.get(key))
                .cloned()
                .unwrap()
        }

        fn run(&mut self, args: &[&str]) -> Result<String, String> {
            let key = format!("{} {}", args[1], args[2]);
            match args[0] {
                "get" => return Ok(format!("{}\n", self.get(&key))),
                "set" => {
                    self.user.insert(key, args[3].to_string());
                }
                "reset" => {
                    self.user.remove(&key);
                }
                other => return Err(format!("unexpected {other}")),
            }
            self.calls.push(args.join(" "));
            Ok(String::new())
        }

        fn snapshot(&mut self) -> Value {
            let defaults = self.defaults.clone();
            let (values, unset) = gnome::snapshot(
                |schema, key| self.run(&["get", schema, key]),
                |schema, key| defaults.get(&format!("{schema} {key}")).cloned(),
            )
            .unwrap();
            serde_json::json!({ "gnome": values, "gnome_defaults": unset })
        }

        fn apply(&mut self) {
            for scheme in ["http", "https", "socks"] {
                let schema = format!("org.gnome.system.proxy.{scheme}");
                self.run(&["set", &schema, "host", "'127.0.0.1'"]).unwrap();
                self.run(&["set", &schema, "port", "7891"]).unwrap();
            }
            let hosts = gnome::ignore_hosts();
            self.run(&["set", "org.gnome.system.proxy", "ignore-hosts", &hosts])
                .unwrap();
            self.run(&["set", "org.gnome.system.proxy", "mode", "'manual'"])
                .unwrap();
            self.calls.clear();
        }

        fn restore(&mut self, saved: &Value) -> Option<String> {
            let values = saved["gnome"].as_object().unwrap().clone();
            gnome::restore(&values, saved.get("gnome_defaults"), |args| self.run(args))
        }
    }

    #[test]
    fn gnome_restore_resets_what_was_default() {
        let mut fake = FakeGsettings::new();
        fake.user
            .insert("org.gnome.system.proxy mode".into(), "'auto'".into());
        fake.user
            .insert("org.gnome.system.proxy.http host".into(), "'orig'".into());
        // A user value equal to the default is not kept either.
        fake.user
            .insert("org.gnome.system.proxy.https port".into(), "0".into());
        let saved = fake.snapshot();
        assert_eq!(
            saved["gnome_defaults"],
            serde_json::json!([
                "org.gnome.system.proxy ignore-hosts",
                "org.gnome.system.proxy.http port",
                "org.gnome.system.proxy.https host",
                "org.gnome.system.proxy.https port",
                "org.gnome.system.proxy.socks host",
                "org.gnome.system.proxy.socks port",
            ])
        );

        fake.apply();
        assert_eq!(fake.restore(&saved), None);
        assert_eq!(
            fake.calls,
            vec![
                "reset org.gnome.system.proxy ignore-hosts",
                "set org.gnome.system.proxy.http host 'orig'",
                "reset org.gnome.system.proxy.http port",
                "reset org.gnome.system.proxy.https host",
                "reset org.gnome.system.proxy.https port",
                "reset org.gnome.system.proxy.socks host",
                "reset org.gnome.system.proxy.socks port",
                "set org.gnome.system.proxy mode 'auto'",
            ]
        );
        let user: Vec<&str> = fake.user.keys().map(String::as_str).collect();
        assert_eq!(
            user,
            vec![
                "org.gnome.system.proxy mode",
                "org.gnome.system.proxy.http host"
            ]
        );
    }

    #[test]
    fn gnome_restore_writes_the_value_when_a_reset_lands_elsewhere() {
        let mut fake = FakeGsettings::new();
        let saved = fake.snapshot();
        // Reset now yields a site default instead of the saved value.
        fake.site
            .insert("org.gnome.system.proxy.socks port".into(), "1080".into());
        fake.apply();
        assert_eq!(fake.restore(&saved), None);
        assert_eq!(
            fake.calls.last().map(String::as_str),
            Some("reset org.gnome.system.proxy mode")
        );
        assert!(fake
            .calls
            .contains(&"set org.gnome.system.proxy.socks port 0".to_string()));
        assert_eq!(fake.get("org.gnome.system.proxy.socks port"), "0");
    }

    #[test]
    fn gnome_restore_of_an_earlier_backup_writes_every_key() {
        let mut fake = FakeGsettings::new();
        fake.user
            .insert("org.gnome.system.proxy mode".into(), "'auto'".into());
        let mut saved = fake.snapshot();
        saved.as_object_mut().unwrap().remove("gnome_defaults");
        fake.apply();
        assert_eq!(fake.restore(&saved), None);
        assert_eq!(fake.calls.len(), 8);
        assert!(fake.calls.iter().all(|call| call.starts_with("set ")));
        assert_eq!(
            fake.calls.last().map(String::as_str),
            Some("set org.gnome.system.proxy mode 'auto'")
        );
    }

    #[test]
    fn gnome_restore_continues_past_failures() {
        let saved = serde_json::json!({
            "org.gnome.system.proxy mode": "'none'",
            "org.gnome.system.proxy.http host": "'orig'",
            "org.gnome.system.proxy.http port": "",
        });
        let defaults = serde_json::json!(["org.gnome.system.proxy mode"]);
        let mut calls = Vec::new();
        let error = gnome::restore(saved.as_object().unwrap(), Some(&defaults), |args| {
            calls.push(args.join(" "));
            Err(format!("boom {}", args[0]))
        });
        assert_eq!(error.as_deref(), Some("boom set"));
        assert_eq!(
            calls,
            vec![
                "set org.gnome.system.proxy.http host 'orig'",
                "reset org.gnome.system.proxy mode",
                "set org.gnome.system.proxy mode 'none'",
            ]
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_listings_parse() {
        let services = os::services(
            "An asterisk (*) denotes that a network service is disabled.\nWi-Fi\n*Bluetooth PAN\nUSB 10/100/1000 LAN\n",
        );
        assert_eq!(services, vec!["Wi-Fi", "USB 10/100/1000 LAN"]);
        let proxy = os::parse_proxy(
            "Enabled: Yes\nServer: 10.0.0.2\nPort: 3128\nAuthenticated Proxy Enabled: 0\n",
        );
        assert_eq!(
            proxy,
            serde_json::json!({"enabled": true, "server": "10.0.0.2", "port": "3128"})
        );
        assert!(os::parse_bypass("There aren't any bypass domains set on Wi-Fi.\n").is_empty());
        assert_eq!(
            os::parse_bypass("*.local\n169.254/16\n"),
            vec!["*.local", "169.254/16"]
        );
    }
}
