//! The profile's binary rule sets (Go: internal/rulesets): downloaded from
//! the profile's pinned host, verified against its sha256, cached in the
//! state directory (so a restart needs no network) and refreshed in the
//! background. A set that cannot be had only degrades routing: its rules
//! skip it, and an apply never fails for it.
//!
//! An apply [`Manager::prepare`]s a [`Snapshot`] (fetching what is missing
//! within a deadline), translates with [`Snapshot::files`] and then
//! [`Manager::activate`]s it. Activation reports the state changes and
//! starts the refresh task, which reports later ones and asks for a rebuild
//! when a set appears, disappears or changes how it must be rendered.

mod fetch;
mod srs;
#[cfg(test)]
mod tests;

pub(crate) use fetch::{Dial, DirectDial, Stream};

use std::collections::{HashMap, HashSet};
use std::io;
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use chrono::{DateTime, Utc};
use futures_util::stream::{self, StreamExt};
use sha2::{Digest, Sha256};
use tokio::task::JoinHandle;
use tokio::time::Instant;

use crate::event::Event;
use crate::profile::{normalize_rule_set_host, rule_set_host, RuleSet};
use crate::status::RuleSetStatus;
use crate::translate::RuleSetFile;

// Error codes reported in a status's `error` and an event's `code`.
pub(crate) const HOST_NOT_PINNED: &str = "RULE_SET_HOST_NOT_PINNED";
pub(crate) const DOWNLOAD_FAILED: &str = "RULE_SET_DOWNLOAD_FAILED";
pub(crate) const HTTP_STATUS: &str = "RULE_SET_HTTP_STATUS";
pub(crate) const TOO_LARGE: &str = "RULE_SET_TOO_LARGE";
pub(crate) const SHA256_MISMATCH: &str = "RULE_SET_SHA256_MISMATCH";
pub(crate) const INVALID: &str = "RULE_SET_INVALID";
pub(crate) const STORAGE_FAILED: &str = "RULE_SET_STORAGE_FAILED";
pub(crate) const STORAGE_UNAVAILABLE: &str = crate::error::codes::RULE_SET_STORAGE_UNAVAILABLE;

/// The most one downloaded rule set may be.
pub(crate) const MAX_SIZE: usize = 32 << 20;

/// How long an apply waits for its downloads, all of them together.
pub(crate) const PREPARE_TIMEOUT: Duration = Duration::from_secs(10);

/// The parallel downloads of one refresh round.
const REFRESH_CONCURRENCY: usize = 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum State {
    /// The local copy matches the profile's sha256 and is in use.
    Ready,
    /// The profile's version could not be fetched; an earlier verified copy
    /// (another sha256) is in use.
    Stale,
    /// No copy exists; rules naming the set skip it.
    Unavailable,
}

impl State {
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            State::Ready => "ready",
            State::Stale => "stale",
            State::Unavailable => "unavailable",
        }
    }
}

/// One rule set's state. `failures` and `next_retry_at` are Go's get-status
/// fields the public [`RuleSetStatus`] does not carry (yet); the log has
/// them.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Status {
    pub id: String,
    pub state: State,
    pub updated_at: Option<DateTime<Utc>>,
    /// The error code, empty when ready.
    pub error: &'static str,
    /// Consecutive failed downloads (0 when ready).
    pub failures: u32,
    /// When a set that is not ready is retried; none when ready, or when
    /// nothing can change before the next apply (host not pinned, no
    /// storage).
    pub next_retry_at: Option<DateTime<Utc>>,
}

impl Status {
    /// The status as `Engine::status` reports it.
    pub(crate) fn public(&self) -> RuleSetStatus {
        RuleSetStatus {
            id: self.id.clone(),
            state: self.state.as_str().into(),
            updated_at: self.updated_at,
            error: self.error.into(),
        }
    }

    /// The `RuleSetChanged` event of a state change.
    pub(crate) fn event(&self, at: DateTime<Utc>) -> Event {
        Event::RuleSetChanged {
            at,
            rule_set_id: self.id.clone(),
            message: self.state.as_str().into(),
            code: self.error.into(),
        }
    }
}

type StateHook = Arc<dyn Fn(Status) + Send + Sync>;
type RebuildHook = Arc<dyn Fn() + Send + Sync>;
type Clock = Arc<dyn Fn() -> DateTime<Utc> + Send + Sync>;

#[derive(Clone)]
pub(crate) struct Options {
    /// Holds `<id>.srs` (normally `<state_dir>/rule-sets`). None disables
    /// downloads and caching: every set is unavailable.
    pub dir: Option<PathBuf>,
    /// Opens the connections downloads use; they must not enter the tunnel.
    pub dial: Arc<dyn Dial>,
    /// PEM certificates trusted instead of the system's roots (tests).
    pub trust_pem: Option<String>,
    /// Called on every state change, without locks held.
    pub on_state: Option<StateHook>,
    /// Called from the refresh task when a refresh changed which sets are
    /// available or how they must be rendered, once per round: the
    /// configuration must be rebuilt (with [`Manager::prepare`] without
    /// downloads). It must not block; the rebuild goes on its own task.
    /// A set that only changed content needs none: the file is replaced in
    /// place.
    pub on_rebuild: Option<RebuildHook>,
    /// The retry backoff of sets that are not ready: `retry_min`, doubled
    /// per consecutive failure up to `retry_max`, never above the set's
    /// update interval.
    pub retry_min: Duration,
    pub retry_max: Duration,
    /// Bounds one background download.
    pub fetch_timeout: Duration,
    pub now: Clock,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            dir: None,
            dial: Arc::new(DirectDial),
            trust_pem: None,
            on_state: None,
            on_rebuild: None,
            retry_min: Duration::from_secs(5),
            retry_max: Duration::from_secs(15 * 60),
            fetch_timeout: Duration::from_secs(60),
            now: Arc::new(Utc::now),
        }
    }
}

/// The rule sets of the active profile and their refresh task. Dropping it
/// stops the task.
pub(crate) struct Manager {
    inner: Arc<Inner>,
}

struct Inner {
    opts: Options,
    client: fetch::Client,
    shared: Mutex<Shared>,
}

#[derive(Default)]
struct Shared {
    generation: u64,
    current: Option<Snapshot>,
    task: Option<JoinHandle<()>>,
}

/// The state of one profile's rule sets. [`Manager::prepare`] returns one;
/// [`Manager::activate`] makes it current.
#[derive(Debug, Clone, Default)]
pub(crate) struct Snapshot {
    pinned: HashSet<String>,
    entries: Vec<Entry>,
}

#[derive(Debug, Clone)]
struct Entry {
    set: RuleSet,
    state: State,
    updated_at: Option<DateTime<Utc>>,
    err: &'static str,
    local: Option<LocalCopy>,
    failures: u32,
    due: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct LocalCopy {
    path: PathBuf,
    sha256: String,
    mirror_dns: bool,
}

impl Snapshot {
    /// The verified local copy of every set that has one: the translation's
    /// `Options::rule_sets`.
    pub(crate) fn files(&self) -> HashMap<String, RuleSetFile> {
        self.entries
            .iter()
            .filter(|e| e.state != State::Unavailable)
            .filter_map(|e| {
                let local = e.local.as_ref()?;
                Some((
                    e.set.id.clone(),
                    RuleSetFile {
                        path: local.path.to_string_lossy().into_owned(),
                        mirror_dns: local.mirror_dns,
                    },
                ))
            })
            .collect()
    }

    /// The sets by state: (ready, stale, unavailable), for the apply log.
    pub(crate) fn counts(&self) -> (usize, usize, usize) {
        let count = |state| self.entries.iter().filter(|e| e.state == state).count();
        (
            count(State::Ready),
            count(State::Stale),
            count(State::Unavailable),
        )
    }

    fn host_pinned(&self, set: &RuleSet) -> bool {
        host_pinned(&self.pinned, set)
    }
}

fn host_pinned(pinned: &HashSet<String>, set: &RuleSet) -> bool {
    rule_set_host(&set.url).is_ok_and(|host| pinned.contains(&host))
}

impl Entry {
    fn new(set: RuleSet) -> Self {
        Entry {
            set,
            state: State::Unavailable,
            updated_at: None,
            err: "",
            local: None,
            failures: 0,
            due: None,
        }
    }

    /// Nothing can change before the next apply.
    fn stuck(&self) -> bool {
        self.err == HOST_NOT_PINNED || self.err == STORAGE_UNAVAILABLE
    }

    fn status(&self) -> Status {
        let mut status = Status {
            id: self.set.id.clone(),
            state: self.state,
            updated_at: self.updated_at,
            error: "",
            failures: 0,
            next_retry_at: None,
        };
        if self.state != State::Ready {
            status.error = self.err;
            status.failures = self.failures;
            if !self.stuck() {
                status.next_retry_at = self.due;
            }
        }
        status
    }
}

impl Manager {
    pub(crate) fn new(opts: Options) -> Self {
        let client = fetch::Client::new(opts.dial.clone(), opts.trust_pem.as_deref());
        Manager {
            inner: Arc::new(Inner {
                opts,
                client,
                shared: Mutex::default(),
            }),
        }
    }

    /// Resolves every set to a verified local copy. A cached copy matching
    /// the profile's sha256 is used at once. Otherwise, with `download` set
    /// and the URL's host pinned, the set is fetched within that time (the
    /// downloads run together). A failed fetch falls back to an older cached
    /// copy (stale) or leaves the set unavailable. It never fails: rule sets
    /// can only degrade routing. A rebuild prepares without downloads; the
    /// refresh task owns retries.
    pub(crate) async fn prepare(
        &self,
        sets: &[RuleSet],
        allowed_hosts: &[String],
        download: Option<Duration>,
    ) -> Snapshot {
        let inner = &self.inner;
        let mut snapshot = Snapshot {
            pinned: allowed_hosts
                .iter()
                .filter_map(|host| normalize_rule_set_host(host).ok())
                .collect(),
            entries: Vec::with_capacity(sets.len()),
        };
        // Copies: the refresh task updates the current entries in place.
        let previous: HashMap<String, Entry> = inner
            .lock()
            .current
            .iter()
            .flat_map(|s| s.entries.iter())
            .map(|e| (e.set.id.clone(), e.clone()))
            .collect();
        let now = inner.now();
        let mut fetch = Vec::new();
        for set in sets {
            let mut e = Entry::new(set.clone());
            if let Some(before) = previous.get(&set.id).filter(|b| b.set == *set) {
                e.err = before.err;
                e.failures = before.failures;
            }
            if inner.opts.dir.is_none() {
                e.err = STORAGE_UNAVAILABLE;
            } else {
                e.local = inner.read_local(&set.id);
                match &e.local {
                    Some(local) if local.sha256 == sha256_hex_of(set) => {
                        e.state = State::Ready;
                        e.err = "";
                        e.failures = 0;
                        e.updated_at = Some(mod_time(&local.path).unwrap_or(now));
                    }
                    local => {
                        if local.is_some() {
                            e.state = State::Stale;
                        }
                        if !snapshot.host_pinned(set) {
                            e.err = HOST_NOT_PINNED;
                        } else if download.is_some() {
                            fetch.push(snapshot.entries.len());
                        }
                    }
                }
            }
            snapshot.entries.push(e);
        }
        if let Some(timeout) = download {
            let deadline = Instant::now() + timeout;
            let fetched = futures_util::future::join_all(fetch.into_iter().map(|i| {
                let mut e = snapshot.entries[i].clone();
                async move {
                    inner.fetch_into(&mut e, deadline).await;
                    (i, e)
                }
            }))
            .await;
            for (i, e) in fetched {
                snapshot.entries[i] = e;
            }
        }
        for e in &mut snapshot.entries {
            e.due = Some(inner.next_due(e, now));
        }
        snapshot
    }

    /// Makes `snapshot` current: reports state changes, removes the cached
    /// files of sets the profile no longer names, and (re)starts the refresh
    /// task. It must be called within a Tokio runtime.
    pub(crate) fn activate(&self, snapshot: Snapshot) {
        let inner = &self.inner;
        let mut keep = HashSet::new();
        let mut changed = Vec::new();
        {
            let mut shared = inner.lock();
            if let Some(task) = shared.task.take() {
                task.abort();
            }
            let previous: HashMap<&str, State> = shared
                .current
                .iter()
                .flat_map(|s| s.entries.iter())
                .map(|e| (e.set.id.as_str(), e.state))
                .collect();
            for e in &snapshot.entries {
                keep.insert(format!("{}.srs", e.set.id));
                if previous.get(e.set.id.as_str()) != Some(&e.state) {
                    changed.push(e.status());
                }
            }
            shared.generation += 1;
            let generation = shared.generation;
            let refresh = !snapshot.entries.is_empty();
            shared.current = Some(snapshot);
            if refresh {
                shared.task = Some(tokio::spawn(run(inner.clone(), generation)));
            }
        }
        inner.prune(&keep);
        inner.notify(changed);
    }

    /// Stops the refresh task.
    pub(crate) fn close(&self) {
        let mut shared = self.inner.lock();
        if let Some(task) = shared.task.take() {
            task.abort();
        }
        shared.generation += 1;
    }

    /// The current sets in profile order.
    pub(crate) fn statuses(&self) -> Vec<Status> {
        self.inner
            .lock()
            .current
            .iter()
            .flat_map(|s| s.entries.iter())
            .map(Entry::status)
            .collect()
    }
}

impl Drop for Manager {
    fn drop(&mut self) {
        self.close();
    }
}

/// Refreshes every set when it is due: ready sets on their update interval,
/// the others with a bounded backoff, the due ones concurrently. When a set
/// that was not ready recovers, the connectivity it was waiting for is
/// probably back, so every other set that is not ready is retried at once
/// instead of on its own backoff. A rebuild is asked for once, after the
/// refreshes due now have all finished (a rebuild replaces the kernel).
async fn run(inner: Arc<Inner>, generation: u64) {
    let mut rebuild_pending = false;
    loop {
        let (due, next, pinned, now) = {
            let shared = inner.lock();
            let Some(current) = shared
                .current
                .as_ref()
                .filter(|_| shared.generation == generation)
            else {
                return;
            };
            let now = inner.now();
            let mut due = Vec::new();
            let mut next: Option<DateTime<Utc>> = None;
            for (i, e) in current.entries.iter().enumerate() {
                match e.due {
                    Some(at) if at > now => next = Some(next.map_or(at, |n| n.min(at))),
                    _ => due.push(i),
                }
            }
            (due, next, current.pinned.clone(), now)
        };
        if due.is_empty() {
            if rebuild_pending {
                if let Some(hook) = &inner.opts.on_rebuild {
                    hook();
                }
            }
            rebuild_pending = false;
            let Some(next) = next else {
                return;
            };
            tokio::time::sleep((next - now).to_std().unwrap_or_default()).await;
            continue;
        }
        let results: Vec<(bool, bool)> = stream::iter(due)
            .map(|i| inner.refresh(generation, &pinned, i))
            .buffer_unordered(REFRESH_CONCURRENCY)
            .collect()
            .await;
        rebuild_pending |= results.iter().any(|&(rebuild, _)| rebuild);
        if results.iter().any(|&(_, recovered)| recovered) {
            let mut shared = inner.lock();
            if shared.generation != generation {
                return;
            }
            let now = inner.now();
            for e in shared.current.iter_mut().flat_map(|s| s.entries.iter_mut()) {
                if e.state != State::Ready && !e.stuck() && e.due.is_some_and(|at| at > now) {
                    e.due = Some(now);
                }
            }
        }
    }
}

impl Inner {
    fn lock(&self) -> MutexGuard<'_, Shared> {
        self.shared.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn now(&self) -> DateTime<Utc> {
        (self.opts.now)()
    }

    fn notify(&self, changed: Vec<Status>) {
        if let Some(hook) = &self.opts.on_state {
            for status in changed {
                hook(status);
            }
        }
    }

    /// Fetches entry `i` of the current snapshot. Returns whether the
    /// configuration must be rebuilt, and whether a set that was not ready
    /// now is.
    async fn refresh(&self, generation: u64, pinned: &HashSet<String>, i: usize) -> (bool, bool) {
        let mut candidate = {
            let shared = self.lock();
            match shared.current.as_ref().and_then(|s| s.entries.get(i)) {
                Some(e) if shared.generation == generation => e.clone(),
                _ => return (false, false),
            }
        };
        if self.opts.dir.is_some() && host_pinned(pinned, &candidate.set) {
            let deadline = Instant::now() + self.opts.fetch_timeout;
            self.fetch_into(&mut candidate, deadline).await;
        }
        let (before, status, rebuild) = {
            let mut shared = self.lock();
            if shared.generation != generation {
                return (false, false);
            }
            let Some(e) = shared.current.as_mut().and_then(|s| s.entries.get_mut(i)) else {
                return (false, false);
            };
            let before = std::mem::replace(e, candidate);
            e.due = Some(self.next_due(e, self.now()));
            let rebuild = (before.state == State::Unavailable) != (e.state == State::Unavailable)
                || matches!((&before.local, &e.local), (Some(a), Some(b)) if a.mirror_dns != b.mirror_dns);
            (before.state, e.status(), rebuild)
        };
        let recovered = before != State::Ready && status.state == State::Ready;
        if before != status.state {
            self.notify(vec![status]);
        }
        (rebuild, recovered)
    }

    fn next_due(&self, e: &Entry, now: DateTime<Utc>) -> DateTime<Utc> {
        let interval = e.set.update_interval();
        if e.state == State::Ready || e.stuck() {
            return after(now, interval);
        }
        let mut backoff = self.opts.retry_min;
        for _ in 1..e.failures {
            if backoff >= self.opts.retry_max {
                break;
            }
            backoff = backoff.saturating_mul(2);
        }
        after(now, backoff.min(self.opts.retry_max).min(interval))
    }

    /// Downloads `e`'s set (within `deadline`) and, when it verifies,
    /// installs it as the local copy.
    async fn fetch_into(&self, e: &mut Entry, deadline: Instant) {
        let want = sha256_hex_of(&e.set);
        let cached = e.local.as_ref().map(|l| l.sha256.clone());
        let fetched =
            tokio::time::timeout_at(deadline, self.client.get(&e.set.url, cached.as_deref()))
                .await
                .unwrap_or(Err(DOWNLOAD_FAILED));
        let now = self.now();
        let outcome = match fetched {
            // The server still serves the cached version: an older one
            // unless it matches.
            Ok(fetch::Fetched::NotModified) if cached.as_deref() == Some(want.as_str()) => Ok(()),
            Ok(fetch::Fetched::NotModified) => Err(SHA256_MISMATCH),
            Ok(fetch::Fetched::Body(body)) => self.install(e, &body, &want).await,
            Err(code) => Err(code),
        };
        match outcome {
            Ok(()) => {
                e.state = State::Ready;
                e.err = "";
                e.failures = 0;
                e.updated_at = Some(now);
            }
            Err(code) => {
                e.err = code;
                e.failures += 1;
            }
        }
    }

    /// Verifies `body` against `want` and atomically replaces the local copy.
    async fn install(&self, e: &mut Entry, body: &[u8], want: &str) -> Result<(), &'static str> {
        if sha256_hex(body) != want {
            return Err(SHA256_MISMATCH);
        }
        let mirror_dns = srs::inspect(body).map_err(|_| INVALID)?;
        let path = self.path(&e.set.id).ok_or(STORAGE_FAILED)?;
        write_atomic(&path, body)
            .await
            .map_err(|_| STORAGE_FAILED)?;
        e.local = Some(LocalCopy {
            path,
            sha256: want.into(),
            mirror_dns,
        });
        Ok(())
    }

    /// The file of rule set `id` inside the directory. Profile validation
    /// already restricts ids to a safe alphabet; this keeps the file inside
    /// the directory even for an id that skipped it.
    fn path(&self, id: &str) -> Option<PathBuf> {
        let dir = self.opts.dir.as_ref()?;
        let name = format!("{id}.srs");
        let mut components = Path::new(&name).components();
        match (components.next(), components.next()) {
            (Some(Component::Normal(_)), None) => Some(dir.join(name)),
            _ => None,
        }
    }

    /// Loads and inspects the cached copy of `id`.
    fn read_local(&self, id: &str) -> Option<LocalCopy> {
        let path = self.path(id)?;
        let data = std::fs::read(&path).ok()?;
        if data.len() > MAX_SIZE {
            return None;
        }
        let mirror_dns = srs::inspect(&data).ok()?;
        Some(LocalCopy {
            path,
            sha256: sha256_hex(&data),
            mirror_dns,
        })
    }

    /// Removes the cached sets not in `keep`, and leftover temporary files.
    fn prune(&self, keep: &HashSet<String>) {
        let Some(dir) = &self.opts.dir else {
            return;
        };
        let Ok(entries) = std::fs::read_dir(dir) else {
            return;
        };
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            let file = entry.file_type().is_ok_and(|t| !t.is_dir());
            if file
                && ((name.ends_with(".srs") && !keep.contains(&name)) || name.starts_with(".tmp-"))
            {
                let _ = std::fs::remove_file(entry.path());
            }
        }
    }
}

/// The profile's digest in lowercase hex.
fn sha256_hex_of(set: &RuleSet) -> String {
    set.sha256.to_ascii_lowercase()
}

fn sha256_hex(data: &[u8]) -> String {
    Sha256::digest(data)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

fn after(now: DateTime<Utc>, d: Duration) -> DateTime<Utc> {
    chrono::Duration::from_std(d)
        .ok()
        .and_then(|d| now.checked_add_signed(d))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

fn mod_time(path: &Path) -> Option<DateTime<Utc>> {
    std::fs::metadata(path)
        .and_then(|m| m.modified())
        .ok()
        .map(DateTime::<Utc>::from)
}

/// Writes `data` to a private temporary file next to `path` and renames it
/// over `path`, so readers (sail's file watcher included) only ever see a
/// complete file.
async fn write_atomic(path: &Path, data: &[u8]) -> io::Result<()> {
    static SEQUENCE: AtomicU64 = AtomicU64::new(0);
    let dir = path
        .parent()
        .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
    let mut builder = std::fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir)?;
    let tmp = dir.join(format!(
        ".tmp-{}-{}",
        std::process::id(),
        SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    let result = match write_new(&tmp, data) {
        Ok(()) => rename(&tmp, path).await,
        Err(e) => Err(e),
    };
    if result.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    result
}

fn write_new(path: &Path, data: &[u8]) -> io::Result<()> {
    use std::io::Write;
    let mut options = std::fs::OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
    let mut file = options.open(path)?;
    file.write_all(data)?;
    file.sync_all()
}

/// Retries a failed replace briefly: on Windows a file still open without
/// FILE_SHARE_DELETE cannot be replaced.
async fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let mut attempt = 0;
    loop {
        match std::fs::rename(from, to) {
            Ok(()) => return Ok(()),
            Err(e) if attempt >= 9 => return Err(e),
            Err(e) if e.kind() != io::ErrorKind::PermissionDenied && !cfg!(windows) => {
                return Err(e)
            }
            Err(_) => {}
        }
        attempt += 1;
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}
