//! The TUN instance's translation inputs (docs/host-integration.md, section
//! 2) and the host's IPv6 state they follow (Go: runtime.hostIPv6State and
//! reprobeHostIPv6).
//!
//! The host's IPv6 is probed on every apply and start (an administrator can
//! toggle it between two starts), and again when the default interface
//! changes ([`Engine::reprobe_host_ipv6`]): a host that loses or gains its
//! IPv6 path switches kernels to the build that hands direct IPv6 its domain
//! (see `translate::Tun::no_host_ipv6_route`), or back.

use std::sync::{Arc, Mutex};

use super::{Engine, Error, Inner};
use crate::config::{EngineConfig, Platform, Role};
use crate::error::codes;
use crate::hostipv6;
use crate::localdns::{self, listener, Cache, Change, Hosts, Interface, Server};
use crate::runtime::NetworkSnapshot;
use crate::translate::{self, LocalDns, Tun};

/// What the host says about its IPv6.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct HostIpv6 {
    /// IPv6 is enabled ([`hostipv6::available`]).
    pub available: bool,
    /// It has an IPv6 path of its own ([`hostipv6::route`]); `Err` when that
    /// cannot be read.
    pub route: Result<bool, String>,
}

/// Reads the host's IPv6; tests replace it.
pub(super) type Probe = Arc<dyn Fn() -> HostIpv6 + Send + Sync>;

/// The host's IPv6 as a TUN build uses it.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub(super) struct Ipv6State {
    pub ipv6: bool,
    pub no_host_ipv6_route: bool,
}

/// What a TUN instance keeps besides `Live`.
pub(super) struct TunState {
    local_dns: LocalDns,
    probe: Mutex<Probe>,
    /// The last probe's result, which the next translation uses.
    host: Mutex<Ipv6State>,
    /// The core's own dns-local, serving sail on loopback (desktop TUN
    /// instances, from `Engine::new`): sail's dns-local server points here.
    listener: Mutex<Option<listener::Listener>>,
    /// The default interface as sail last reported it: what dns-local reads
    /// the resolvers of.
    interface: Arc<Mutex<Option<Interface>>>,
}

impl TunState {
    pub(super) fn new(config: &EngineConfig) -> Self {
        Self {
            // `Engine::new` refused one that does not parse.
            local_dns: local_dns(config).unwrap_or(LocalDns::System),
            probe: Mutex::new(Arc::new(system_probe)),
            host: Mutex::default(),
            listener: Mutex::new(None),
            interface: Arc::default(),
        }
    }
}

/// What `Engine::new` refuses in a TUN instance's configuration.
pub(super) fn check(config: &EngineConfig) -> Result<(), Error> {
    if config.role == Role::Tun {
        local_dns(config)?;
    }
    Ok(())
}

/// dns-local's servers: the host's override, else the system's.
fn local_dns(config: &EngineConfig) -> Result<LocalDns, Error> {
    let servers = config
        .tun
        .as_ref()
        .map(|tun| tun.local_dns_servers.as_slice())
        .unwrap_or_default();
    if servers.is_empty() {
        return Ok(LocalDns::System);
    }
    translate::local_dns_servers(servers)
        .map(LocalDns::Servers)
        .map_err(|e| Error::invalid(codes::CORE_OPERATION_FAILED, "tun.local_dns_servers", e))
}

/// Desktop TUNs own the default route (auto_route); mobile hosts build the
/// tunnel themselves, IPv4 only.
fn desktop(platform: Platform) -> bool {
    matches!(
        platform,
        Platform::Linux | Platform::Macos | Platform::Windows
    )
}

fn system_probe() -> HostIpv6 {
    let available = hostipv6::available();
    HostIpv6 {
        available,
        route: if available {
            hostipv6::route()
        } else {
            Ok(false)
        },
    }
}

/// Names what a build does with IPv6 (Go's hostIPv6Policy).
fn policy(state: Ipv6State) -> &'static str {
    match state {
        Ipv6State { ipv6: false, .. } => "tun_ipv4_only",
        Ipv6State {
            no_host_ipv6_route: true,
            ..
        } => "tun_ipv6_direct_ipv4",
        _ => "tun_ipv6",
    }
}

impl Engine {
    /// The default interface changed: probes the host's IPv6 path again and,
    /// when it differs from the running build's, reloads (a kernel switch,
    /// `KernelSwitched`). Nothing while offline: the path looks lost only
    /// because every path is; the change that brings the network back
    /// probes again. Its source is sail's network events (E1b), not wired
    /// yet.
    #[allow(dead_code)]
    pub(crate) async fn reprobe_host_ipv6(&self) {
        self.inner.reprobe_host_ipv6().await;
    }
}

/// dns-local's interface of `snapshot`: none offline.
fn interface_of(snapshot: &NetworkSnapshot) -> Option<Interface> {
    if snapshot.offline {
        return None;
    }
    snapshot.interface.as_ref().map(|name| Interface {
        index: snapshot.index.unwrap_or(0),
        name: name.clone(),
    })
}

/// The interface dns-local asks the resolvers of: the network the run last
/// reported, else, before it reported one (sail resolving its nodes during
/// its first start), sail's snapshot, readable from the Starting phase on.
fn local_dns_interface(
    current: &Mutex<Option<Interface>>,
    runtime: &dyn crate::runtime::Runtime,
) -> Option<Interface> {
    let known = current.lock().unwrap_or_else(|e| e.into_inner()).clone();
    known.or_else(|| runtime.network().as_ref().and_then(interface_of))
}

impl Inner {
    /// The TUN of a TUN instance (None otherwise), with the host's IPv6 as
    /// last probed.
    pub(super) fn tun_options(&self) -> Option<Tun> {
        if self.config.role != Role::Tun {
            return None;
        }
        let host = *self.tun.host.lock().expect("host ipv6");
        Some(Tun {
            desktop: desktop(self.config.platform),
            ipv6: host.ipv6,
            no_host_ipv6_route: host.no_host_ipv6_route,
            interface_name: translate::interface_name(self.config.platform).into(),
            local_dns: match self.tun.listener.lock().expect("dns-local").as_ref() {
                Some(listener) => LocalDns::Listener(listener.addr()),
                None => self.tun.local_dns.clone(),
            },
        })
    }

    /// Starts the core's own dns-local (Go: internal/localdns) on a desktop
    /// TUN instance: a loopback listener sail's dns-local server points to.
    /// It reads the default interface's resolvers (or the host's override)
    /// and asks them through sail's direct outbound, which binds the
    /// physical interface; sail's network events make it read them again.
    pub(super) async fn start_local_dns(&self) -> Result<(), Error> {
        if self.config.role != Role::Tun || !desktop(self.config.platform) {
            return Ok(());
        }
        let overridden: Vec<Server> = match &self.tun.local_dns {
            LocalDns::Servers(servers) => servers
                .iter()
                .map(|s| Server::new(s.ip(), s.port()))
                .collect(),
            _ => Vec::new(),
        };
        let current = self.tun.interface.clone();
        let runtime = self.runtime.clone();
        let started = std::time::Instant::now();
        let cache = Cache::new(
            move |iface: &Interface| {
                if overridden.is_empty() {
                    localdns::source::system(iface)
                } else {
                    localdns::source::overridden(&overridden)
                }
            },
            move || local_dns_interface(&current, runtime.as_ref()),
            localdns::servers::tunnel_prefixes(),
            move || started.elapsed(),
            |change: &Change| {
                tracing::info!(
                    interface = change.interface.as_str(),
                    source = change.source.as_str(),
                    servers = change.servers_field(),
                    error = change.error.as_deref(),
                    "local dns servers"
                );
            },
        );
        let dns = localdns::LocalDns::new(
            Arc::new(cache),
            Arc::new(Hosts::system()),
            Arc::new(localdns::RuntimeDial {
                runtime: self.runtime.clone(),
                outbound: translate::DIRECT_TAG.into(),
            }),
        );
        let listener = listener::start(Arc::new(dns)).await.map_err(|e| {
            Error::new(
                codes::CORE_OPERATION_FAILED,
                false,
                format!("dns-local: {e}"),
            )
        })?;
        tracing::info!(addr = %listener.addr(), "dns-local listening");
        *self.tun.listener.lock().expect("dns-local") = Some(listener);
        Ok(())
    }

    /// sail reported the network: dns-local follows the default interface
    /// and reads its resolvers again at the next query.
    /// Before the runtime stops (stop, restart, shutdown): dns-local's
    /// queries under way end, so that sail's stop waits for none of their
    /// dials (#221).
    pub(super) fn cancel_dns_queries(&self) {
        if let Some(listener) = self.tun.listener.lock().expect("dns-local").as_ref() {
            listener.cancel_queries();
        }
    }

    pub(super) fn local_dns_network(&self, snapshot: &NetworkSnapshot) {
        let interface = interface_of(snapshot);
        *self.tun.interface.lock().unwrap_or_else(|e| e.into_inner()) = interface;
        if let Some(listener) = self.tun.listener.lock().expect("dns-local").as_ref() {
            listener.invalidate();
        }
    }

    /// Probes the host's IPv6 for the next translation (apply and start).
    pub(super) fn probe_host_ipv6(&self) {
        if self.config.role != Role::Tun {
            return;
        }
        let state = self.read_host_ipv6();
        *self.tun.host.lock().expect("host ipv6") = state;
    }

    /// The host's IPv6 now, logged. Mobile: an IPv4-only tunnel, nothing to
    /// probe. A route that cannot be read keeps IPv6 as before (no
    /// hand-off).
    fn read_host_ipv6(&self) -> Ipv6State {
        if !desktop(self.config.platform) {
            return Ipv6State::default();
        }
        let probe = self.tun.probe.lock().expect("host ipv6 probe").clone();
        let host = probe();
        let route = host.route.clone().unwrap_or(true);
        let state = Ipv6State {
            ipv6: host.available,
            no_host_ipv6_route: host.available && !route,
        };
        tracing::info!(
            host_ipv6_enabled = host.available,
            host_ipv6_route = route,
            policy = policy(state),
            error = host.route.as_ref().err().map(String::as_str),
            "host ipv6"
        );
        state
    }

    pub(super) async fn reprobe_host_ipv6(self: &Arc<Self>) {
        if self.config.role != Role::Tun || !desktop(self.config.platform) {
            return;
        }
        if self.admit().is_err() {
            return;
        }
        // Like a lifecycle call: an apply that ran in between built for the
        // state now, and then nothing changes here.
        let _op = self.op.lock().await;
        let (profile, mode, selected, pins, handed_off) = {
            let live = self.live();
            if live.offline {
                tracing::debug!(
                    reason = "no default interface",
                    "host ipv6 re-probe skipped"
                );
                return;
            }
            if !live.running || live.shut_down || live.fatal.is_some() {
                return;
            }
            let Some(a) = &live.applied else {
                return;
            };
            (
                a.profile.clone(),
                a.mode,
                a.selected.clone(),
                a.pins.clone(),
                a.translation.direct_ipv6_hand_off,
            )
        };
        let state = self.read_host_ipv6();
        if !state.ipv6 || state.no_host_ipv6_route == handed_off {
            // Unchanged; a host that disabled IPv6 changes the TUN itself and
            // is left to the next apply or start.
            return;
        }
        // An expired profile is not built again (contract 4.1).
        if let Some(error) = super::rule_sets::expired(&profile) {
            self.rebuild_failed(error, "host ipv6 rebuild failed");
            return;
        }
        *self.tun.host.lock().expect("host ipv6") = state;
        let previous = Ipv6State {
            ipv6: true,
            no_host_ipv6_route: handed_off,
        };
        let build = || translate::translate(&profile, &self.options(mode, &selected, &pins));
        let translation = match build() {
            Ok(translation) => translation,
            Err(e) => {
                tracing::error!(previous_policy = policy(previous), policy = policy(state),
                        rebuilt = false, error = %e, "host ipv6 changed");
                return;
            }
        };
        // The selection and the pins are as applied: sail keeps them across
        // a reload. The TUN itself does not change here, so this is a
        // reload; should it ever change, the switch restarts instead.
        let Some(running) = self.live().applied.as_ref().map(|a| a.translation.clone()) else {
            return;
        };
        let (switch, translation) = match self.switch_to(&running, translation, &build).await {
            Ok(switched) => switched,
            Err(error) => {
                tracing::error!(previous_policy = policy(previous), policy = policy(state),
                    rebuilt = false, error = %error, "host ipv6 changed");
                return;
            }
        };
        tracing::info!(
            previous_policy = policy(previous),
            policy = policy(state),
            rebuilt = true,
            "host ipv6 changed"
        );
        {
            let mut live = self.live();
            let Some(applied) = live.applied.as_mut() else {
                return;
            };
            applied.translation = translation;
            let revision = applied.profile.revision.clone();
            drop(live);
            if switch == crate::request::SwitchKind::KernelSwitch {
                self.kernel_switched(&revision);
            }
        }
        self.refresh().await;
    }

    /// Replaces the host IPv6 probe.
    #[cfg(test)]
    pub(super) fn set_host_ipv6_probe(&self, probe: impl Fn() -> HostIpv6 + Send + Sync + 'static) {
        *self.tun.probe.lock().expect("host ipv6 probe") = Arc::new(probe);
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use serde_json::Value;

    use super::super::lifecycle_tests::{drain, kinds, running, R1};
    use super::*;
    use crate::config::TunConfig;
    use crate::event::{Event, EventKind};
    use crate::runtime::fake::{Call, FakeRuntime};

    /// What the injected probe answers, and how often it was asked.
    #[derive(Default)]
    struct Host {
        answer: Mutex<Option<HostIpv6>>,
        asked: AtomicUsize,
    }

    impl Host {
        fn set(&self, available: bool, route: Result<bool, String>) {
            *self.answer.lock().unwrap() = Some(HostIpv6 { available, route });
        }
        fn asked(&self) -> usize {
            self.asked.load(Ordering::SeqCst)
        }
    }

    fn instance(role: Role, platform: Platform) -> (Engine, Arc<FakeRuntime>, Arc<Host>) {
        let fake = Arc::new(FakeRuntime::default());
        let engine = Engine::with_runtime(
            EngineConfig::new(role, platform, "/nonexistent"),
            fake.clone(),
        );
        let host = Arc::new(Host::default());
        host.set(true, Ok(true));
        let probe = host.clone();
        engine.inner.set_host_ipv6_probe(move || {
            probe.asked.fetch_add(1, Ordering::SeqCst);
            probe.answer.lock().unwrap().clone().expect("an answer")
        });
        (engine, fake, host)
    }

    fn tun_inbounds(config: &str) -> Vec<Value> {
        let config: Value = serde_json::from_str(config).unwrap();
        config["inbounds"]
            .as_array()
            .into_iter()
            .flatten()
            .filter(|inbound| inbound["type"] == "tun")
            .cloned()
            .collect()
    }

    fn hands_off(fake: &FakeRuntime) -> bool {
        fake.config().unwrap().contains("proxy_and_direct")
    }

    fn reloads(fake: &FakeRuntime) -> usize {
        fake.calls()
            .iter()
            .filter(|call| matches!(call, Call::Reload(_)))
            .count()
    }

    #[tokio::test]
    async fn a_tun_instance_runs_a_tun_inbound_a_standard_one_does_not() {
        let (engine, fake, host) = instance(Role::Tun, Platform::Linux);
        running(&engine).await;
        let inbounds = tun_inbounds(&fake.config().unwrap());
        assert_eq!(inbounds.len(), 1, "{inbounds:?}");
        assert_eq!(inbounds[0]["interface_name"], "ppvpn0");
        assert_eq!(inbounds[0]["auto_route"], true);
        assert_eq!(inbounds[0]["address"].as_array().unwrap().len(), 2, "IPv6");
        assert_eq!(host.asked(), 2, "probed on apply and on start");

        let (standard, fake, host) = instance(Role::Standard, Platform::Linux);
        running(&standard).await;
        assert!(tun_inbounds(&fake.config().unwrap()).is_empty());
        assert_eq!(host.asked(), 0);
    }

    #[tokio::test]
    async fn mobile_tun_has_no_auto_route_and_no_probe() {
        let (engine, fake, host) = instance(Role::Tun, Platform::Android);
        running(&engine).await;
        let inbounds = tun_inbounds(&fake.config().unwrap());
        assert_eq!(inbounds.len(), 1);
        assert_eq!(inbounds[0].get("auto_route"), None);
        assert_eq!(inbounds[0].get("interface_name"), None);
        assert_eq!(host.asked(), 0);
    }

    #[tokio::test]
    async fn the_probe_decides_the_tun_ipv6_and_the_hand_off() {
        let (engine, fake, host) = instance(Role::Tun, Platform::Linux);
        host.set(true, Ok(false));
        running(&engine).await;
        assert!(hands_off(&fake));

        // IPv6 disabled: an IPv4-only TUN, no hand-off.
        let (engine, fake, host) = instance(Role::Tun, Platform::Linux);
        host.set(false, Ok(false));
        running(&engine).await;
        let inbounds = tun_inbounds(&fake.config().unwrap());
        assert_eq!(inbounds[0]["address"].as_array().unwrap().len(), 1);
        assert!(!hands_off(&fake));

        // A route that cannot be read: IPv6 as before.
        let (engine, fake, host) = instance(Role::Tun, Platform::Linux);
        host.set(true, Err("cannot read".into()));
        running(&engine).await;
        assert!(!hands_off(&fake));
    }

    #[tokio::test]
    async fn local_dns_servers_override_the_system_and_bad_ones_fail_new() {
        let config = EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent")
            .with_tun(TunConfig::new().with_local_dns_servers(vec!["192.0.2.53".into()]));
        let engine = Engine::with_runtime(config, Arc::new(FakeRuntime::default()));
        assert_eq!(
            engine.inner.tun_options().unwrap().local_dns,
            LocalDns::Servers(vec!["192.0.2.53:53".parse().unwrap()])
        );
        let (engine, _, _) = engine_default();
        assert_eq!(
            engine.inner.tun_options().unwrap().local_dns,
            LocalDns::System
        );

        let dir =
            std::env::temp_dir().join(format!("ppvpn-core-engine-tun-dns-{}", std::process::id()));
        let err = Engine::new(
            EngineConfig::new(Role::Tun, Platform::Linux, dir)
                .with_tun(TunConfig::new().with_local_dns_servers(vec!["not-an-ip".into()])),
        )
        .await
        .unwrap_err();
        assert_eq!(
            (err.code, err.field.as_deref(), err.retryable),
            (
                codes::CORE_OPERATION_FAILED,
                Some("tun.local_dns_servers"),
                false
            )
        );
    }

    fn engine_default() -> (Engine, Arc<FakeRuntime>, Arc<Host>) {
        instance(Role::Tun, Platform::Linux)
    }

    #[tokio::test]
    async fn a_changed_ipv6_path_switches_kernels() {
        let (engine, fake, host) = engine_default();
        running(&engine).await;
        assert!(!hands_off(&fake));
        let mut rx = engine.subscribe(&[EventKind::KernelSwitched]);

        // The host joins an IPv4-only network.
        host.set(true, Ok(false));
        engine.reprobe_host_ipv6().await;
        assert_eq!(reloads(&fake), 1);
        assert!(hands_off(&fake));
        let events = drain(&mut rx);
        assert_eq!(kinds(&events), [EventKind::KernelSwitched]);
        assert!(matches!(&events[0], Event::KernelSwitched { revision, .. } if revision == R1));

        // And back.
        host.set(true, Ok(true));
        engine.reprobe_host_ipv6().await;
        assert_eq!(reloads(&fake), 2);
        assert!(!hands_off(&fake));
        assert_eq!(kinds(&drain(&mut rx)), [EventKind::KernelSwitched]);
    }

    #[tokio::test]
    async fn an_unchanged_ipv6_path_does_nothing() {
        let (engine, fake, host) = engine_default();
        running(&engine).await;
        let mut rx = engine.subscribe(EventKind::ALL);
        let calls = fake.calls().len();
        engine.reprobe_host_ipv6().await;
        assert_eq!(host.asked(), 3, "probed");
        assert_eq!(fake.calls().len(), calls, "nothing sent to the runtime");
        assert!(drain(&mut rx).is_empty());

        // IPv6 disabled meanwhile: left to the next apply or start.
        host.set(false, Ok(false));
        engine.reprobe_host_ipv6().await;
        assert_eq!(fake.calls().len(), calls);
        assert!(drain(&mut rx).is_empty());
    }

    #[tokio::test]
    async fn offline_or_stopped_does_not_probe() {
        let (engine, fake, host) = engine_default();
        running(&engine).await;
        engine.on_network(None);
        host.set(true, Ok(false));
        engine.reprobe_host_ipv6().await;
        assert_eq!(host.asked(), 2, "not probed offline");
        assert_eq!(reloads(&fake), 0);

        engine.on_network(Some(("eth0", 2)));
        engine.stop().await.unwrap();
        engine.reprobe_host_ipv6().await;
        assert_eq!(host.asked(), 2, "not probed while stopped");
        assert_eq!(reloads(&fake), 0);
    }

    #[tokio::test]
    async fn a_desktop_tun_instance_serves_dns_local_on_loopback() {
        let (engine, _fake, _) = instance(Role::Tun, Platform::Linux);
        engine.inner.start_local_dns().await.unwrap();
        match engine.inner.tun_options().unwrap().local_dns {
            LocalDns::Listener(addr) => {
                assert!(addr.ip().is_loopback());
                assert_ne!(addr.port(), 0);
            }
            other => panic!("{other:?}"),
        }
        // sail's network becomes dns-local's interface; offline, none.
        engine.inner.local_dns_network(&NetworkSnapshot {
            interface: Some("eth0".into()),
            index: Some(2),
            ..NetworkSnapshot::default()
        });
        assert_eq!(
            *engine.inner.tun.interface.lock().unwrap(),
            Some(Interface {
                index: 2,
                name: "eth0".into()
            })
        );
        engine.inner.local_dns_network(&NetworkSnapshot {
            offline: true,
            ..NetworkSnapshot::default()
        });
        assert_eq!(*engine.inner.tun.interface.lock().unwrap(), None);
    }

    /// During the first start, before the run reported its network, sail
    /// resolves its nodes through dns-local: it reads sail's snapshot (ready
    /// from the Starting phase on) and dials through direct.
    #[tokio::test]
    async fn dns_local_works_during_the_first_start() {
        let (engine, fake, _) = instance(Role::Tun, Platform::Linux);
        engine.inner.start_local_dns().await.unwrap();
        engine
            .apply(crate::request::ApplyRequest::new(
                super::super::lifecycle_tests::profile(R1),
            ))
            .await
            .unwrap();
        let gate = fake.hold_start();
        let starting = {
            let engine = engine.clone();
            tokio::spawn(async move { engine.start().await })
        };
        let mut states = crate::runtime::Runtime::states(fake.as_ref());
        while *states.borrow_and_update() != crate::runtime::RuntimeState::Starting {
            states.changed().await.unwrap();
        }
        // Not reported yet: the run has not started.
        assert_eq!(*engine.inner.tun.interface.lock().unwrap(), None);
        let current = engine.inner.tun.interface.clone();
        assert_eq!(
            local_dns_interface(&current, engine.inner.runtime.as_ref()),
            Some(Interface {
                index: 2,
                name: "eth0".into()
            })
        );
        let dial = localdns::RuntimeDial {
            runtime: engine.inner.runtime.clone(),
            outbound: translate::DIRECT_TAG.into(),
        };
        let server = "192.0.2.53:53".parse().unwrap();
        assert!(localdns::Dial::udp(&dial, server).await.is_ok());
        gate.notify_one();
        starting.await.unwrap().unwrap();
    }

    /// A dns-local query hung in its dial (a resolver that does not
    /// answer) ends when the instance stops: sail's stop waits for no dial
    /// of ours, and the stop returns at once (#221).
    #[tokio::test]
    async fn stop_ends_the_dns_local_queries_under_way() {
        let fake = Arc::new(FakeRuntime::default());
        let engine = Engine::with_runtime(
            EngineConfig::new(Role::Tun, Platform::Linux, "/nonexistent")
                .with_tun(TunConfig::new().with_local_dns_servers(vec!["192.0.2.53:53".into()])),
            fake.clone(),
        );
        engine.inner.set_host_ipv6_probe(|| HostIpv6 {
            available: true,
            route: Ok(true),
        });
        engine.inner.start_local_dns().await.unwrap();
        running(&engine).await;
        fake.hang_dials();
        let addr = match engine.inner.tun_options().unwrap().local_dns {
            LocalDns::Listener(addr) => addr,
            other => panic!("{other:?}"),
        };
        let mut query = hickory_proto::op::Message::new(
            7,
            hickory_proto::op::MessageType::Query,
            hickory_proto::op::OpCode::Query,
        );
        query.add_query(hickory_proto::op::Query::query(
            "example.com.".parse().unwrap(),
            hickory_proto::rr::RecordType::A,
        ));
        let client = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
        client
            .send_to(&query.to_vec().unwrap(), addr)
            .await
            .unwrap();
        while fake.pending_dials() == 0 {
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }
        let started = std::time::Instant::now();
        engine.stop().await.unwrap();
        assert!(
            started.elapsed() < std::time::Duration::from_millis(500),
            "{:?}",
            started.elapsed()
        );
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert_eq!(fake.pending_dials(), 0, "the hung dial is dropped");
    }

    #[tokio::test]
    async fn only_desktop_tun_instances_run_dns_local() {
        for (role, platform) in [
            (Role::Standard, Platform::Linux),
            (Role::Tun, Platform::Ios),
        ] {
            let (engine, _fake, _) = instance(role, platform);
            engine.inner.start_local_dns().await.unwrap();
            assert!(engine.inner.tun.listener.lock().unwrap().is_none());
        }
    }
}
