//! The default interface's resolvers, read again on network changes (Go:
//! internal/localdns/servers.go `cache`).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Mutex;
use std::time::Duration;

use super::servers::{join, usable, Interface, Prefix, Server};
use super::{Discovered, LocalDnsError};

/// The shortest time between two reads on the same interface: without
/// usable servers a query fails at once instead of reading again.
pub const RETRY_INTERVAL: Duration = Duration::from_secs(1);
/// The age after which the next query reads again even without a change (a
/// DHCP renewal that changed the DNS).
pub const SOFT_REFRESH: Duration = Duration::from_secs(60);

/// A read whose outcome differs from the previous one: the host logs it as
/// `msg="local dns servers" source=… interface=… servers=…` at info
/// (`servers=none` with the error when there are none).
#[derive(Clone, Debug, PartialEq)]
pub struct Change {
    pub interface: String,
    pub source: String,
    pub servers: Vec<Server>,
    pub error: Option<String>,
}

impl Change {
    /// The servers as the log prints them: "none" when there are none.
    pub fn servers_field(&self) -> String {
        if self.servers.is_empty() {
            "none".into()
        } else {
            join(&self.servers)
        }
    }
}

type DiscoverFn = Box<dyn Fn(&Interface) -> Discovered + Send + Sync>;
type CurrentFn = Box<dyn Fn() -> Option<Interface> + Send + Sync>;
type ClockFn = Box<dyn Fn() -> Duration + Send + Sync>;
type ChangedFn = Box<dyn Fn(&Change) + Send + Sync>;

/// The resolvers of the default interface. Read again when the interface
/// differs from the one they were read on, after [`Cache::invalidate`] (an
/// interface change), after [`Cache::failed`] (every server failed) and after
/// [`SOFT_REFRESH`]; never more often than once per [`RETRY_INTERVAL`] on the
/// same interface, unless invalidated.
pub struct Cache {
    discover: DiscoverFn,
    current: CurrentFn,
    exclude: Vec<Prefix>,
    now: ClockFn,
    changed: ChangedFn,
    generation: AtomicU64,
    state: Mutex<State>,
}

#[derive(Default)]
struct State {
    read: bool,
    read_gen: u64,
    if_index: u32,
    stale: bool,
    servers: Vec<Server>,
    error: Option<LocalDnsError>,
    read_at: Duration,
    tried_at: Duration,
    logged: Option<String>,
}

impl Cache {
    /// `discover` reads the interface's resolvers; `current` is the default
    /// interface (None: no network); `now` a monotonic clock; `changed` is
    /// called with every read whose outcome differs from the previous one.
    pub fn new(
        discover: impl Fn(&Interface) -> Discovered + Send + Sync + 'static,
        current: impl Fn() -> Option<Interface> + Send + Sync + 'static,
        exclude: Vec<Prefix>,
        now: impl Fn() -> Duration + Send + Sync + 'static,
        changed: impl Fn(&Change) + Send + Sync + 'static,
    ) -> Self {
        Cache {
            discover: Box::new(discover),
            current: Box::new(current),
            exclude,
            now: Box::new(now),
            changed: Box::new(changed),
            generation: AtomicU64::new(0),
            state: Mutex::new(State::default()),
        }
    }

    /// Makes the next query read again (an interface change). Does not wait
    /// for a read in progress.
    pub fn invalidate(&self) {
        self.generation.fetch_add(1, Ordering::SeqCst);
    }

    /// Marks the servers suspect after every one of them failed: the next
    /// query reads again, at most once per [`RETRY_INTERVAL`].
    pub fn failed(&self) {
        self.state.lock().expect("cache").stale = true;
    }

    /// The resolvers to ask, read first when needed. Concurrent callers wait
    /// for one read.
    pub fn get(&self) -> Result<Vec<Server>, LocalDnsError> {
        self.resolve().map(|(_, servers)| servers)
    }

    /// [`Cache::get`] with the interface the servers were read on: a
    /// link-local server is dialled through its index.
    pub fn resolve(&self) -> Result<(Interface, Vec<Server>), LocalDnsError> {
        let iface = (self.current)().ok_or(LocalDnsError::NoInterface)?;
        let mut st = self.state.lock().expect("cache");
        let now = (self.now)();
        let gen = self.generation.load(Ordering::SeqCst);
        let same = st.read && st.read_gen == gen && st.if_index == iface.index;
        if same
            && !st.servers.is_empty()
            && !st.stale
            && now.saturating_sub(st.read_at) < SOFT_REFRESH
        {
            return Ok((iface, st.servers.clone()));
        }
        if same && now.saturating_sub(st.tried_at) < RETRY_INTERVAL {
            return match &st.error {
                None if !st.servers.is_empty() => Ok((iface, st.servers.clone())),
                Some(e) => Err(e.clone()),
                None => Err(LocalDnsError::NoServers {
                    interface: iface.name.clone(),
                    source: String::new(),
                }),
            };
        }
        st.tried_at = now;
        let found = (self.discover)(&iface);
        if found.error.is_some() && same && !st.servers.is_empty() {
            // A refresh that could not read (scutil timed out, ...) keeps the
            // servers read before on the same interface.
            return Ok((iface, st.servers.clone()));
        }
        let servers = usable(&found.servers, &iface, &self.exclude);
        let error = match &found.error {
            Some(cause) => Some(LocalDnsError::Read {
                interface: iface.name.clone(),
                source: found.source.clone(),
                cause: cause.clone(),
            }),
            None if servers.is_empty() => Some(LocalDnsError::NoServers {
                interface: iface.name.clone(),
                source: found.source.clone(),
            }),
            None => None,
        };
        st.read = true;
        st.read_gen = gen;
        st.if_index = iface.index;
        st.stale = false;
        st.servers = servers.clone();
        st.error = error.clone();
        st.read_at = now;
        let key = format!(
            "{} {} {} {} {}",
            iface.index,
            iface.name,
            found.source,
            join(&servers),
            error.is_some()
        );
        if st.logged.as_deref() != Some(key.as_str()) {
            st.logged = Some(key);
            (self.changed)(&Change {
                interface: iface.name.clone(),
                source: found.source,
                servers: servers.clone(),
                error: error.as_ref().map(|e| e.to_string()),
            });
        }
        match error {
            Some(e) => Err(e),
            None => Ok((iface, servers)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::localdns::servers::tunnel_prefixes;
    use std::collections::HashMap;
    use std::sync::atomic::AtomicU32;
    use std::sync::{Arc, Condvar};

    /// What the fake discover returns per interface name, and how often it ran.
    #[derive(Default)]
    struct Source {
        servers: Mutex<HashMap<String, Vec<Server>>>,
        error: Mutex<Option<String>>,
        reads: AtomicU32,
        gate: Option<Arc<(Mutex<bool>, Condvar)>>,
    }

    impl Source {
        fn set(&self, name: &str, values: &[&str]) {
            let list = values.iter().map(|v| Server::parse(v).unwrap()).collect();
            self.servers.lock().unwrap().insert(name.into(), list);
        }
        fn reads(&self) -> u32 {
            self.reads.load(Ordering::SeqCst)
        }
    }

    struct Rig {
        cache: Cache,
        source: Arc<Source>,
        iface: Arc<Mutex<Option<Interface>>>,
        clock: Arc<Mutex<Duration>>,
        logged: Arc<Mutex<Vec<(String, String, bool)>>>,
    }

    impl Rig {
        fn new(source: Source) -> Rig {
            let source = Arc::new(source);
            let iface = Arc::new(Mutex::new(Some(Interface {
                index: 6,
                name: "en0".into(),
            })));
            let clock = Arc::new(Mutex::new(Duration::from_secs(1000)));
            let logged = Arc::new(Mutex::new(Vec::new()));
            let (s, i, c, l) = (source.clone(), iface.clone(), clock.clone(), logged.clone());
            let cache = Cache::new(
                move |iface: &Interface| {
                    s.reads.fetch_add(1, Ordering::SeqCst);
                    if let Some(gate) = &s.gate {
                        let (lock, cv) = &**gate;
                        let mut open = lock.lock().unwrap();
                        while !*open {
                            open = cv.wait(open).unwrap();
                        }
                    }
                    let servers = s
                        .servers
                        .lock()
                        .unwrap()
                        .get(&iface.name)
                        .cloned()
                        .unwrap_or_default();
                    match s.error.lock().unwrap().clone() {
                        Some(e) => Discovered {
                            servers,
                            source: "fake".into(),
                            error: Some(e),
                        },
                        None => Discovered::read("fake", servers),
                    }
                },
                move || i.lock().unwrap().clone(),
                tunnel_prefixes(),
                move || *c.lock().unwrap(),
                move |change: &Change| {
                    l.lock().unwrap().push((
                        change.interface.clone(),
                        change.servers_field(),
                        change.error.is_some(),
                    ))
                },
            );
            Rig {
                cache,
                source,
                iface,
                clock,
                logged,
            }
        }
        fn get(&self) -> String {
            match self.cache.get() {
                Ok(s) => join(&s),
                Err(e) => format!("error: {e}"),
            }
        }
        fn advance(&self, d: Duration) {
            *self.clock.lock().unwrap() += d;
        }
        fn set_iface(&self, iface: Option<(u32, &str)>) {
            *self.iface.lock().unwrap() = iface.map(|(index, name)| Interface {
                index,
                name: name.into(),
            });
        }
    }

    // Go: TestCacheFollowsInterfaceChanges (B1, B2, B7).
    #[test]
    fn follows_interface_changes() {
        let rig = Rig::new(Source::default());
        rig.source.set("en0", &["192.168.1.1"]);
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.1:53", 1)
        );
        rig.advance(Duration::from_secs(30));
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.1:53", 1),
            "cached"
        );

        // Another Wi-Fi on the same interface: invalidated, read at once.
        rig.source.set("en0", &["10.0.0.1"]);
        rig.cache.invalidate();
        rig.advance(Duration::from_millis(1));
        assert_eq!((rig.get().as_str(), rig.source.reads()), ("10.0.0.1:53", 2));

        // A new default interface is read even without the callback.
        rig.source.set("en7", &["192.168.8.1"]);
        rig.set_iface(Some((16, "en7")));
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.8.1:53", 3)
        );

        // No network: fails at once, without a read.
        rig.set_iface(None);
        assert_eq!(rig.cache.get(), Err(LocalDnsError::NoInterface));
        assert_eq!(rig.source.reads(), 3);

        let want = vec![
            ("en0".to_string(), "192.168.1.1:53".to_string(), false),
            ("en0".to_string(), "10.0.0.1:53".to_string(), false),
            ("en7".to_string(), "192.168.8.1:53".to_string(), false),
        ];
        assert_eq!(*rig.logged.lock().unwrap(), want);
    }

    // Go: TestCacheFailsFastWithoutServers (B3; C1's error side).
    #[test]
    fn fails_fast_without_servers() {
        let rig = Rig::new(Source::default());
        // Only addresses that must never be used.
        rig.source.set(
            "en0",
            &[
                "127.0.0.1",
                "::1",
                "10.60.159.90",
                "172.19.0.2",
                "fec0:0:0:ffff::1",
            ],
        );
        for i in 0..5 {
            let got = rig.get();
            assert!(got.contains("no DNS servers on en0"), "query {i}: {got}");
            rig.advance(Duration::from_millis(100));
        }
        assert_eq!(rig.source.reads(), 1, "reads within RETRY_INTERVAL");
        rig.advance(RETRY_INTERVAL);
        rig.source.set("en0", &["192.168.1.1"]);
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.1:53", 2)
        );
        {
            let logged = rig.logged.lock().unwrap();
            assert_eq!(logged.len(), 2);
            assert!(logged[0].2 && !logged[1].2, "{logged:?}");
        }

        // An invalidation reads at once even right after a read.
        rig.source.set("en0", &[]);
        rig.cache.invalidate();
        assert!(rig.cache.get().is_err());
        assert_eq!(rig.source.reads(), 3);
    }

    // Go: TestCacheRefreshes (B4, B5).
    #[test]
    fn refreshes() {
        let rig = Rig::new(Source::default());
        rig.source.set("en0", &["192.168.1.1"]);
        rig.get();

        // Every server failed: read again on the next query, at most once
        // per RETRY_INTERVAL.
        rig.source.set("en0", &["192.168.1.2"]);
        rig.cache.failed();
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.1:53", 1),
            "within RETRY_INTERVAL"
        );
        rig.advance(RETRY_INTERVAL);
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.2:53", 2)
        );

        // Soft refresh after SOFT_REFRESH; a read that fails keeps the servers.
        rig.advance(SOFT_REFRESH);
        *rig.source.error.lock().unwrap() = Some("scutil timed out".into());
        assert_eq!(
            (rig.get().as_str(), rig.source.reads()),
            ("192.168.1.2:53", 3)
        );
        // A read error with nothing to keep is reported.
        rig.set_iface(Some((7, "en1")));
        let got = rig.get();
        assert!(
            got.contains("read the DNS servers of en1 (fake): scutil timed out"),
            "{got}"
        );
        let logged = rig.logged.lock().unwrap();
        assert_eq!(logged.len(), 3);
        assert!(logged[2].2);
    }

    // Go: TestCacheReadsOnceForConcurrentQueries (B6).
    #[test]
    fn concurrent_queries_share_one_read() {
        let gate = Arc::new((Mutex::new(false), Condvar::new()));
        let rig = Arc::new(Rig::new(Source {
            gate: Some(gate.clone()),
            ..Default::default()
        }));
        rig.source.set("en0", &["192.168.1.1"]);
        let threads: Vec<_> = (0..8)
            .map(|_| {
                let rig = rig.clone();
                std::thread::spawn(move || rig.cache.get().map(|s| s.len()))
            })
            .collect();
        while rig.source.reads() == 0 {
            std::thread::sleep(Duration::from_millis(1));
        }
        *gate.0.lock().unwrap() = true;
        gate.1.notify_all();
        for t in threads {
            assert_eq!(t.join().unwrap(), Ok(1));
        }
        assert_eq!(rig.source.reads(), 1);
    }
}
