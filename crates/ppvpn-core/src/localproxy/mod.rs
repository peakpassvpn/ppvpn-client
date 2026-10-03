//! The shared local proxy's device state (docs/host-integration.md, 4.6;
//! Go: localproxy). One loopback port and one password serve every node:
//! the username `<prefix>-<node id>` pins a node, the bare `<prefix>` is the
//! routed user. The prefix, the password and the last bound ports live in
//! `state_dir`, so they survive restarts and upgrades; the profile never
//! does.
//!
//! Read or created once by `Engine::new` ([`LocalProxyState::open`]); the
//! instance holds the `state_dir` lock, so memory is authoritative after
//! that and every change is written through.
//!
//! Where Rust differs from Go 0.5.21 (docs/rust-parity.md):
//! - A state file that cannot be used (not JSON, unknown version, invalid
//!   prefix, prefix without password) is rebuilt, not refused: a refusal
//!   would fail `Engine::new` until someone deletes the file by hand.
//! - A state file other users may read (Unix) keeps its ports but gets a
//!   new prefix and password, written 0600: its secret is no longer one.
//! - Either rebuild is reported ([`LocalProxyState::credentials_reset`]):
//!   the Engine shows it in `status.local_proxy.credentials_reset`.
//! - The routed user exists without a profile: its credential is readable
//!   right after `new` (contract, section 3).

#![allow(dead_code)] // the Engine is wired to it with the runtime

use std::fs;
use std::io::{self, Write as _};
use std::net::{IpAddr, SocketAddr, TcpListener};
use std::path::{Path, PathBuf};

use base64::Engine as _;
use chrono::Utc;
use serde::{Deserialize, Serialize};

use crate::config::LocalProxyConfig;
use crate::error::{codes, Error};
use crate::event::Event;
use crate::profile::Profile;
use crate::status::{CredentialsResetReason, LocalProxyStatus};
use crate::translate;
use crate::types::{LocalProxyCredential, LocalProxyKind, LocalProxyMetadata};

/// The state file in `state_dir`; the name Go's `serve` used.
pub(crate) const STATE_FILE: &str = "local-proxies.json";
/// Version 2 replaced per-node ports and credentials (version 1) with one
/// shared port, username prefix and password.
pub(crate) const STATE_VERSION: u32 = 2;
/// Tried after the persisted and the configured port (section 4.6).
pub(crate) const PREFERRED_PORT: u16 = 7890;
/// The first choice of the system proxy listener.
pub(crate) const SYSTEM_PROXY_PREFERRED_PORT: u16 = 7891;
/// What both listeners speak (Go: Metadata.Protocols).
pub(crate) const PROTOCOLS: [&str; 2] = ["http", "socks5"];

const PREFIX_LENGTH: usize = 5;
const PREFIX_ALPHABET: &[u8] = b"abcdefghijklmnopqrstuvwxyz0123456789";
const PASSWORD_BYTES: usize = 32;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
struct DiskState {
    version: u32,
    #[serde(default)]
    prefix: String,
    #[serde(default)]
    password: String,
    #[serde(default, skip_serializing_if = "is_zero")]
    port: u16,
    /// The last port of the system proxy listener. Whether it is enabled is
    /// deliberately not persisted: it starts disabled.
    #[serde(default, skip_serializing_if = "is_zero")]
    system_proxy_port: u16,
}

/// Without prefix and password: a state may end up in a log or a panic.
impl std::fmt::Debug for DiskState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("DiskState")
            .field("version", &self.version)
            .field("port", &self.port)
            .field("system_proxy_port", &self.system_proxy_port)
            .finish_non_exhaustive()
    }
}

fn is_zero(port: &u16) -> bool {
    *port == 0
}

/// How a state file was read.
#[derive(Debug, PartialEq, Eq)]
enum Loaded {
    Missing,
    Current(DiskState),
    /// Version 1: its per-node values are dropped and regenerated.
    Legacy,
    /// Unusable: the reason, for the log.
    Corrupt(String),
}

/// The device's local proxy state. Not synchronised: the Engine keeps it
/// behind its own lock.
#[derive(Debug)]
pub(crate) struct LocalProxyState {
    path: PathBuf,
    listen: IpAddr,
    preferred_port: u16,
    system_preferred_port: u16,
    state: DiskState,
    /// Why `open` replaced the prefix and password, if it did.
    reset: Option<CredentialsResetReason>,
}

impl LocalProxyState {
    /// Reads `state_dir`'s state or creates it, and picks the port for a
    /// listener that is not open yet: the persisted port when it is free,
    /// else the configured one, else 7890, else any free port
    /// (`preferred_port` 0: the persisted one, else any). Persists what
    /// changed.
    pub(crate) fn open(state_dir: &Path, config: &LocalProxyConfig) -> Result<Self, Error> {
        Self::open_with(state_dir, config, SYSTEM_PROXY_PREFERRED_PORT)
    }

    /// [`open`](Self::open) with another first choice for the system proxy
    /// port (0: any free port); for tests.
    pub(crate) fn open_with(
        state_dir: &Path,
        config: &LocalProxyConfig,
        system_preferred_port: u16,
    ) -> Result<Self, Error> {
        let listen: IpAddr = config.listen.parse().map_err(|_| {
            Error::invalid(
                codes::CORE_OPERATION_FAILED,
                "local_proxy.listen",
                format!(
                    "local proxy: listen {:?} is not an IP address",
                    config.listen
                ),
            )
        })?;
        let path = state_dir.join(STATE_FILE);
        prepare_directory(state_dir).map_err(|e| io_error("create the state directory", &e))?;
        let mut changed = false;
        let mut reset = None;
        let loaded = load(&path).map_err(|e| io_error("read the state", &e))?;
        let existed = !matches!(loaded, Loaded::Missing);
        let mut state = match loaded {
            Loaded::Missing => DiskState::default(),
            Loaded::Current(state) => state,
            Loaded::Legacy => {
                changed = true;
                DiskState::default()
            }
            Loaded::Corrupt(reason) => {
                tracing::warn!(reason = %reason, "local proxy: state unusable, rebuilt");
                reset = Some(CredentialsResetReason::Corrupt);
                changed = true;
                DiskState::default()
            }
        };
        if state.version != STATE_VERSION {
            state.version = STATE_VERSION;
            changed = true;
        }
        if existed && !private(&path) {
            tracing::warn!("local proxy: state readable by other users, new credentials");
            reset.get_or_insert(CredentialsResetReason::InsecurePermissions);
            state.prefix.clear();
            state.password.clear();
        }
        if state.prefix.is_empty() {
            state.prefix = random_prefix()?;
            state.password = random_password()?;
            changed = true;
        }
        let mut this = Self {
            path,
            listen,
            preferred_port: config.preferred_port,
            system_preferred_port,
            state,
            reset,
        };
        let port = this.choose_local_port()?;
        if port != this.state.port {
            this.state.port = port;
            changed = true;
        }
        if changed {
            this.save()?;
        }
        Ok(this)
    }

    /// Why `open` replaced the prefix and password (the file is rewritten
    /// 0600 either way); None when it kept them or created the first ones.
    pub(crate) fn credentials_reset(&self) -> Option<CredentialsResetReason> {
        self.reset
    }

    pub(crate) fn listen(&self) -> String {
        self.listen.to_string()
    }

    /// The shared port, chosen by `open` or the last `reconcile_port`.
    pub(crate) fn port(&self) -> u16 {
        self.state.port
    }

    /// What the translation needs for the `mixed` inbound and its users.
    pub(crate) fn translate_options(&self) -> translate::LocalProxy {
        translate::LocalProxy {
            listen: self.listen(),
            port: self.state.port,
            prefix: self.state.prefix.clone(),
            password: self.state.password.clone(),
        }
    }

    pub(crate) fn status(&self, listening: bool) -> LocalProxyStatus {
        LocalProxyStatus {
            listen: self.listen(),
            port: self.state.port,
            listening,
            credentials_reset: self.reset,
        }
    }

    /// Before the listener opens again (`start`, a retry after
    /// `LocalProxyUnavailable`): keeps the port when it is still free,
    /// otherwise picks one as `open` does and persists it. Returns the
    /// `LocalProxyEndpointChanged` to emit when the port moved. Never call
    /// it while the listener is open: its own port reads as taken.
    pub(crate) fn reconcile_port(&mut self) -> Result<Option<Event>, Error> {
        let port = self.choose_local_port()?;
        if port == self.state.port {
            return Ok(None);
        }
        self.state.port = port;
        self.save()?;
        Ok(Some(Event::LocalProxyEndpointChanged {
            at: Utc::now(),
            listen: self.listen(),
            port,
        }))
    }

    /// One entry per node of `profile`, by node id, and the routed user
    /// last (hosts that read the first entry keep getting a node). No
    /// secrets: what a WebView may see.
    pub(crate) fn metadata(&self, profile: Option<&Profile>) -> Vec<LocalProxyMetadata> {
        let entry = |kind, node_id: &str| LocalProxyMetadata {
            kind,
            node_id: node_id.to_owned(),
            listen: self.listen(),
            port: self.state.port,
            protocols: PROTOCOLS.iter().map(|p| (*p).to_owned()).collect(),
            auth_required: true,
        };
        let mut out: Vec<_> = node_ids(profile)
            .into_iter()
            .map(|id| entry(LocalProxyKind::Node, id))
            .collect();
        out.push(entry(LocalProxyKind::Routed, ""));
        out
    }

    /// The credential that pins `node_id`; `NODE_NOT_FOUND` when `profile`
    /// (none before the first apply) has no such node.
    pub(crate) fn credential(
        &self,
        profile: Option<&Profile>,
        node_id: &str,
    ) -> Result<LocalProxyCredential, Error> {
        if node_id.is_empty() || !node_ids(profile).contains(&node_id) {
            return Err(Error::invalid(
                codes::NODE_NOT_FOUND,
                "node_id",
                format!("node {node_id:?} is not in the active profile"),
            ));
        }
        Ok(self.credential_for(LocalProxyKind::Node, node_id))
    }

    /// The routed user's credential: routed like the system proxy, by the
    /// profile rules and then the selected node.
    pub(crate) fn routed_credential(&self) -> LocalProxyCredential {
        self.credential_for(LocalProxyKind::Routed, "")
    }

    fn credential_for(&self, kind: LocalProxyKind, node_id: &str) -> LocalProxyCredential {
        LocalProxyCredential {
            kind,
            node_id: node_id.to_owned(),
            listen: self.listen(),
            port: self.state.port,
            username: format_username(&self.state.prefix, node_id),
            password: self.state.password.clone(),
        }
    }

    /// The port of the system proxy listener: the persisted one when it is
    /// free, else 7891, else any free port, never the shared port. With
    /// `probe` false (the listener already runs on it) the persisted port is
    /// kept unchecked. Persists the choice.
    pub(crate) fn system_proxy_port(&mut self, probe: bool) -> Result<u16, Error> {
        let avoid = self.state.port;
        let mut port = self.state.system_proxy_port;
        if port == 0 || port == avoid || probe {
            let persisted = if port == avoid { 0 } else { port };
            let preferred = if self.system_preferred_port == avoid {
                0
            } else {
                self.system_preferred_port
            };
            port = choose_port(self.listen, &[persisted, preferred])?;
            if port == avoid {
                // Only when the shared port is not bound yet.
                port = choose_port(self.listen, &[])?;
            }
        }
        if port != self.state.system_proxy_port {
            self.state.system_proxy_port = port;
            self.save()?;
        }
        Ok(port)
    }

    fn choose_local_port(&self) -> Result<u16, Error> {
        let mut candidates = vec![self.state.port];
        if self.preferred_port != 0 {
            candidates.extend([self.preferred_port, PREFERRED_PORT]);
        }
        // Never the system proxy listener's port.
        let system = self.state.system_proxy_port;
        candidates.retain(|port| *port != system);
        let mut port = choose_port(self.listen, &candidates)?;
        if port == system {
            port = choose_port(self.listen, &[])?;
        }
        Ok(port)
    }

    /// Writes the state atomically: a private temporary file renamed over
    /// the old one.
    fn save(&self) -> Result<(), Error> {
        let data = serde_json::to_vec(&self.state).expect("state serialises");
        write_private(&self.path, &data).map_err(|e| io_error("write the state", &e))
    }
}

fn node_ids(profile: Option<&Profile>) -> Vec<&str> {
    let mut ids: Vec<&str> = profile
        .map(|p| p.nodes.iter().map(|n| n.id.as_str()).collect())
        .unwrap_or_default();
    ids.sort_unstable();
    ids
}

/// The username that pins `node_id`, or the routed user's (the bare
/// prefix) for an empty one.
pub(crate) fn format_username(prefix: &str, node_id: &str) -> String {
    if node_id.is_empty() {
        prefix.to_owned()
    } else {
        format!("{prefix}-{node_id}")
    }
}

/// Splits a username into prefix and node id. The prefix never contains
/// '-', so the node id is everything after the first one and may contain
/// more. A bare prefix is the routed user (empty node id); node ids are
/// never empty, so the two cannot collide.
pub(crate) fn parse_username(username: &str) -> Option<(&str, &str)> {
    let (prefix, node_id) = match username.split_once('-') {
        Some((_, "")) => return None,
        Some(split) => split,
        None => (username, ""),
    };
    valid_prefix(prefix).then_some((prefix, node_id))
}

fn valid_prefix(prefix: &str) -> bool {
    prefix.len() == PREFIX_LENGTH && prefix.bytes().all(|b| PREFIX_ALPHABET.contains(&b))
}

fn load(path: &Path) -> io::Result<Loaded> {
    let data = match fs::read(path) {
        Ok(data) => data,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok(Loaded::Missing),
        Err(e) => return Err(e),
    };
    Ok(decode(&data))
}

fn decode(data: &[u8]) -> Loaded {
    #[derive(Deserialize)]
    struct Header {
        #[serde(default)]
        version: u32,
    }
    #[derive(Deserialize)]
    struct Legacy {
        endpoints: Option<serde_json::Map<String, serde_json::Value>>,
    }
    let corrupt = |reason: &str| Loaded::Corrupt(reason.to_owned());
    let header: Header = match serde_json::from_slice(data) {
        Ok(header) => header,
        Err(e) => return decode_error(&e),
    };
    match header.version {
        1 => match serde_json::from_slice::<Legacy>(data) {
            Ok(Legacy { endpoints: Some(_) }) => Loaded::Legacy,
            _ => corrupt("version 1 without endpoints"),
        },
        STATE_VERSION => match serde_json::from_slice::<DiskState>(data) {
            Err(e) => decode_error(&e),
            Ok(state) if !state.prefix.is_empty() && !valid_prefix(&state.prefix) => {
                corrupt("invalid prefix")
            }
            Ok(state) if state.prefix.is_empty() != state.password.is_empty() => {
                corrupt("prefix and password must come together")
            }
            Ok(state) => Loaded::Current(state),
        },
        version => Loaded::Corrupt(format!("unsupported version {version}")),
    }
}

/// Where the file did not decode, without what serde quotes of its values
/// (a password among them): the reason is logged.
fn decode_error(e: &serde_json::Error) -> Loaded {
    Loaded::Corrupt(format!(
        "decode: {:?} error at line {} column {}",
        e.classify(),
        e.line(),
        e.column()
    ))
}

/// Creates `dir` and, on Unix, makes it 0700. On Windows the host's
/// app-private directory carries the ACL.
fn prepare_directory(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(dir, fs::Permissions::from_mode(0o700))?;
    }
    Ok(())
}

/// Whether nobody but the owner can read `path` (missing counts as
/// private). Unix mode bits only.
fn private(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(path) {
            Ok(meta) => meta.permissions().mode() & 0o077 == 0,
            Err(_) => true,
        }
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

fn write_private(path: &Path, data: &[u8]) -> io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    let (tmp, mut file) = create_temp(dir)?;
    let written = file.write_all(data).and_then(|()| file.sync_all());
    drop(file);
    let result = written.and_then(|()| fs::rename(&tmp, path));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result
}

fn create_temp(dir: &Path) -> io::Result<(PathBuf, fs::File)> {
    for attempt in 0..16u32 {
        let name = dir.join(format!(
            ".local-proxies-{}-{attempt}.tmp",
            std::process::id()
        ));
        let mut options = fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        match options.open(&name) {
            Ok(file) => return Ok((name, file)),
            Err(e) if e.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e),
        }
    }
    Err(io::Error::other("cannot create a temporary file"))
}

/// The first free port among `candidates` (zeros skipped), else one the
/// kernel assigns.
fn choose_port(listen: IpAddr, candidates: &[u16]) -> Result<u16, Error> {
    for &port in candidates {
        if port != 0 && port_free(listen, port) {
            return Ok(port);
        }
    }
    TcpListener::bind(SocketAddr::new(listen, 0))
        .and_then(|l| l.local_addr())
        .map(|a| a.port())
        .map_err(|e| {
            Error::new(
                codes::CORE_OPERATION_FAILED,
                true,
                format!("local proxy: no free port on {listen}: {e}"),
            )
        })
}

fn port_free(listen: IpAddr, port: u16) -> bool {
    TcpListener::bind(SocketAddr::new(listen, port)).is_ok()
}

fn random_bytes(buf: &mut [u8]) -> Result<(), Error> {
    getrandom::fill(buf).map_err(|e| {
        Error::new(
            codes::CORE_OPERATION_FAILED,
            true,
            format!("local proxy: random source: {e}"),
        )
    })
}

/// Five characters of [`PREFIX_ALPHABET`]; rejection sampling keeps each
/// equally likely.
fn random_prefix() -> Result<String, Error> {
    let limit = (256 - 256 % PREFIX_ALPHABET.len()) as u16;
    let mut out = String::with_capacity(PREFIX_LENGTH);
    let mut buf = [0u8; 16];
    while out.len() < PREFIX_LENGTH {
        random_bytes(&mut buf)?;
        for &b in &buf {
            if u16::from(b) < limit && out.len() < PREFIX_LENGTH {
                out.push(char::from(
                    PREFIX_ALPHABET[usize::from(b) % PREFIX_ALPHABET.len()],
                ));
            }
        }
    }
    Ok(out)
}

/// 32 random bytes, base64url without padding (43 characters).
fn random_password() -> Result<String, Error> {
    let mut buf = [0u8; PASSWORD_BYTES];
    random_bytes(&mut buf)?;
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(buf))
}

fn io_error(what: &str, e: &io::Error) -> Error {
    let code = if e.kind() == io::ErrorKind::PermissionDenied {
        codes::PERMISSION_DENIED
    } else {
        codes::CORE_OPERATION_FAILED
    };
    Error::new(code, false, format!("local proxy: {what}: {e}"))
}

#[cfg(test)]
mod tests;
