//! An in-memory [`Runtime`] for the Engine's tests: it runs nothing, keeps
//! the config it was given, records every call and fails on request. Its
//! groups are the config's selectors and fallbacks (selection kept across a
//! reload for the same tag, as sail does) unless a test sets them.

use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Mutex;

use super::*;

/// A call, as recorded.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Call {
    Start(String),
    Reload(String),
    Stop,
    Select(String, String),
    Unfix(String),
    CloseConnection(u64),
    DialTcp(String, Target),
    DialUdp(String, Target),
    ReplaceInboundUsers(String, Vec<(String, String)>),
    NetworkChanged,
    AddInbound(String),
    RemoveInbound(String),
}

/// Which call the next failure is for.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum Op {
    Start,
    Reload,
    Stop,
    Select,
    Dial,
    ReplaceInboundUsers,
    AddInbound,
    RemoveInbound,
}

const LOG_CAPACITY: usize = 64;

pub(crate) struct FakeRuntime {
    calls: Mutex<Vec<Call>>,
    config: Mutex<Option<String>>,
    failures: Mutex<HashMap<Op, RuntimeError>>,
    groups: Mutex<Vec<GroupInfo>>,
    /// Set by `set_groups`: the config no longer decides them.
    explicit_groups: AtomicBool,
    connections: Mutex<Vec<RuntimeConnection>>,
    /// The inbounds' tags: the configuration's at start, then as
    /// add_inbound and remove_inbound change them; none after stop. A
    /// reload leaves them, as sail's adds and removes no listener.
    inbounds: Mutex<Vec<String>>,
    traffic: Mutex<RuntimeTraffic>,
    /// How often `traffic` was read.
    traffic_reads: AtomicU64,
    tcp_route: Mutex<Option<SocketAddr>>,
    state: watch::Sender<RuntimeState>,
    switches: (
        mpsc::Sender<GroupSwitch>,
        Mutex<Option<mpsc::Receiver<GroupSwitch>>>,
    ),
    logs: (mpsc::Sender<String>, Mutex<Option<mpsc::Receiver<String>>>),
    failures_seen: (
        mpsc::Sender<DialFailed>,
        Mutex<Option<mpsc::Receiver<DialFailed>>>,
    ),
    dropped: AtomicU64,
    network: Mutex<NetworkSnapshot>,
    network_changes: watch::Sender<Option<NetworkChange>>,
    generation: AtomicU64,
}

impl Default for FakeRuntime {
    fn default() -> Self {
        let (switch_tx, switch_rx) = mpsc::channel(64);
        let (log_tx, log_rx) = mpsc::channel(LOG_CAPACITY);
        let (dial_tx, dial_rx) = mpsc::channel(64);
        Self {
            calls: Mutex::default(),
            config: Mutex::default(),
            failures: Mutex::default(),
            groups: Mutex::default(),
            explicit_groups: AtomicBool::new(false),
            connections: Mutex::default(),
            inbounds: Mutex::default(),
            traffic: Mutex::default(),
            traffic_reads: AtomicU64::new(0),
            tcp_route: Mutex::default(),
            state: watch::channel(RuntimeState::Idle).0,
            switches: (switch_tx, Mutex::new(Some(switch_rx))),
            logs: (log_tx, Mutex::new(Some(log_rx))),
            failures_seen: (dial_tx, Mutex::new(Some(dial_rx))),
            dropped: AtomicU64::new(0),
            // Known at the start, as sail's start returns with its first
            // detection done; `set_network` makes it unknown for a test.
            network: Mutex::new(NetworkSnapshot {
                interface: Some("eth0".into()),
                index: Some(2),
                ..NetworkSnapshot::default()
            }),
            network_changes: watch::channel(None).0,
            generation: AtomicU64::new(0),
        }
    }
}

impl FakeRuntime {
    /// The inbounds now, by tag.
    pub(crate) fn inbounds(&self) -> Vec<String> {
        self.inbounds.lock().unwrap().clone()
    }

    fn take_inbounds_of(&self, config: &str) {
        let config: serde_json::Value = serde_json::from_str(config).unwrap_or_default();
        let tags = config["inbounds"]
            .as_array()
            .map(|list| list.iter().filter_map(inbound_tag).collect())
            .unwrap_or_default();
        *self.inbounds.lock().unwrap() = tags;
    }

    pub(crate) fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }

    /// The config it runs (the last start or successful reload).
    pub(crate) fn config(&self) -> Option<String> {
        self.config.lock().unwrap().clone()
    }

    /// The next `op` fails with `error` (once).
    pub(crate) fn fail_next(&self, op: Op, error: RuntimeError) {
        self.failures.lock().unwrap().insert(op, error);
    }

    pub(crate) fn set_groups(&self, groups: Vec<GroupInfo>) {
        self.explicit_groups.store(true, Ordering::Relaxed);
        *self.groups.lock().unwrap() = groups;
    }

    /// The groups of `config` as sail would run them: a selector on its
    /// default, a fallback on its first member, never checked.
    fn take_groups_of(&self, config: &str) {
        if self.explicit_groups.load(Ordering::Relaxed) {
            return;
        }
        let Ok(value) = serde_json::from_str::<serde_json::Value>(config) else {
            return;
        };
        let mut groups = self.groups.lock().unwrap();
        let previous = std::mem::take(&mut *groups);
        for outbound in value["outbounds"].as_array().into_iter().flatten() {
            let kind = outbound["type"].as_str().unwrap_or("");
            if kind != "selector" && kind != "fallback" {
                continue;
            }
            let tag = outbound["tag"].as_str().unwrap_or("").to_owned();
            let members: Vec<String> = outbound["outbounds"]
                .as_array()
                .into_iter()
                .flatten()
                .filter_map(|m| m.as_str().map(str::to_owned))
                .collect();
            let kept = previous
                .iter()
                .find(|g| g.tag == tag && members.contains(&g.now));
            let now = match (kept, outbound["default"].as_str()) {
                (Some(g), _) => g.now.clone(),
                (None, Some(default)) => default.to_owned(),
                (None, None) => members.first().cloned().unwrap_or_default(),
            };
            groups.push(GroupInfo {
                tag,
                now,
                members: members
                    .into_iter()
                    .map(|tag| MemberInfo {
                        tag,
                        alive: None,
                        last_check: None,
                        consecutive_failures: 0,
                    })
                    .collect(),
                fixed: kept.is_some_and(|g| g.fixed),
            });
        }
    }

    pub(crate) fn set_connections(&self, connections: Vec<RuntimeConnection>) {
        *self.connections.lock().unwrap() = connections;
    }

    /// How often the runtime's traffic was read.
    pub(crate) fn traffic_reads(&self) -> u64 {
        self.traffic_reads.load(Ordering::Relaxed)
    }

    pub(crate) fn set_traffic(&self, traffic: RuntimeTraffic) {
        *self.traffic.lock().unwrap() = traffic;
    }

    /// TCP dials from now on connect to `to` (a test's local listener),
    /// whatever their target; without it they get a stream that is closed.
    pub(crate) fn route_tcp_to(&self, to: SocketAddr) {
        *self.tcp_route.lock().unwrap() = Some(to);
    }

    /// As if sail moved on its own (a crash, a panic).
    pub(crate) fn set_state(&self, state: RuntimeState) {
        self.state.send_replace(state);
    }

    /// As if failover moved `group` to `to`.
    pub(crate) fn switch(&self, group: &str, to: &str, reason: &str) {
        let mut groups = self.groups.lock().unwrap();
        let group = groups
            .iter_mut()
            .find(|g| g.tag == group)
            .expect("no such group");
        let from = std::mem::replace(&mut group.now, to.to_owned());
        let _ = self.switches.0.try_send(GroupSwitch {
            group: group.tag.clone(),
            from,
            to: to.to_owned(),
            reason: reason.to_owned(),
        });
    }

    /// As if connections through `chain` failed.
    #[allow(dead_code)] // for the Engine's tests
    pub(crate) fn dial_failed(&self, failed: DialFailed) {
        let _ = self.failures_seen.0.try_send(failed);
    }

    /// A log line from sail; dropped and counted when the reader is behind.
    /// The network becomes `new`, as sail would publish it (`reason`:
    /// default_interface, state, host or wake).
    /// The network becomes `new` without a change being reported (sail
    /// learning its first default interface).
    pub(crate) fn set_network(&self, new: NetworkSnapshot) {
        *self.network.lock().unwrap() = new;
    }

    pub(crate) fn change_network(&self, new: NetworkSnapshot, reason: &str) {
        let old = std::mem::replace(&mut *self.network.lock().unwrap(), new.clone());
        let generation = self.generation.fetch_add(1, Ordering::Relaxed) + 1;
        let change = made_up_kind(&old, &new).into();
        self.network_changes.send_replace(Some(NetworkChange {
            generation,
            change,
            reason: reason.into(),
            old,
            new,
        }));
    }

    pub(crate) fn log(&self, line: &str) {
        if self.logs.0.try_send(line.to_owned()).is_err() {
            self.dropped.fetch_add(1, Ordering::Relaxed);
        }
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn check(&self, op: Op) -> Result<(), RuntimeError> {
        match self.failures.lock().unwrap().remove(&op) {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    fn running(&self) -> Result<(), RuntimeError> {
        if *self.state.borrow() == RuntimeState::Running {
            Ok(())
        } else {
            Err(RuntimeError::new("not_running", "not running"))
        }
    }
}

#[async_trait]
impl Runtime for FakeRuntime {
    async fn start(&self, config: &str) -> Result<(), RuntimeError> {
        self.record(Call::Start(config.to_owned()));
        if *self.state.borrow() == RuntimeState::Running {
            return Err(RuntimeError::new("state", "already running"));
        }
        self.state.send_replace(RuntimeState::Starting);
        if let Err(error) = self.check(Op::Start) {
            self.state.send_replace(RuntimeState::Failed {
                code: error.code.clone(),
                message: error.message.clone(),
            });
            return Err(error);
        }
        *self.config.lock().unwrap() = Some(config.to_owned());
        self.take_groups_of(config);
        self.take_inbounds_of(config);
        self.state.send_replace(RuntimeState::Running);
        Ok(())
    }

    async fn reload(&self, config: &str) -> Result<(), RuntimeError> {
        self.record(Call::Reload(config.to_owned()));
        self.running()?;
        self.check(Op::Reload)?;
        *self.config.lock().unwrap() = Some(config.to_owned());
        self.take_groups_of(config);
        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        self.record(Call::Stop);
        self.check(Op::Stop)?;
        self.state.send_replace(RuntimeState::Stopping);
        self.connections.lock().unwrap().clear();
        self.state.send_replace(RuntimeState::Stopped);
        self.inbounds.lock().unwrap().clear();
        Ok(())
    }

    fn states(&self) -> watch::Receiver<RuntimeState> {
        self.state.subscribe()
    }

    fn state(&self) -> RuntimeState {
        self.state.borrow().clone()
    }

    async fn groups(&self) -> Result<Vec<GroupInfo>, RuntimeError> {
        self.running()?;
        Ok(self.groups.lock().unwrap().clone())
    }

    async fn select(&self, group: &str, member: &str) -> Result<(), RuntimeError> {
        self.record(Call::Select(group.to_owned(), member.to_owned()));
        self.running()?;
        self.check(Op::Select)?;
        let mut groups = self.groups.lock().unwrap();
        let group = groups
            .iter_mut()
            .find(|g| g.tag == group)
            .ok_or_else(|| RuntimeError::new("not_found", format!("group {group}")))?;
        if !group.members.iter().any(|m| m.tag == member) {
            return Err(RuntimeError::new("not_found", format!("member {member}")));
        }
        group.now = member.to_owned();
        group.fixed = true;
        Ok(())
    }

    async fn unfix(&self, group: &str) -> Result<(), RuntimeError> {
        self.record(Call::Unfix(group.to_owned()));
        self.running()?;
        let mut groups = self.groups.lock().unwrap();
        let group = groups
            .iter_mut()
            .find(|g| g.tag == group)
            .ok_or_else(|| RuntimeError::new("not_found", format!("group {group}")))?;
        group.fixed = false;
        Ok(())
    }

    fn group_switches(&self) -> mpsc::Receiver<GroupSwitch> {
        self.switches.1.lock().unwrap().take().expect("taken once")
    }

    fn dial_failures(&self) -> mpsc::Receiver<DialFailed> {
        self.failures_seen
            .1
            .lock()
            .unwrap()
            .take()
            .expect("taken once")
    }

    async fn traffic(&self) -> Result<RuntimeTraffic, RuntimeError> {
        self.traffic_reads.fetch_add(1, Ordering::Relaxed);
        self.running()?;
        Ok(*self.traffic.lock().unwrap())
    }

    async fn connections(&self) -> Result<Vec<RuntimeConnection>, RuntimeError> {
        self.running()?;
        Ok(self.connections.lock().unwrap().clone())
    }

    async fn close_connection(&self, id: u64) -> Result<bool, RuntimeError> {
        self.record(Call::CloseConnection(id));
        self.running()?;
        let mut connections = self.connections.lock().unwrap();
        let before = connections.len();
        connections.retain(|c| c.id != id);
        Ok(connections.len() != before)
    }

    async fn dial_tcp(
        &self,
        outbound: &str,
        to: Target,
        _timeout: Duration,
    ) -> Result<Box<dyn AsyncReadWrite>, RuntimeError> {
        self.record(Call::DialTcp(outbound.to_owned(), to));
        self.running()?;
        self.check(Op::Dial)?;
        let route = *self.tcp_route.lock().unwrap();
        if let Some(route) = route {
            let stream = tokio::net::TcpStream::connect(route)
                .await
                .map_err(|e| RuntimeError::new("failed", e.to_string()))?;
            return Ok(Box::new(stream));
        }
        let (near, _far) = tokio::io::duplex(64);
        Ok(Box::new(near))
    }

    async fn dial_udp(
        &self,
        outbound: &str,
        to: Target,
        _timeout: Duration,
    ) -> Result<Box<dyn Datagram>, RuntimeError> {
        self.record(Call::DialUdp(outbound.to_owned(), to));
        self.running()?;
        self.check(Op::Dial)?;
        Ok(Box::new(Echo::default()))
    }

    async fn replace_inbound_users(
        &self,
        inbound: &str,
        users: Vec<(String, String)>,
    ) -> Result<(), RuntimeError> {
        self.record(Call::ReplaceInboundUsers(inbound.to_owned(), users));
        self.running()?;
        self.check(Op::ReplaceInboundUsers)
    }

    async fn network_changed(&self) -> Result<(), RuntimeError> {
        self.record(Call::NetworkChanged);
        Ok(())
    }

    fn logs(&self) -> mpsc::Receiver<String> {
        self.logs.1.lock().unwrap().take().expect("taken once")
    }

    fn dropped_log_lines(&self) -> u64 {
        self.dropped.load(Ordering::Relaxed)
    }

    fn network(&self) -> Option<NetworkSnapshot> {
        (*self.state.borrow() == RuntimeState::Running)
            .then(|| self.network.lock().unwrap().clone())
    }

    fn network_changes(&self) -> watch::Receiver<Option<NetworkChange>> {
        self.network_changes.subscribe()
    }

    fn tun_name(&self) -> Option<String> {
        if *self.state.borrow() != RuntimeState::Running {
            return None;
        }
        super::configured_tun_name(self.config.lock().unwrap().as_deref()?)
    }

    async fn add_inbound(&self, inbound: &str) -> Result<(), RuntimeError> {
        let value: serde_json::Value = serde_json::from_str(inbound)
            .map_err(|e| RuntimeError::new("config", format!("inbound: {e}")))?;
        let tag =
            inbound_tag(&value).ok_or_else(|| RuntimeError::new("config", "inbound: no type"))?;
        self.record(Call::AddInbound(tag.clone()));
        self.running()?;
        self.check(Op::AddInbound)?;
        let mut inbounds = self.inbounds.lock().unwrap();
        if inbounds.contains(&tag) {
            return Err(RuntimeError::new(
                "config",
                format!("[{tag}] inbound: exists"),
            ));
        }
        inbounds.push(tag);
        Ok(())
    }

    async fn remove_inbound(&self, tag: &str) -> Result<(), RuntimeError> {
        self.record(Call::RemoveInbound(tag.into()));
        self.running()?;
        self.check(Op::RemoveInbound)?;
        let mut inbounds = self.inbounds.lock().unwrap();
        let before = inbounds.len();
        inbounds.retain(|t| t != tag);
        if inbounds.len() == before {
            return Err(RuntimeError::new(
                "not_found",
                format!("[{tag}] inbound: does not exist"),
            ));
        }
        self.connections
            .lock()
            .unwrap()
            .retain(|c| c.inbound != tag);
        Ok(())
    }
}

/// A datagram association that answers what it is sent.
#[derive(Default)]
struct Echo {
    last: Mutex<Vec<u8>>,
}

#[async_trait]
impl Datagram for Echo {
    async fn send(&self, data: &[u8]) -> std::io::Result<()> {
        *self.last.lock().unwrap() = data.to_vec();
        Ok(())
    }

    async fn recv(&self, buf: &mut [u8]) -> std::io::Result<usize> {
        let last = self.last.lock().unwrap();
        let n = last.len().min(buf.len());
        buf[..n].copy_from_slice(&last[..n]);
        Ok(n)
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;

    #[tokio::test]
    async fn inbounds_follow_the_config_and_the_calls() {
        let fake = FakeRuntime::default();
        fake.start(r#"{"inbounds":[{"type":"mixed","tag":"local"},{"type":"tun"}]}"#)
            .await
            .unwrap();
        assert_eq!(fake.inbounds(), ["local", "tun"]);
        fake.add_inbound(r#"{"type":"mixed","tag":"system"}"#)
            .await
            .unwrap();
        assert!(fake
            .add_inbound(r#"{"type":"mixed","tag":"system"}"#)
            .await
            .is_err());
        fake.remove_inbound("system").await.unwrap();
        assert_eq!(
            fake.remove_inbound("system").await.unwrap_err().code,
            "not_found"
        );
        assert_eq!(fake.inbounds(), ["local", "tun"]);
        assert!(fake.calls().contains(&Call::AddInbound("system".into())));
    }

    #[tokio::test]
    async fn network_changes_are_injected() {
        let fake = FakeRuntime::default();
        let mut changes = fake.network_changes();
        assert_eq!(fake.network(), None, "not running");
        fake.start("{}").await.unwrap();
        let wifi = NetworkSnapshot {
            interface: Some("en0".into()),
            index: Some(6),
            ..Default::default()
        };
        fake.change_network(wifi.clone(), "default_interface");
        changes.changed().await.unwrap();
        let change = changes.borrow_and_update().clone().unwrap();
        assert_eq!(
            (change.generation, change.reason.as_str(), &change.new),
            (1, "default_interface", &wifi)
        );
        assert_eq!(fake.network(), Some(wifi.clone()));
        fake.change_network(
            NetworkSnapshot {
                offline: true,
                ..Default::default()
            },
            "state",
        );
        let change = changes.borrow_and_update().clone().unwrap();
        assert_eq!((change.generation, change.old), (2, wifi));
    }

    fn group(tag: &str, members: &[&str]) -> GroupInfo {
        GroupInfo {
            tag: tag.into(),
            now: members[0].into(),
            members: members
                .iter()
                .map(|m| MemberInfo {
                    tag: (*m).into(),
                    alive: None,
                    last_check: None,
                    consecutive_failures: 0,
                })
                .collect(),
            fixed: false,
        }
    }

    #[tokio::test]
    async fn a_failed_reload_changes_nothing() {
        let runtime = FakeRuntime::default();
        runtime.start("a").await.unwrap();
        runtime.fail_next(Op::Reload, RuntimeError::new("config", "bad"));
        let error = runtime.reload("b").await.unwrap_err();
        assert_eq!(error.code, "config");
        assert_eq!(runtime.config().as_deref(), Some("a"));
        assert_eq!(runtime.state(), RuntimeState::Running);
        runtime.reload("c").await.unwrap();
        assert_eq!(runtime.config().as_deref(), Some("c"));
    }

    #[tokio::test]
    async fn states_are_seen_after_subscribing() {
        let runtime = FakeRuntime::default();
        let mut states = runtime.states();
        assert_eq!(*states.borrow_and_update(), RuntimeState::Idle);
        runtime.fail_next(Op::Start, RuntimeError::new("config", "bad"));
        runtime.start("a").await.unwrap_err();
        states.changed().await.unwrap();
        assert!(matches!(
            &*states.borrow_and_update(),
            RuntimeState::Failed { code, .. } if code == "config"
        ));
    }

    #[tokio::test]
    async fn select_fixes_and_switches_are_reported() {
        let runtime: Arc<dyn Runtime> = Arc::new(FakeRuntime::default());
        let fake = FakeRuntime::default();
        fake.set_groups(vec![group("jp", &["jp-1", "jp-2"])]);
        fake.start("a").await.unwrap();
        let mut switches = fake.group_switches();
        fake.select("jp", "jp-2").await.unwrap();
        let groups = fake.groups().await.unwrap();
        assert_eq!((groups[0].now.as_str(), groups[0].fixed), ("jp-2", true));
        assert_eq!(
            fake.select("jp", "jp-9").await.unwrap_err().code,
            "not_found"
        );
        fake.unfix("jp").await.unwrap();
        fake.switch("jp", "jp-1", "down");
        let switch = switches.recv().await.unwrap();
        assert_eq!((switch.from.as_str(), switch.to.as_str()), ("jp-2", "jp-1"));
        assert_eq!(
            runtime.groups().await.unwrap_err().code,
            "not_running",
            "a runtime is usable as a trait object"
        );
    }

    #[tokio::test]
    async fn groups_follow_the_config_and_keep_their_selection() {
        let runtime = FakeRuntime::default();
        let config = |default: &str| {
            serde_json::json!({ "outbounds": [
                { "type": "direct", "tag": "a" },
                { "type": "direct", "tag": "b" },
                { "type": "selector", "tag": "pick", "outbounds": ["a", "b"], "default": default },
                { "type": "fallback", "tag": "auto", "outbounds": ["b", "a"] },
            ]})
            .to_string()
        };
        runtime.start(&config("a")).await.unwrap();
        let groups = runtime.groups().await.unwrap();
        assert_eq!(
            groups
                .iter()
                .map(|g| (g.tag.as_str(), g.now.as_str()))
                .collect::<Vec<_>>(),
            [("pick", "a"), ("auto", "b")]
        );
        runtime.select("pick", "b").await.unwrap();
        runtime.reload(&config("a")).await.unwrap();
        let groups = runtime.groups().await.unwrap();
        assert_eq!((groups[0].now.as_str(), groups[0].fixed), ("b", true));
    }

    #[tokio::test]
    async fn logs_never_wait() {
        let runtime = FakeRuntime::default();
        let mut logs = runtime.logs();
        for i in 0..LOG_CAPACITY + 3 {
            runtime.log(&format!("line {i}"));
        }
        assert_eq!(runtime.dropped_log_lines(), 3);
        assert_eq!(logs.recv().await.unwrap(), "line 0");
    }
}
