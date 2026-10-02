//! An in-memory [`Runtime`] for the Engine's tests: it runs nothing, keeps
//! the config it was given, records every call and fails on request.

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
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
}

const LOG_CAPACITY: usize = 64;

pub(crate) struct FakeRuntime {
    calls: Mutex<Vec<Call>>,
    config: Mutex<Option<String>>,
    failures: Mutex<HashMap<Op, RuntimeError>>,
    groups: Mutex<Vec<GroupInfo>>,
    connections: Mutex<Vec<RuntimeConnection>>,
    traffic: Mutex<RuntimeTraffic>,
    state: watch::Sender<RuntimeState>,
    switches: (
        mpsc::Sender<GroupSwitch>,
        Mutex<Option<mpsc::Receiver<GroupSwitch>>>,
    ),
    logs: (mpsc::Sender<String>, Mutex<Option<mpsc::Receiver<String>>>),
    dropped: AtomicU64,
}

impl Default for FakeRuntime {
    fn default() -> Self {
        let (switch_tx, switch_rx) = mpsc::channel(64);
        let (log_tx, log_rx) = mpsc::channel(LOG_CAPACITY);
        Self {
            calls: Mutex::default(),
            config: Mutex::default(),
            failures: Mutex::default(),
            groups: Mutex::default(),
            connections: Mutex::default(),
            traffic: Mutex::default(),
            state: watch::channel(RuntimeState::Idle).0,
            switches: (switch_tx, Mutex::new(Some(switch_rx))),
            logs: (log_tx, Mutex::new(Some(log_rx))),
            dropped: AtomicU64::new(0),
        }
    }
}

impl FakeRuntime {
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
        *self.groups.lock().unwrap() = groups;
    }

    pub(crate) fn set_connections(&self, connections: Vec<RuntimeConnection>) {
        *self.connections.lock().unwrap() = connections;
    }

    pub(crate) fn set_traffic(&self, traffic: RuntimeTraffic) {
        *self.traffic.lock().unwrap() = traffic;
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

    /// A log line from sail; dropped and counted when the reader is behind.
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
        self.state.send_replace(RuntimeState::Running);
        Ok(())
    }

    async fn reload(&self, config: &str) -> Result<(), RuntimeError> {
        self.record(Call::Reload(config.to_owned()));
        self.running()?;
        self.check(Op::Reload)?;
        *self.config.lock().unwrap() = Some(config.to_owned());
        Ok(())
    }

    async fn stop(&self) -> Result<(), RuntimeError> {
        self.record(Call::Stop);
        self.check(Op::Stop)?;
        self.state.send_replace(RuntimeState::Stopping);
        self.connections.lock().unwrap().clear();
        self.state.send_replace(RuntimeState::Stopped);
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

    async fn traffic(&self) -> Result<RuntimeTraffic, RuntimeError> {
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
