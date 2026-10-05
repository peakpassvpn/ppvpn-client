//! The guard (Linux): one task that hears the deletions in the scope, checks
//! the snapshot against what is listed, and puts back what is missing.

use std::ffi::CString;
use std::io;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::unix::AsyncFd;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::netlink::{Netlink, Notification, Watch};
use super::{
    missing, names, restore_order, route_restore_order, Route, Rule, Scope, TunRoutingStatus,
};

/// Timing, and whether to put back at all; tests shorten and switch off.
#[derive(Debug, Clone)]
pub(crate) struct Tuning {
    /// Merges a burst of deletions (networkd drops the rules one by one)
    /// into one check.
    pub settle: Duration,
    /// The backstop for notifications lost to a full socket buffer
    /// (ENOBUFS) or never sent.
    pub periodic: Duration,
    /// More restores than `flap_limit` within `flap_window` is logged as an
    /// error (something keeps deleting them); restoring goes on.
    pub flap_window: Duration,
    pub flap_limit: usize,
    /// Off: what is missing is reported `Broken` at once, so a test can
    /// prove the putting back is what brings it back.
    pub restore: bool,
}

impl Default for Tuning {
    fn default() -> Tuning {
        Tuning {
            settle: Duration::from_millis(50),
            periodic: Duration::from_secs(30),
            flap_window: Duration::from_secs(10),
            flap_limit: 10,
            restore: true,
        }
    }
}

/// Watches the scope and puts back what goes missing from the snapshot.
/// Stop it before the TUN closes: sail's own cleanup must not be undone.
/// Dropping it stops it too.
pub(crate) struct Guard {
    requests: Option<mpsc::Sender<Request>>,
    /// Set by `stop`; a check holds it from its first read to its last
    /// write, so once `stop` has it, the routing is no longer touched.
    stopped: Arc<Mutex<bool>>,
    task: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone)]
struct Request {
    reason: String,
    /// The netlink port that asked for a deletion (0: the kernel); named
    /// only when a check acts on it.
    sender: Option<u32>,
    settle: bool,
}

impl Guard {
    /// Snapshots the scope and starts watching it; on a tokio runtime, right
    /// after sail started the TUN. Never fails: a guard that cannot start
    /// reports `Unguarded` and does nothing, the routing being as sail
    /// installed it.
    pub(crate) fn start(scope: Scope) -> (Guard, watch::Receiver<TunRoutingStatus>) {
        Guard::start_with(scope, Tuning::default())
    }

    pub(crate) fn start_with(
        scope: Scope,
        tuning: Tuning,
    ) -> (Guard, watch::Receiver<TunRoutingStatus>) {
        let stopped = Arc::new(Mutex::new(false));
        match Worker::new(scope, tuning, stopped.clone()) {
            Ok((worker, watch)) => {
                let (status, statuses) = watch::channel(TunRoutingStatus::Ok);
                let (requests, requests_rx) = mpsc::channel(16);
                let task = tokio::spawn(worker.run(watch, requests_rx, status));
                let guard = Guard {
                    requests: Some(requests),
                    stopped,
                    task: Some(task),
                };
                (guard, statuses)
            }
            Err(e) => {
                tracing::error!(error = %e, "tun routing unguarded");
                let (_, statuses) = watch::channel(TunRoutingStatus::Unguarded {
                    error: e.to_string(),
                });
                let guard = Guard {
                    requests: None,
                    stopped,
                    task: None,
                };
                (guard, statuses)
            }
        }
    }

    /// Asks for a check now (after a kernel switch, a default interface
    /// change). A check already queued sees the same state.
    pub(crate) fn check(&self, reason: &str) {
        if let Some(requests) = &self.requests {
            let _ = requests.try_send(Request {
                reason: reason.to_owned(),
                sender: None,
                settle: false,
            });
        }
    }

    /// Stops it; on return it no longer touches the routing (a check under
    /// way is waited for: milliseconds). Synchronous, for a cleanup step.
    pub(crate) fn stop(mut self) {
        self.halt();
    }

    fn halt(&mut self) {
        *self.stopped.lock().unwrap_or_else(|e| e.into_inner()) = true;
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

impl Drop for Guard {
    fn drop(&mut self) {
        self.halt();
    }
}

/// Removes every rule of either family in the scope's priorities and every
/// route of its table, whatever made them: what a previous instance left
/// (a crash before sail's cleanup). Nothing else is touched.
pub(crate) fn sweep(scope: &Scope) -> io::Result<usize> {
    let deleted = Netlink::open()?.delete_scope(scope.rule_start..=scope.rule_end, scope.table)?;
    if deleted > 0 {
        tracing::info!(deleted, "tun routing leftovers removed");
    }
    Ok(deleted)
}

struct Worker {
    scope: Scope,
    tuning: Tuning,
    link: u32,
    netlink: Netlink,
    rules: Vec<Rule>,
    routes: Vec<Route>,
    broken: bool,
    restores: Vec<Instant>,
    stopped: Arc<Mutex<bool>>,
}

impl Worker {
    /// Subscribes first, so no deletion after the snapshot goes unheard.
    fn new(scope: Scope, tuning: Tuning, stopped: Arc<Mutex<bool>>) -> io::Result<(Worker, Watch)> {
        let link = link_index(&scope.interface)?;
        let watch = Watch::open()?;
        let mut netlink = Netlink::open()?;
        let (owned, foreign) = scope.owned(&netlink.rules()?);
        for rule in &foreign {
            tracing::info!(rule = %rule, "tun routing rule not ours");
        }
        if owned.is_empty() {
            return Err(io::Error::other(format!(
                "no routing rules of {} at priorities {}-{}",
                scope.interface, scope.rule_start, scope.rule_end
            )));
        }
        let routes = netlink.routes(scope.table, link)?;
        let worker = Worker {
            scope,
            tuning,
            link,
            netlink,
            rules: owned,
            routes,
            broken: false,
            restores: Vec::new(),
            stopped,
        };
        Ok((worker, watch))
    }

    async fn run(
        mut self,
        watch: Watch,
        mut requests: mpsc::Receiver<Request>,
        status: watch::Sender<TunRoutingStatus>,
    ) {
        let watch = match AsyncFd::new(watch) {
            Ok(watch) => watch,
            Err(e) => {
                tracing::error!(error = %e, "tun routing unguarded");
                status.send_replace(TunRoutingStatus::Unguarded {
                    error: e.to_string(),
                });
                return;
            }
        };
        let mut periodic = tokio::time::interval(self.tuning.periodic);
        periodic.tick().await;
        // A check waiting for its deletions to settle: why, by whom, when.
        let mut pending: Option<(Request, tokio::time::Instant)> = None;
        loop {
            let deadline = pending.as_ref().map(|(_, at)| *at);
            let request = tokio::select! {
                ready = watch.readable() => {
                    let Ok(mut ready) = ready else { return };
                    match ready.get_inner().read() {
                        Ok(notes) => {
                            ready.clear_ready();
                            notes.iter().find_map(|n| self.relevant(n))
                        }
                        Err(e) if e.raw_os_error() == Some(libc::ENOBUFS) => Some(Request {
                            reason: "notifications lost".into(),
                            sender: None,
                            settle: true,
                        }),
                        Err(e) => {
                            tracing::warn!(error = %e, "tun routing watch");
                            tokio::time::sleep(Duration::from_secs(1)).await;
                            None
                        }
                    }
                }
                Some(request) = requests.recv() => Some(request),
                _ = sleep_until(deadline) => {
                    let (request, _) = pending.take().expect("a deadline is a pending check");
                    self.check(&request, &status);
                    None
                }
                _ = periodic.tick() => {
                    let request = Request { reason: "periodic".into(), sender: None, settle: false };
                    self.check(&request, &status);
                    None
                }
            };
            let Some(request) = request else { continue };
            if !request.settle {
                self.check(&request, &status);
                continue;
            }
            match &mut pending {
                None => {
                    let at = tokio::time::Instant::now() + self.tuning.settle;
                    pending = Some((request, at));
                }
                // Keep the first reason, but name who deleted when known.
                Some((first, _)) if first.sender.is_none() && request.sender.is_some() => {
                    *first = request;
                }
                Some(_) => {}
            }
        }
    }

    /// A deletion in the scope becomes a check, after the burst settles.
    fn relevant(&self, n: &Notification) -> Option<Request> {
        let (reason, sender) = match *n {
            Notification::RuleDeleted { priority, sender } if self.scope.in_range(priority) => {
                ("rule deleted", sender)
            }
            Notification::RouteDeleted { table, sender } if table == self.scope.table => {
                ("route deleted", sender)
            }
            _ => return None,
        };
        Some(Request {
            reason: reason.into(),
            sender: Some(sender),
            settle: true,
        })
    }

    fn missing(&mut self) -> io::Result<(Vec<Rule>, Vec<Route>)> {
        let rules = self.netlink.rules()?;
        let routes = self.netlink.routes(self.scope.table, self.link)?;
        Ok((missing(&self.rules, &rules), missing(&self.routes, &routes)))
    }

    fn check(&mut self, request: &Request, status: &watch::Sender<TunRoutingStatus>) {
        let stopped = self.stopped.clone();
        let stopped = stopped.lock().unwrap_or_else(|e| e.into_inner());
        if *stopped {
            return;
        }
        let (rules, routes) = match self.missing() {
            Ok(found) => found,
            Err(e) => {
                tracing::warn!(reason = %request.reason, error = %e, "tun routing check");
                return;
            }
        };
        if rules.is_empty() && routes.is_empty() {
            if self.broken {
                self.broken = false;
                tracing::info!("tun routing ok again");
                status.send_replace(TunRoutingStatus::Restored {
                    missing: Vec::new(),
                });
            }
            return;
        }
        let missing_names = names(&rules, &routes);
        if !self.tuning.restore {
            self.set_broken(missing_names, "restoring disabled".into(), status);
            return;
        }
        if !self.broken {
            status.send_replace(TunRoutingStatus::Restoring {
                missing: missing_names.clone(),
            });
        }
        let added = self.restore(&rules, &routes);
        match self.missing() {
            Ok((still_rules, still_routes))
                if still_rules.is_empty() && still_routes.is_empty() =>
            {
                // Named after the putting back: it reads all of /proc.
                let by = request.sender.map_or("unknown".into(), sender_name);
                tracing::warn!(
                    missing = %missing_names.join(", "),
                    by = %by,
                    reason = %request.reason,
                    "tun routing rules restored"
                );
                self.flapped();
                self.broken = false;
                status.send_replace(TunRoutingStatus::Restored {
                    missing: missing_names,
                });
            }
            Ok((still_rules, still_routes)) => {
                let error = added
                    .err()
                    .map_or("still missing after restoring".into(), |e| e.to_string());
                self.set_broken(names(&still_rules, &still_routes), error, status);
            }
            Err(e) => self.set_broken(missing_names, e.to_string(), status),
        }
    }

    fn set_broken(
        &mut self,
        missing: Vec<String>,
        error: String,
        status: &watch::Sender<TunRoutingStatus>,
    ) {
        tracing::error!(missing = %missing.join(", "), error = %error, "tun routing broken");
        if !self.broken {
            self.broken = true;
            status.send_replace(TunRoutingStatus::Broken { missing, error });
        }
    }

    fn flapped(&mut self) {
        let now = Instant::now();
        let window = self.tuning.flap_window;
        self.restores.retain(|at| now.duration_since(*at) < window);
        self.restores.push(now);
        if self.restores.len() == self.tuning.flap_limit + 1 {
            tracing::error!(
                restores = self.restores.len(),
                window_s = window.as_secs(),
                "tun routing rules keep being deleted"
            );
        }
    }

    /// Routes first (the rules point at their table), then rules; EEXIST is
    /// not an error: the entry came back meanwhile. Goes on past a failure
    /// and returns the first.
    fn restore(&mut self, rules: &[Rule], routes: &[Route]) -> io::Result<()> {
        let mut first = None;
        let exists = |e: &io::Error| e.raw_os_error() == Some(libc::EEXIST);
        for route in route_restore_order(routes) {
            if let Err(e) = self.netlink.add_route(&route, self.scope.table) {
                if !exists(&e) {
                    first.get_or_insert(io::Error::new(e.kind(), format!("route {route}: {e}")));
                }
            }
        }
        for rule in restore_order(rules) {
            if let Err(e) = self.netlink.add_rule(&rule) {
                if !exists(&e) {
                    first.get_or_insert(io::Error::new(e.kind(), format!("rule {rule}: {e}")));
                }
            }
        }
        first.map_or(Ok(()), Err)
    }
}

async fn sleep_until(deadline: Option<tokio::time::Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => std::future::pending().await,
    }
}

fn link_index(name: &str) -> io::Result<u32> {
    let c_name = CString::new(name).map_err(|e| io::Error::new(io::ErrorKind::InvalidInput, e))?;
    // SAFETY: a valid NUL-terminated string, read only for the call.
    match unsafe { libc::if_nametoindex(c_name.as_ptr()) } {
        0 => Err(io::Error::new(
            io::ErrorKind::NotFound,
            format!("find {name}: {}", io::Error::last_os_error()),
        )),
        index => Ok(index),
    }
}

/// Names the process behind a notification's netlink port id. The id is
/// not a pid: socket-activated daemons (systemd-networkd) use a socket PID 1
/// created. So the socket is found by port id in /proc/net/netlink and its
/// holders by inode; a holder other than PID 1 is preferred. Best effort:
/// "unknown" when nothing matches.
fn sender_name(port: u32) -> String {
    if port == 0 {
        return "kernel".into();
    }
    let Some(inode) = netlink_inode(port) else {
        return "unknown".into();
    };
    let target = format!("socket:[{inode}]");
    let mut holders: Vec<String> = Vec::new();
    let Ok(entries) = std::fs::read_dir("/proc") else {
        return "unknown".into();
    };
    for entry in entries.flatten() {
        let pid = entry.file_name().to_string_lossy().into_owned();
        if !pid.starts_with(|c: char| c.is_ascii_digit()) {
            continue;
        }
        let Ok(fds) = std::fs::read_dir(format!("/proc/{pid}/fd")) else {
            continue;
        };
        let holds = fds.flatten().any(|fd| {
            std::fs::read_link(fd.path()).is_ok_and(|link| link.as_os_str() == target.as_str())
        });
        if holds {
            let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
            let name = format!("{} (pid {pid})", comm.trim());
            if pid == "1" {
                holders.push(name);
            } else {
                holders.insert(0, name);
            }
        }
    }
    holders
        .into_iter()
        .next()
        .unwrap_or_else(|| "unknown".into())
}

/// The inode of the NETLINK_ROUTE socket bound to `port`.
fn netlink_inode(port: u32) -> Option<String> {
    let table = std::fs::read_to_string("/proc/net/netlink").ok()?;
    let port = port.to_string();
    // sk Eth Pid Groups Rmem Wmem Dump Locks Drops Inode
    table.lines().find_map(|line| {
        let fields: Vec<&str> = line.split_whitespace().collect();
        (fields.len() >= 10 && fields[1] == "0" && fields[2] == port).then(|| fields[9].to_owned())
    })
}
