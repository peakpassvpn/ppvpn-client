//! The Runtime on `sail::embed` (sail's docs/embed.md): one `Instance` per
//! Engine, made at `new` and started, reloaded and stopped with the
//! translation's sing-box JSON. sail's error codes pass through as they are
//! (`RuntimeError::to_error` maps them).
//!
//! Three tasks run beside the instance, from `new` until it is dropped:
//! - states: sail's `State` as a [`RuntimeState`];
//! - groups: sail has no typed group events yet (#45, Sail to-dos), so the
//!   groups are polled every [`GROUP_POLL`] while running and a member that
//!   changed becomes a [`GroupSwitch`];
//! - logs: the instance's lines as logfmt, into a bounded channel; lines
//!   that do not fit are dropped and counted, never waited for.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use sail::embed::{self, Address, Config, Instance, LogFilter, Options};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use sail::net::network::{ChangeReason, NetworkState};

use super::{
    AsyncReadWrite, Datagram, GroupInfo, GroupSwitch, MemberInfo, NetworkChange, NetworkSnapshot,
    Runtime, RuntimeConnection, RuntimeError, RuntimeState, RuntimeTraffic, Target,
};
use crate::logfmt;

/// How often the groups are read for switches while running.
pub(crate) const GROUP_POLL: Duration = Duration::from_secs(1);
/// Log lines that wait for the Engine before new ones are dropped.
const LOG_BUFFER: usize = 1024;
/// Group switches that wait for the Engine before new ones are dropped.
const SWITCH_BUFFER: usize = 256;

pub(crate) struct SailRuntime {
    instance: Instance,
    states: watch::Receiver<RuntimeState>,
    switches: Mutex<Option<mpsc::Receiver<GroupSwitch>>>,
    logs: Mutex<Option<mpsc::Receiver<String>>>,
    dropped: Arc<AtomicU64>,
    /// The tun inbound's name in what runs (`tun_name`).
    tun: Mutex<Option<String>>,
    /// sail's network changes, as ours (`network_changes`).
    network: Arc<watch::Sender<Option<NetworkChange>>>,
    /// Follows sail's changes while the runtime runs.
    follow: Mutex<Option<JoinHandle<()>>>,
    tasks: Vec<JoinHandle<()>>,
}

/// An error of the runtime manager (sail's own, outside embed).
fn manager_error(e: sail::Error) -> RuntimeError {
    let code = match e {
        sail::Error::Config(_) => "config",
        _ => "failed",
    };
    RuntimeError::new(code, format!("{e:#}"))
}

fn error(e: embed::Error) -> RuntimeError {
    RuntimeError::new(e.code(), e.message())
}

fn state(s: &embed::State) -> RuntimeState {
    match s {
        embed::State::Idle => RuntimeState::Idle,
        embed::State::Starting => RuntimeState::Starting,
        embed::State::Running { .. } => RuntimeState::Running,
        embed::State::Stopping => RuntimeState::Stopping,
        embed::State::Stopped => RuntimeState::Stopped,
        embed::State::Failed(e) => RuntimeState::Failed {
            code: e.code().into(),
            message: e.message().into(),
        },
        // States sail adds later: read as stopping until mapped.
        _ => RuntimeState::Stopping,
    }
}

fn address(target: Target) -> Address {
    match target {
        Target::Domain(name, port) => Address::Domain(name, port),
        Target::Addr(addr) => Address::from(addr),
    }
}

impl SailRuntime {
    /// An idle instance with `options` (data directory, threads); must be
    /// called on a tokio runtime (the Engine's `new`).
    pub(crate) fn new(options: Options) -> Result<SailRuntime, RuntimeError> {
        let instance = Instance::new(options).map_err(error)?;
        let mut tasks = Vec::new();

        // Subscribe first, then read (embed.md): no change is missed.
        let mut sail_states = instance.states();
        let (state_tx, states) = watch::channel(state(&sail_states.borrow_and_update()));
        tasks.push(tokio::spawn(async move {
            while sail_states.changed().await.is_ok() {
                let next = state(&sail_states.borrow_and_update());
                if state_tx.send(next).is_err() {
                    return;
                }
            }
        }));

        let (switch_tx, switch_rx) = mpsc::channel(SWITCH_BUFFER);
        tasks.push(tokio::spawn(poll_groups(
            instance.clone(),
            states.clone(),
            switch_tx,
        )));

        let (log_tx, log_rx) = mpsc::channel(LOG_BUFFER);
        let dropped = Arc::new(AtomicU64::new(0));
        let mut batches = Box::pin(instance.logs(LogFilter::default().backlog(false)));
        let counted = dropped.clone();
        tasks.push(tokio::spawn(async move {
            while let Some(batch) = batches.next().await {
                counted.fetch_add(batch.dropped, Ordering::Relaxed);
                for l in &batch.lines {
                    let level = l.level.as_str().to_ascii_lowercase();
                    let text = logfmt::line(l.time, &level, &l.message, &[("source", &"sail")]);
                    if log_tx.try_send(text).is_err() {
                        counted.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
        }));

        Ok(SailRuntime {
            instance,
            states,
            switches: Mutex::new(Some(switch_rx)),
            logs: Mutex::new(Some(log_rx)),
            dropped,
            tun: Mutex::new(None),
            network: Arc::new(watch::channel(None).0),
            follow: Mutex::new(None),
            tasks,
        })
    }
}

impl SailRuntime {
    /// Forwards sail's network changes to `network` while this run lasts.
    /// Until sail::embed has network events (E1b), this reads them through
    /// the runtime manager, which is outside embed's stable API: the same
    /// channel sail's own reaction to a move reads (one monitor).
    fn follow_network(&self) {
        let Ok(manager) = self.instance.manager() else {
            return;
        };
        let mut changes = manager.network().changes();
        // What was published before this start is the previous run's.
        changes.borrow_and_update();
        let network = self.network.clone();
        let task = tokio::spawn(async move {
            while changes.changed().await.is_ok() {
                let change = changes.borrow_and_update().clone();
                if let Some(change) = change {
                    network.send_replace(Some(NetworkChange {
                        generation: change.generation,
                        reason: reason(change.reason).into(),
                        old: snapshot(&change.old),
                        new: snapshot(&change.new),
                    }));
                }
            }
        });
        if let Some(previous) = self.follow.lock().expect("follow").replace(task) {
            previous.abort();
        }
    }
}

fn reason(reason: ChangeReason) -> &'static str {
    match reason {
        ChangeReason::DefaultInterface => "default_interface",
        ChangeReason::State => "state",
        ChangeReason::HostPush => "host",
        ChangeReason::Wake => "wake",
    }
}

/// A state with nothing known is no network.
fn snapshot(state: &NetworkState) -> NetworkSnapshot {
    NetworkSnapshot {
        interface: state.interface.clone(),
        index: state.index,
        gateway: state.gateway,
        addresses: state.addresses.iter().map(|a| a.to_string()).collect(),
        offline: *state == NetworkState::default(),
    }
}

impl Drop for SailRuntime {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Some(follow) = self.follow.get_mut().ok().and_then(Option::take) {
            follow.abort();
        }
    }
}

/// Reads the groups every GROUP_POLL while running; a group whose member in
/// use changed is a switch: `selected` for a selector (only `select` moves
/// one) or a fallback fixed by `select`, else `failover`.
async fn poll_groups(
    instance: Instance,
    mut states: watch::Receiver<RuntimeState>,
    tx: mpsc::Sender<GroupSwitch>,
) {
    let mut last: HashMap<String, String> = HashMap::new();
    loop {
        if *states.borrow_and_update() != RuntimeState::Running {
            last.clear();
            if states.changed().await.is_err() {
                return;
            }
            continue;
        }
        if let Ok(groups) = instance.groups().await {
            let mut now = HashMap::new();
            for g in groups {
                let Some(group) = g.group else { continue };
                if let Some(from) = last.get(&g.tag) {
                    if *from != group.selected {
                        let reason = if group.selectable || group.fixed.is_some() {
                            "selected"
                        } else {
                            "failover"
                        };
                        let switch = GroupSwitch {
                            group: g.tag.clone(),
                            from: from.clone(),
                            to: group.selected.clone(),
                            reason: reason.into(),
                        };
                        let _ = tx.try_send(switch);
                    }
                }
                now.insert(g.tag, group.selected);
            }
            last = now;
        }
        tokio::select! {
            _ = tokio::time::sleep(GROUP_POLL) => {}
            changed = states.changed() => if changed.is_err() { return },
        }
    }
}

/// A member's health from its delay history, newest last: alive by the last
/// test, failures counted back from it.
fn member(tag: &str, outbounds: &HashMap<String, embed::OutboundInfo>) -> MemberInfo {
    let history = outbounds
        .get(tag)
        .map(|o| o.history.as_slice())
        .unwrap_or(&[]);
    let last = history.last();
    MemberInfo {
        tag: tag.into(),
        alive: last.map(|d| d.delay.is_some()),
        last_check: last.map(|d| d.time),
        consecutive_failures: history
            .iter()
            .rev()
            .take_while(|d| d.delay.is_none())
            .count() as u32,
    }
}

struct SailDatagram(embed::DialDatagram);

#[async_trait]
impl Datagram for SailDatagram {
    async fn send(&self, data: &[u8]) -> std::io::Result<()> {
        self.0.send(data).await
    }
    async fn recv(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        self.0.recv_from(buf).await.map(|(n, _)| n)
    }
}

#[async_trait]
impl Runtime for SailRuntime {
    async fn start(&self, config: &str) -> Result<(), RuntimeError> {
        self.instance
            .start(Config::Json(config.into()))
            .await
            .map_err(error)?;
        *self.tun.lock().expect("tun") = super::configured_tun_name(config);
        self.follow_network();
        Ok(())
    }

    async fn reload(&self, config: &str) -> Result<(), RuntimeError> {
        self.instance
            .reload(Some(Config::Json(config.into())))
            .await
            .map_err(error)?;
        *self.tun.lock().expect("tun") = super::configured_tun_name(config);
        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        self.instance.stop().await.map_err(error)?;
        *self.tun.lock().expect("tun") = None;
        if let Some(follow) = self.follow.lock().expect("follow").take() {
            follow.abort();
        }
        Ok(())
    }

    fn states(&self) -> watch::Receiver<RuntimeState> {
        self.states.clone()
    }

    fn state(&self) -> RuntimeState {
        state(&self.instance.state())
    }

    async fn groups(&self) -> Result<Vec<GroupInfo>, RuntimeError> {
        let groups = self.instance.groups().await.map_err(error)?;
        let outbounds: HashMap<String, embed::OutboundInfo> = self
            .instance
            .outbounds()
            .await
            .map_err(error)?
            .into_iter()
            .map(|o| (o.tag.clone(), o))
            .collect();
        Ok(groups
            .into_iter()
            .filter_map(|g| {
                let group = g.group?;
                Some(GroupInfo {
                    tag: g.tag,
                    now: group.selected,
                    members: group
                        .members
                        .iter()
                        .map(|m| member(m, &outbounds))
                        .collect(),
                    fixed: group.fixed.is_some(),
                })
            })
            .collect())
    }

    async fn select(&self, group: &str, member: &str) -> Result<(), RuntimeError> {
        self.instance.select(group, member).await.map_err(error)
    }

    async fn unfix(&self, group: &str) -> Result<(), RuntimeError> {
        self.instance.unfix(group).await.map_err(error)
    }

    fn group_switches(&self) -> mpsc::Receiver<GroupSwitch> {
        self.switches
            .lock()
            .expect("switches")
            .take()
            .expect("group_switches is taken once")
    }

    async fn traffic(&self) -> Result<RuntimeTraffic, RuntimeError> {
        let t = self.instance.traffic().await.map_err(error)?;
        Ok(RuntimeTraffic {
            upload_bytes: t.up_total,
            download_bytes: t.down_total,
        })
    }

    async fn connections(&self) -> Result<Vec<RuntimeConnection>, RuntimeError> {
        let connections = self.instance.connections().await.map_err(error)?;
        Ok(connections
            .into_iter()
            .map(|c| RuntimeConnection {
                id: c.id,
                inbound: c.inbound_tag,
                chain: c.chains,
                network: match c.network {
                    embed::Network::Tcp => "tcp".into(),
                    embed::Network::Udp => "udp".into(),
                },
                destination: c.destination.to_string(),
                upload_bytes: c.upload,
                download_bytes: c.download,
                started: UNIX_EPOCH + Duration::from_secs(u64::from(c.start)),
            })
            .collect())
    }

    async fn close_connection(&self, id: u64) -> Result<bool, RuntimeError> {
        self.instance.close_connection(id).await.map_err(error)
    }

    async fn dial_tcp(
        &self,
        outbound: &str,
        to: Target,
        timeout: Duration,
    ) -> Result<Box<dyn AsyncReadWrite>, RuntimeError> {
        let stream = self
            .instance
            .dial_tcp(outbound, address(to), timeout)
            .await
            .map_err(error)?;
        Ok(Box::new(stream))
    }

    async fn dial_udp(
        &self,
        outbound: &str,
        to: Target,
        timeout: Duration,
    ) -> Result<Box<dyn Datagram>, RuntimeError> {
        let datagram = self
            .instance
            .dial_udp(outbound, address(to), timeout)
            .await
            .map_err(error)?;
        Ok(Box::new(SailDatagram(datagram)))
    }

    /// One add, replace or remove per user (sail has no call for the whole
    /// set): users not in `users` go, the others are replaced or added.
    async fn replace_inbound_users(
        &self,
        inbound: &str,
        users: Vec<(String, String)>,
    ) -> Result<(), RuntimeError> {
        let current = self
            .instance
            .inbound_users(inbound)
            .map_err(error)?
            .ok_or_else(|| {
                RuntimeError::new("not_found", format!("inbound {inbound} has no users"))
            })?;
        for (name, password) in &users {
            let user = serde_json::json!({ "username": name, "password": password });
            if current.contains(name) {
                self.instance
                    .replace_inbound_user(inbound, name, user)
                    .await
                    .map_err(error)?;
            } else {
                self.instance
                    .add_inbound_user(inbound, user)
                    .await
                    .map_err(error)?;
            }
        }
        for name in current
            .iter()
            .filter(|n| !users.iter().any(|(u, _)| u == *n))
        {
            self.instance
                .remove_inbound_user(inbound, name)
                .await
                .map_err(error)?;
        }
        Ok(())
    }

    async fn network_changed(&self) -> Result<(), RuntimeError> {
        self.instance.network_changed(None).await.map_err(error)
    }

    fn logs(&self) -> mpsc::Receiver<String> {
        self.logs
            .lock()
            .expect("logs")
            .take()
            .expect("logs is taken once")
    }

    fn dropped_log_lines(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn tun_name(&self) -> Option<String> {
        self.tun.lock().expect("tun").clone()
    }

    fn network(&self) -> Option<NetworkSnapshot> {
        let manager = self.instance.manager().ok()?;
        let network = manager.network();
        let mut now = snapshot(&network.snapshot());
        now.offline = network.is_down();
        Some(now)
    }

    async fn add_inbound(&self, inbound: &str) -> Result<(), RuntimeError> {
        // Through the runtime manager, outside sail::embed's stable API,
        // until embed offers it (rust-parity: transitional).
        let manager = self.instance.manager().map_err(error)?;
        let mut config = sail::config::from_string(&format!(r#"{{"inbounds":[{inbound}]}}"#))
            .map_err(|e| RuntimeError::new("config", format!("{e:#}")))?;
        let inbound = config
            .inbounds
            .pop()
            .ok_or_else(|| RuntimeError::new("config", "no inbound"))?;
        manager.add_inbound(inbound).await.map_err(manager_error)
    }

    async fn remove_inbound(&self, tag: &str) -> Result<(), RuntimeError> {
        let manager = self.instance.manager().map_err(error)?;
        manager.remove_inbound(tag).await.map_err(manager_error)
    }

    fn network_changes(&self) -> watch::Receiver<Option<NetworkChange>> {
        self.network.subscribe()
    }
}

/// Seconds since the epoch, for tests that compare `started`.
#[cfg(test)]
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

#[cfg(test)]
#[path = "sail_tests.rs"]
mod tests;
