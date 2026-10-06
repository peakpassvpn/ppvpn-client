//! The Runtime on `sail::embed` (sail's docs/embed.md): one `Instance` per
//! Engine, made at `new` and started, reloaded and stopped with the
//! translation's sing-box JSON. sail's error codes pass through as they are
//! (`RuntimeError::to_error` maps them).
//!
//! Five tasks run beside the instance, from `new` until it is dropped:
//! - states: sail's `State` as a [`RuntimeState`];
//! - groups: sail's group events (`events(Kinds::GROUP)`: fallback and
//!   url-test switches) as [`GroupSwitch`]es; a subscriber that fell behind
//!   reads the groups again. sail tells no selector's switch by hand, so
//!   `select` tells those itself;
//! - dial failures: sail's `events(Kinds::DIAL)` as [`DialFailed`];
//! - routes, only once `routes()` is taken (the Engine logs at debug):
//!   sail's `events(Kinds::ROUTE)` as [`Routed`], built by sail only while
//!   someone subscribes;
//! - DNS exchanges, likewise once `dns_exchanges()` is taken:
//!   `events(Kinds::DNS)` as [`DnsExchange`];
//! - logs: the instance's lines as logfmt, into a bounded channel; lines
//!   that do not fit are dropped and counted, never waited for;
//! - network: sail's network events (`instance.events(Kinds::NETWORK)`,
//!   through stops and starts) as [`NetworkChange`]s; a subscriber that
//!   fell behind reads the snapshot again;
//! - system changes: sail's `events(Kinds::SYSTEM)` (a TUN's route or
//!   address changed by another program) as [`SystemChange`]s.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, UNIX_EPOCH};

use async_trait::async_trait;
use futures_util::StreamExt;
use sail::embed::{self, Address, Config, Instance, LogFilter, Options};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use super::{
    AsyncReadWrite, Datagram, DialFailed, DnsExchange, GroupInfo, GroupSwitch, MemberInfo,
    NetworkChange, NetworkSnapshot, ReloadReport, Routed, Runtime, RuntimeConnection, RuntimeError,
    RuntimeState, RuntimeTraffic, SystemChange, Target,
};
use crate::logfmt;
use crate::types::{Leftover, LeftoverKind};

/// Log lines that wait for the Engine before new ones are dropped.
const LOG_BUFFER: usize = 1024;
/// Group switches that wait for the Engine before new ones are dropped.
const SWITCH_BUFFER: usize = 256;
/// Dial failures that wait for the Engine before new ones are dropped
/// (sail already folds a chain's failures into one with a count).
const DIAL_BUFFER: usize = 256;
/// Routed connections that wait for the Engine before new ones are dropped.
const ROUTE_BUFFER: usize = 1024;
/// DNS exchanges that wait for the Engine before new ones are dropped
/// (queries come in bursts at start).
const DNS_BUFFER: usize = 256;
/// System changes that wait for the Engine before new ones are dropped
/// (sail tells a break once).
const SYSTEM_BUFFER: usize = 16;

/// Each group's member as last told, by tag: what a switch is from, and
/// what the groups are compared with after falling behind.
type Members = Arc<Mutex<HashMap<String, String>>>;

pub(crate) struct SailRuntime {
    instance: Instance,
    states: watch::Receiver<RuntimeState>,
    switches: Mutex<Option<mpsc::Receiver<GroupSwitch>>>,
    /// For the switches `select` tells.
    switch_tx: mpsc::Sender<GroupSwitch>,
    members: Members,
    failures: Mutex<Option<mpsc::Receiver<DialFailed>>>,
    systems: Mutex<Option<mpsc::Receiver<SystemChange>>>,
    /// The routes' follower, started by the first `routes()`.
    route_task: Mutex<Option<JoinHandle<()>>>,
    /// The DNS exchanges' follower, started by the first `dns_exchanges()`.
    dns_task: Mutex<Option<JoinHandle<()>>>,
    logs: Mutex<Option<mpsc::Receiver<String>>>,
    dropped: Arc<AtomicU64>,
    /// sail's network changes, as ours (`network_changes`).
    network: Arc<watch::Sender<Option<NetworkChange>>>,
    tasks: Vec<JoinHandle<()>>,
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
        let members = Members::default();
        tasks.push(tokio::spawn(follow_groups(
            instance.clone(),
            members.clone(),
            switch_tx.clone(),
        )));

        let (dial_tx, dial_rx) = mpsc::channel(DIAL_BUFFER);
        tasks.push(tokio::spawn(follow_dials(instance.clone(), dial_tx)));

        let (system_tx, system_rx) = mpsc::channel(SYSTEM_BUFFER);
        tasks.push(tokio::spawn(follow_system(instance.clone(), system_tx)));

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

        // Subscribed at once and through every run (embed.md, The network).
        let network = Arc::new(watch::channel(None).0);
        tasks.push(tokio::spawn(follow_network(
            instance.clone(),
            network.clone(),
        )));

        Ok(SailRuntime {
            instance,
            states,
            switches: Mutex::new(Some(switch_rx)),
            switch_tx,
            members,
            failures: Mutex::new(Some(dial_rx)),
            systems: Mutex::new(Some(system_rx)),
            route_task: Mutex::new(None),
            dns_task: Mutex::new(None),
            logs: Mutex::new(Some(log_rx)),
            dropped,
            network,
            tasks,
        })
    }
}

/// Forwards sail's network events to `network` as [`NetworkChange`]s.
/// After a `Lagged` (more than sail keeps for a slow subscriber), the change
/// is made up from the snapshot: from the last state passed on to the one
/// now, under the snapshot's generation, so the Engine sees where it is.
async fn follow_network(instance: Instance, network: Arc<watch::Sender<Option<NetworkChange>>>) {
    let mut events = Box::pin(instance.events(embed::Kinds::NETWORK));
    let mut last = NetworkSnapshot::default();
    while let Some(event) = events.next().await {
        let change = match event {
            embed::Event::Network(e) => NetworkChange {
                generation: e.generation,
                change: change_kind(e.change).into(),
                reason: reason(e.reason).into(),
                old: snapshot(&e.old),
                new: snapshot(&e.new),
            },
            embed::Event::Lagged { .. } => {
                let Ok(now) = instance.network() else {
                    continue;
                };
                let new = snapshot(&now);
                NetworkChange {
                    generation: now.generation,
                    change: super::made_up_kind(&last, &new).into(),
                    reason: "lagged".into(),
                    old: last.clone(),
                    new,
                }
            }
            _ => continue,
        };
        last = change.new.clone();
        network.send_replace(Some(change));
    }
}

fn change_kind(kind: embed::NetworkChangeKind) -> &'static str {
    match kind {
        embed::NetworkChangeKind::InterfaceChanged => "interface_changed",
        embed::NetworkChangeKind::Moved => "moved",
        embed::NetworkChangeKind::Offline => "offline",
        embed::NetworkChangeKind::Restored => "restored",
        _ => "moved",
    }
}

fn reason(reason: embed::NetworkChangeReason) -> &'static str {
    match reason {
        embed::NetworkChangeReason::DefaultInterface => "default_interface",
        embed::NetworkChangeReason::Detected => "state",
        embed::NetworkChangeReason::Host => "host",
        embed::NetworkChangeReason::Wake => "wake",
        _ => "state",
    }
}

fn snapshot(state: &embed::NetworkState) -> NetworkSnapshot {
    NetworkSnapshot {
        interface: state.interface.as_ref().map(|i| i.name.clone()),
        index: state.interface.as_ref().and_then(|i| i.index),
        gateway: state.gateway,
        addresses: state
            .addresses
            .iter()
            .map(|(ip, len)| format!("{ip}/{len}"))
            .collect(),
        offline: state.offline(),
    }
}

impl Drop for SailRuntime {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if let Some(task) = self.route_task.lock().expect("route task").take() {
            task.abort();
        }
        if let Some(task) = self.dns_task.lock().expect("dns task").take() {
            task.abort();
        }
    }
}

/// A group's own tag: sail names a group inside others after them
/// (`outer>inner`).
fn group_tag(path: &str) -> &str {
    path.rsplit('>').next().unwrap_or(path)
}

fn switch_reason(reason: embed::SwitchReason) -> &'static str {
    use embed::SwitchReason as R;
    match reason {
        R::MemberDown => "member_down",
        R::TestFailed => "test_failed",
        R::Recovered => "recovered",
        R::AllDown => "all_down",
        R::Pinned => "pinned",
        R::Unpinned => "unpinned",
        R::Selected => "selected",
        R::Faster => "faster",
        R::MembersChanged => "members_changed",
        _ => "other",
    }
}

/// sail's group switches, through every run. One with no `from` (a
/// group's first choice) is only noted. Behind (`Lagged`): the groups are
/// read again, and each that differs from what was told is a switch with
/// reason `lagged`.
async fn follow_groups(instance: Instance, members: Members, tx: mpsc::Sender<GroupSwitch>) {
    let mut events = Box::pin(instance.events(embed::Kinds::GROUP));
    while let Some(event) = events.next().await {
        match event {
            embed::Event::GroupSwitched(s) => {
                let group = group_tag(&s.group).to_owned();
                members
                    .lock()
                    .expect("members")
                    .insert(group.clone(), s.to.clone());
                if let Some(from) = s.from {
                    let _ = tx.try_send(GroupSwitch {
                        group,
                        from,
                        to: s.to,
                        reason: switch_reason(s.reason).into(),
                    });
                }
            }
            embed::Event::Lagged { kind, .. } if kind.contains(embed::Kinds::GROUP) => {
                let Ok(groups) = instance.groups().await else {
                    continue;
                };
                let mut members = members.lock().expect("members");
                for g in groups {
                    let Some(group) = g.group else { continue };
                    let from = members.insert(g.tag.clone(), group.selected.clone());
                    if let Some(from) = from.filter(|from| *from != group.selected) {
                        let _ = tx.try_send(GroupSwitch {
                            group: g.tag,
                            from,
                            to: group.selected,
                            reason: "lagged".into(),
                        });
                    }
                }
            }
            _ => {}
        }
    }
}

fn routed(r: &embed::RoutedConnection) -> Routed {
    use embed::{DomainSource as S, RouteAction as A};
    Routed {
        id: r.id,
        network: r.network.to_string(),
        inbound: r.inbound.clone(),
        source: r.source.to_string(),
        destination: r.destination.to_string(),
        domain: r.domain.clone(),
        domain_source: r.domain_source.map(|s| {
            match s {
                S::Request => "request",
                S::FakeIp => "fake_ip",
                S::Sniffed => "sniffed",
                S::ReverseMapping => "reverse_mapping",
                _ => "other",
            }
            .to_owned()
        }),
        protocol: r.sniffed_protocol.map(str::to_owned),
        rule: r.rule.map(|i| i as usize),
        action: match r.action {
            A::Outbound => "outbound",
            A::Reject => "reject",
            A::Drop => "drop",
            A::HijackDns => "hijack_dns",
            _ => "other",
        }
        .to_owned(),
        chain: r.chain.clone(),
        request_destination: r.request_destination.as_ref().map(ToString::to_string),
        target: r.target.map(|t| t.to_string()),
        error: match &r.connect {
            Some(Err(kind)) => Some(format!("{kind:?}")),
            _ => None,
        },
        connect_ms: match &r.connect {
            Some(Ok(took)) => Some(took.as_millis() as u64),
            _ => None,
        },
    }
}

/// sail's routed connections, through every run. Behind (`Lagged`): those
/// missed are gone, and how many is logged.
async fn follow_routes(instance: Instance, tx: mpsc::Sender<Routed>) {
    let mut events = Box::pin(instance.events(embed::Kinds::ROUTE));
    while let Some(event) = events.next().await {
        match event {
            embed::Event::Routed(r) => {
                let _ = tx.try_send(routed(&r));
            }
            embed::Event::Lagged { kind, missed } if kind.contains(embed::Kinds::ROUTE) => {
                tracing::debug!(missed, "routed connections dropped: the reader fell behind");
            }
            _ => {}
        }
    }
}

fn dns_exchange(e: &embed::DnsExchange) -> DnsExchange {
    use embed::{DnsOutcome as O, DnsSource as S};
    let (rcode, rcode_name, error) = match &e.outcome {
        O::Answered { rcode, rcode_code } => (Some(*rcode_code), Some(rcode.clone()), None),
        O::Failed { error } => (None, None, Some(error.clone())),
        _ => (None, None, Some("unknown outcome".into())),
    };
    DnsExchange {
        name: e.name.clone(),
        qtype: e.qtype.clone(),
        qtype_code: e.qtype_code,
        server: e.server.clone(),
        source: match e.source {
            S::Exchanged => "exchanged",
            S::Cached => "cached",
            S::Optimistic => "optimistic",
            S::Rule => "rule",
            _ => "other",
        }
        .to_owned(),
        attempt: e.attempt,
        rcode,
        rcode_name,
        error,
        answers: e.answers.clone(),
        answers_total: e.answers_total,
        ttl: e.ttl,
        duration_ms: e.duration.map(|d| d.as_millis() as u64),
        for_instance: e.for_instance,
    }
}

/// sail's DNS exchanges, through every run. Behind (`Lagged`): those
/// missed are gone, and how many is logged.
async fn follow_dns(instance: Instance, tx: mpsc::Sender<DnsExchange>) {
    let mut events = Box::pin(instance.events(embed::Kinds::DNS));
    while let Some(event) = events.next().await {
        match event {
            embed::Event::DnsExchange(e) => {
                let _ = tx.try_send(dns_exchange(&e));
            }
            embed::Event::Lagged { kind, missed } if kind.contains(embed::Kinds::DNS) => {
                tracing::debug!(missed, "dns exchanges dropped: the reader fell behind");
            }
            _ => {}
        }
    }
}

/// sail's kind of what is left, as ours: those we have no kind for are the
/// runtime's.
fn left_kind(kind: embed::LeftKind) -> LeftoverKind {
    use embed::LeftKind as K;
    match kind {
        K::Wfp => LeftoverKind::Wfp,
        K::Tun => LeftoverKind::Tun,
        K::Route => LeftoverKind::Route,
        K::Dns => LeftoverKind::Dns,
        K::Rule | K::Nft => LeftoverKind::Rule,
        K::Task => LeftoverKind::Task,
        _ => LeftoverKind::Runtime,
    }
}

fn dial_stage(stage: embed::DialStage) -> &'static str {
    match stage {
        embed::DialStage::Dial => "dial",
        embed::DialStage::Handshake => "handshake",
        embed::DialStage::Transfer => "transfer",
        _ => "other",
    }
}

/// sail's failed connections, through every run. Those missed while
/// behind are gone (sail's next event for a chain counts from its last).
async fn follow_dials(instance: Instance, tx: mpsc::Sender<DialFailed>) {
    let mut events = Box::pin(instance.events(embed::Kinds::DIAL));
    while let Some(event) = events.next().await {
        if let embed::Event::DialFailed { failure, count } = event {
            let _ = tx.try_send(DialFailed {
                chain: failure.chain,
                destination: failure.destination,
                stage: dial_stage(failure.stage).into(),
                error: format!("{:?}", failure.kind),
                count,
                more_to_try: failure.more_to_try,
            });
        }
    }
}

/// sail's kind of what someone else changed, by its name in lower case
/// (sail's `LeftKind` has none of its own).
fn system_kind(kind: embed::LeftKind) -> &'static str {
    use embed::LeftKind as K;
    match kind {
        K::Tun => "tun",
        K::Route => "route",
        K::Rule => "rule",
        K::Dns => "dns",
        K::Nft => "nft",
        K::Wfp => "wfp",
        K::File => "file",
        K::Task => "task",
        _ => "other",
    }
}

/// sail's changes of what it set up for a TUN, made by another program,
/// through every run. Behind (`Lagged`): those missed are gone, and how
/// many is logged.
async fn follow_system(instance: Instance, tx: mpsc::Sender<SystemChange>) {
    let mut events = Box::pin(instance.events(embed::Kinds::SYSTEM));
    while let Some(event) = events.next().await {
        match event {
            embed::Event::SystemChanged { kind, resource } => {
                let _ = tx.try_send(SystemChange {
                    kind: system_kind(kind).into(),
                    resource,
                });
            }
            embed::Event::Lagged { kind, missed } if kind.contains(embed::Kinds::SYSTEM) => {
                tracing::warn!(missed, "system changes dropped: the reader fell behind");
            }
            _ => {}
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
        // A new run's groups start over.
        self.members.lock().expect("members").clear();
        self.instance
            .start(Config::Json(config.into()))
            .await
            .map_err(error)
    }

    /// Each reload rechecks the connections open against the new rules and
    /// closes those they reject or drop (embed.md, Rechecking the
    /// connections open).
    async fn reload(&self, config: &str) -> Result<ReloadReport, RuntimeError> {
        use embed::{InboundChange as C, ReloadPath as P};
        let options = embed::ReloadOptions::new().recheck_open(embed::RecheckOpen::CloseRejected);
        let report = self
            .instance
            .reload_rechecking(Some(Config::Json(config.into())), options)
            .await
            .map_err(error)?;
        let recheck_closed = report
            .recheck
            .as_ref()
            .map(|r| r.closed.iter().map(|c| (c.id, c.rule)).collect())
            .unwrap_or_default();
        Ok(ReloadReport {
            recheck_closed,
            path: match report.path {
                P::Full => "full",
                P::InboundsOnly => "inbounds_only",
                _ => "other",
            }
            .to_owned(),
            notes: report.notes.iter().map(ToString::to_string).collect(),
            inbounds: report
                .inbounds
                .into_iter()
                .map(|(tag, change)| {
                    let change = match change {
                        C::Untouched => "untouched",
                        C::Reloaded => "reloaded",
                        C::Added => "added",
                        C::Removed => "removed",
                        C::Replaced => "replaced",
                        _ => "other",
                    };
                    (tag, change.to_owned())
                })
                .collect(),
        })
    }

    /// A stop that ended with something left (tasks still running, what it
    /// changed in the system and could not undo) is a stop: sail's report
    /// of it is `stop_leftovers`, and a warn line. sail stops an instance
    /// that no longer runs (failed, stopped) at once, telling again what
    /// its run's end left.
    async fn stop(&self) -> Result<(), RuntimeError> {
        match self.instance.stop().await {
            Ok(()) => Ok(()),
            Err(_) if !self.stop_leftovers().is_empty() => {
                let left: Vec<String> = self
                    .stop_leftovers()
                    .iter()
                    .map(|l| format!("{}: {}", l.name, l.detail))
                    .collect();
                tracing::warn!(?left, "sail stopped with something left");
                Ok(())
            }
            Err(e) => Err(error(e)),
        }
    }

    fn stop_leftovers(&self) -> Vec<Leftover> {
        let Some(report) = self.instance.stop_report() else {
            return Vec::new();
        };
        let waited = report.waited.as_millis();
        let tasks = report.tasks.iter().map(|(task, n)| {
            Leftover::new(
                LeftoverKind::Task,
                *task,
                format!("{n} still running after {waited} ms"),
            )
        });
        let system = report.left.iter().map(|left| {
            let detail = match &left.clear {
                Some(clear) => format!("{}; clear it with `{clear}`", left.why),
                None => left.why.clone(),
            };
            Leftover::new(left_kind(left.kind), left.resource.clone(), detail)
        });
        tasks.chain(system).collect()
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

    /// sail tells a fallback's pin as a switch, not a selector's choice:
    /// that one is told here.
    async fn select(&self, group: &str, member: &str) -> Result<(), RuntimeError> {
        let before = self.instance.outbound(group).await.map_err(error)?;
        self.instance.select(group, member).await.map_err(error)?;
        let Some(info) = before.and_then(|o| o.group) else {
            return Ok(());
        };
        if info.selectable && info.selected != member {
            self.members
                .lock()
                .expect("members")
                .insert(group.to_owned(), member.to_owned());
            let _ = self.switch_tx.try_send(GroupSwitch {
                group: group.to_owned(),
                from: info.selected,
                to: member.to_owned(),
                reason: "selected".into(),
            });
        }
        Ok(())
    }

    async fn unfix(&self, group: &str) -> Result<(), RuntimeError> {
        self.instance.unfix(group).await.map_err(error)
    }

    async fn check_group(&self, group: &str, within: Duration) -> Result<(), RuntimeError> {
        // A fallback group runs its own check (sail's url_test_members).
        self.instance
            .url_test_members(group, None, within)
            .await
            .map(drop)
            .map_err(error)
    }

    fn group_switches(&self) -> mpsc::Receiver<GroupSwitch> {
        self.switches
            .lock()
            .expect("switches")
            .take()
            .expect("group_switches is taken once")
    }

    fn routes(&self) -> mpsc::Receiver<Routed> {
        let (tx, rx) = mpsc::channel(ROUTE_BUFFER);
        let mut task = self.route_task.lock().expect("route task");
        assert!(task.is_none(), "routes is taken once");
        *task = Some(tokio::spawn(follow_routes(self.instance.clone(), tx)));
        rx
    }

    fn dns_exchanges(&self) -> mpsc::Receiver<DnsExchange> {
        let (tx, rx) = mpsc::channel(DNS_BUFFER);
        let mut task = self.dns_task.lock().expect("dns task");
        assert!(task.is_none(), "dns_exchanges is taken once");
        *task = Some(tokio::spawn(follow_dns(self.instance.clone(), tx)));
        rx
    }

    fn dial_failures(&self) -> mpsc::Receiver<DialFailed> {
        self.failures
            .lock()
            .expect("failures")
            .take()
            .expect("dial_failures is taken once")
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
                // Clash's order (members first), turned outermost first as
                // Routed's and DialFailed's.
                chain: c.chains.into_iter().rev().collect(),
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

    /// As sail settled it at start: configured (Linux ppvpn0, Windows
    /// PPVPN) or chosen (macOS, the utun after the highest one).
    fn tun_name(&self) -> Option<String> {
        let names = self.instance.tun_names().ok()?;
        names.into_values().next().map(|tun| tun.name)
    }

    fn network(&self) -> Option<NetworkSnapshot> {
        self.instance.network().ok().map(|now| snapshot(&now))
    }

    fn network_changes(&self) -> watch::Receiver<Option<NetworkChange>> {
        self.network.subscribe()
    }

    fn system_changes(&self) -> mpsc::Receiver<SystemChange> {
        self.systems
            .lock()
            .expect("systems")
            .take()
            .expect("system_changes is taken once")
    }

    fn replaced_routes(&self) -> Vec<String> {
        self.instance.replaced_routes().unwrap_or_default()
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
