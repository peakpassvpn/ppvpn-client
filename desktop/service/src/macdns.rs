//! Points the macOS system resolver at the TUN while the privileged core
//! runs (macOS only; a no-op elsewhere).
//!
//! sing-tun routes IPv4 and IPv6 into the utun and ppvpn-core hijacks port
//! 53 that enters it, but on macOS nothing changes the system DNS. When the
//! active resolvers are on-link (the router's LAN address, `fe80::…%en0`, a
//! LAN ULA), their more specific direct routes win, the queries never enter
//! the TUN and leak, and DNS rules / domain routing do not apply.
//!
//! While the core is up, the service publishes one SystemConfiguration
//! dynamic-store entry, [`DNS_KEY`], for a service of its own:
//!
//! ```text
//! ServerAddresses                  10.60.159.90 [fde2:ec40:9312:c7fd::2]
//! SupplementalMatchDomains         ""   (the empty domain: match all)
//! SupplementalMatchDomainsNoSearch 1
//! ```
//!
//! configd's IPMonitor keeps the DNS of a service that has no IPv4/IPv6
//! entity (only its `State:` DNS), and turns a supplemental match domain ""
//! into a resolver *without* a domain, i.e. a default resolver. Supplemental
//! resolvers are collected before the primary service's default resolver and
//! domain-less resolvers keep that order when sorted, so ours becomes
//! resolver #1 in `scutil --dns` (the same mechanism as a Network Extension
//! VPN's `matchDomains = [""]`). The primary service's search domains are
//! still attached to it. Nothing persistent is touched: no `Setup:` key, no
//! `networksetup`, so there is nothing to back up. The entry disappears when
//! the key is removed, and with a reboot even if the service never got to
//! remove it; a service killed with the key in place removes it on its next
//! start (no core can be running then).
//!
//! ppvpn-core's own direct resolver (`dns-local`) does not read the system
//! resolver on Darwin once a TUN exists (it uses the DHCP resolvers of the
//! physical interface), so the override cannot loop.

/// The DNS entity published while the core runs, under a dynamic-store
/// service id of our own (`com.peakpassvpn.ppvpn.tun`, never a real network
/// service).
pub const DNS_KEY: &str = "State:/Network/Service/com.peakpassvpn.ppvpn.tun/DNS";
/// TUN peer addresses ppvpn-core answers DNS on (its builder's .2 / ::2).
pub const TUN_DNS_V4: &str = "10.60.159.90";
pub const TUN_DNS_V6: &str = "fde2:ec40:9312:c7fd::2";
/// ppvpn-core gives the TUN this IPv6 address only when the host has IPv6.
pub const TUN_ADDRESS_V6: &str = "fde2:ec40:9312:c7fd::1";
/// The TUN resolver before ppvpn-core 0.5.7 (172.19.0.2, the sing-box
/// default other tunnels use too): an override an older service left behind
/// is still recognised as ours and removed.
const OLD_TUN_DNS_V4: &str = "172.19.0.2";

/// Runs a system command, feeding `stdin`; `Some(stdout)` when it exited
/// successfully.
pub trait Runner: Send {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Option<String>;
}

/// The real commands.
pub struct SystemRunner;

impl Runner for SystemRunner {
    fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Option<String> {
        use std::io::Write;
        use std::process::{Command, Stdio};
        let mut child = Command::new(program)
            .args(args)
            .stdin(if stdin.is_some() {
                Stdio::piped()
            } else {
                Stdio::null()
            })
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .ok()?;
        if let (Some(input), Some(mut pipe)) = (stdin, child.stdin.take()) {
            let _ = pipe.write_all(input.as_bytes());
        }
        let output = child.wait_with_output().ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// `scutil` script that publishes [`DNS_KEY`].
pub fn apply_script(ipv6: bool) -> String {
    let servers = if ipv6 {
        format!("{TUN_DNS_V4} {TUN_DNS_V6}")
    } else {
        TUN_DNS_V4.to_string()
    };
    format!(
        "d.init\n\
         d.add ServerAddresses * {servers}\n\
         d.add SupplementalMatchDomains * \"\"\n\
         d.add SupplementalMatchDomainsNoSearch # 1\n\
         set {DNS_KEY}\n\
         quit\n"
    )
}

/// `scutil` script that removes [`DNS_KEY`].
pub fn remove_script() -> String {
    format!("remove {DNS_KEY}\nquit\n")
}

fn show_script() -> String {
    format!("show {DNS_KEY}\nquit\n")
}

/// The system DNS override; `enabled` is false off macOS (and in the core
/// manager's tests), where every call is a no-op.
pub struct TunDns {
    runner: Box<dyn Runner>,
    enabled: bool,
}

impl Default for TunDns {
    fn default() -> Self {
        Self::new(
            Box::new(SystemRunner),
            cfg!(all(target_os = "macos", not(test))),
        )
    }
}

impl TunDns {
    pub fn new(runner: Box<dyn Runner>, enabled: bool) -> Self {
        Self { runner, enabled }
    }

    fn scutil(&self, script: &str) -> Option<String> {
        self.runner.run("scutil", &[], Some(script))
    }

    /// [`DNS_KEY`] currently exists. `scutil` exits 0 even when it cannot
    /// read or write a key, so the entry itself is the evidence.
    fn published(&self) -> bool {
        self.scutil(&show_script())
            .is_some_and(|shown| shown.contains(TUN_DNS_V4) || shown.contains(OLD_TUN_DNS_V4))
    }

    /// The TUN carries IPv6 (ppvpn-core adds its IPv6 address only on hosts
    /// with IPv6); without it an IPv6 resolver would only cost a timeout.
    fn tun_has_ipv6(&self) -> bool {
        self.runner
            .run("ifconfig", &[], None)
            .is_some_and(|interfaces| interfaces.contains(TUN_ADDRESS_V6))
    }

    fn flush_cache(&self) {
        let _ = self.runner.run("dscacheutil", &["-flushcache"], None);
        let _ = self.runner.run("killall", &["-HUP", "mDNSResponder"], None);
    }

    /// Makes the TUN resolvers the system default (after the core reported
    /// its TUN started).
    pub fn apply(&self) -> anyhow::Result<()> {
        if !self.enabled {
            return Ok(());
        }
        let ipv6 = self.tun_has_ipv6();
        let _ = self.scutil(&apply_script(ipv6));
        let published = self.published();
        self.flush_cache();
        if !published {
            return Err(anyhow::anyhow!("scutil did not publish {DNS_KEY}"));
        }
        log::info!(
            "system DNS points at the TUN ({TUN_DNS_V4}{}) via {DNS_KEY}",
            if ipv6 {
                format!(", {TUN_DNS_V6}")
            } else {
                String::new()
            }
        );
        Ok(())
    }

    /// Removes the override (core stopped, service stopping); true when an
    /// entry was there.
    pub fn remove(&self) -> bool {
        if !self.enabled || !self.published() {
            return false;
        }
        let _ = self.scutil(&remove_script());
        if self.published() {
            log::error!("scutil did not remove {DNS_KEY}");
        } else {
            log::info!("system DNS restored ({DNS_KEY} removed)");
        }
        self.flush_cache();
        true
    }

    /// Removes an override a killed service left behind, unless a core is
    /// running (it could be a live core's). `when` names the moment. True
    /// when an entry is there and was left for later (a core runs).
    pub fn clean_leftover(&self, core_running: bool, when: &str) -> bool {
        if !self.enabled {
            return false;
        }
        if core_running {
            if !self.published() {
                return false;
            }
            log::info!(
                "{when}: a ppvpn-core process is running; keeping the system DNS until it exits"
            );
            return true;
        }
        if self.remove() {
            log::warn!("{when}: removed the TUN DNS override a killed service left behind");
        }
        false
    }
}

/// What [`DnsWorker`] is asked to do, in order.
enum Op {
    Apply,
    Remove,
    /// At service start: `bool` = a privileged core runs.
    CleanLeftover(bool),
    /// No core of ours runs: remove an orphaned override once `fn` says no
    /// privileged core is left.
    CheckOrphan(fn() -> bool),
    Flush(std::sync::mpsc::Sender<()>),
}

/// [`TunDns`] plus the override an earlier instance left for a core that
/// was still running.
struct DnsState {
    dns: TunDns,
    orphaned: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

impl DnsState {
    fn run(&mut self, op: Op) {
        use std::sync::atomic::Ordering;
        let started = std::time::Instant::now();
        let name = match op {
            Op::Apply => {
                self.orphaned.store(false, Ordering::SeqCst);
                if let Err(error) = self.dns.apply() {
                    log::error!("cannot point the system DNS at the TUN: {error:#}");
                }
                "apply"
            }
            Op::Remove => {
                self.dns.remove();
                "remove"
            }
            Op::CleanLeftover(core_running) => {
                let left = self.dns.clean_leftover(core_running, "service start");
                self.orphaned.store(left, Ordering::SeqCst);
                "startup clean-up"
            }
            Op::CheckOrphan(core_running) => {
                if !self.orphaned.load(Ordering::SeqCst) || core_running() {
                    return;
                }
                self.orphaned.store(false, Ordering::SeqCst);
                if self.dns.remove() {
                    log::warn!("removed the TUN DNS override an orphaned ppvpn-core left behind");
                }
                "orphan clean-up"
            }
            Op::Flush(done) => {
                let _ = done.send(());
                return;
            }
        };
        if self.dns.enabled {
            log::info!(
                "system DNS {name} took {} ms",
                started.elapsed().as_millis()
            );
        }
    }
}

/// Runs the [`TunDns`] commands off the core lock, in order, on a thread of
/// their own: `scutil`, `dscacheutil` and `killall` were seen taking over a
/// minute on a busy Mac, and with the lock held the Connect answer, the
/// lease watchdog and Disconnect all waited for them. Inline (on the
/// caller's thread) in tests and where the override is disabled.
pub struct DnsWorker {
    orphaned: std::sync::Arc<std::sync::atomic::AtomicBool>,
    mode: Mode,
}

enum Mode {
    Inline(Box<DnsState>),
    Thread(std::sync::mpsc::Sender<Op>),
}

impl Default for DnsWorker {
    fn default() -> Self {
        let dns = TunDns::default();
        if dns.enabled {
            Self::threaded(dns)
        } else {
            Self::inline(dns)
        }
    }
}

impl DnsWorker {
    pub fn inline(dns: TunDns) -> Self {
        let orphaned = std::sync::Arc::default();
        let state = DnsState {
            dns,
            orphaned: std::sync::Arc::clone(&orphaned),
        };
        Self {
            orphaned,
            mode: Mode::Inline(Box::new(state)),
        }
    }

    fn threaded(dns: TunDns) -> Self {
        let orphaned = std::sync::Arc::default();
        let mut state = DnsState {
            dns,
            orphaned: std::sync::Arc::clone(&orphaned),
        };
        let (sender, receiver) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new()
            .name("system-dns".into())
            .spawn(move || {
                for op in receiver {
                    state.run(op);
                }
            });
        if let Err(error) = spawned {
            log::error!("cannot start the system DNS thread: {error}");
        }
        Self {
            orphaned,
            mode: Mode::Thread(sender),
        }
    }

    fn send(&mut self, op: Op) {
        match &mut self.mode {
            Mode::Inline(state) => state.run(op),
            Mode::Thread(sender) => {
                let _ = sender.send(op);
            }
        }
    }

    /// Points the system resolver at the TUN (queued).
    pub fn apply(&mut self) {
        self.send(Op::Apply);
    }

    /// Removes the override (queued).
    pub fn remove(&mut self) {
        self.send(Op::Remove);
    }

    /// See [`TunDns::clean_leftover`]; an entry left for a running core is
    /// removed by a later [`Self::check_orphan`].
    pub fn clean_leftover(&mut self, core_running: bool) {
        self.send(Op::CleanLeftover(core_running));
    }

    /// No core of ours runs: removes an orphaned override once
    /// `core_running` says no privileged core is left. Queued only while
    /// one is pending.
    pub fn check_orphan(&mut self, core_running: fn() -> bool) {
        if self.orphaned.load(std::sync::atomic::Ordering::SeqCst) {
            self.send(Op::CheckOrphan(core_running));
        }
    }

    /// Waits (up to `timeout`) until everything queued so far ran: the
    /// service must not exit with the override still in place.
    pub fn flush(&mut self, timeout: std::time::Duration) -> bool {
        let (done, finished) = std::sync::mpsc::channel();
        self.send(Op::Flush(done));
        finished.recv_timeout(timeout).is_ok()
    }
}

#[cfg(test)]
#[allow(unused_imports)] // unused by the uninstall binary's tests
pub(crate) use tests::FakeSystem;

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};

    /// Records every command; `scutil show` answers what the fake dynamic
    /// store holds, `set` / `remove` change it.
    #[derive(Clone, Default)]
    pub(crate) struct FakeSystem {
        calls: Arc<Mutex<Vec<String>>>,
        pub(crate) store: Arc<Mutex<Option<String>>>,
        ifconfig: String,
        read_only: bool,
    }

    impl FakeSystem {
        pub(crate) fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl Runner for FakeSystem {
        fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Option<String> {
            let mut call = format!("{program} {}", args.join(" ")).trim().to_string();
            if let Some(input) = stdin {
                call.push_str(" <<< ");
                call.push_str(input);
            }
            self.calls.lock().unwrap().push(call);
            match program {
                "ifconfig" => Some(self.ifconfig.clone()),
                "scutil" => {
                    let input = stdin.unwrap_or_default();
                    let mut store = self.store.lock().unwrap();
                    if input.starts_with("show ") {
                        return Some(store.clone().unwrap_or_else(|| "  No such key\n".into()));
                    }
                    if self.read_only {
                        return Some("  Permission denied\n".into());
                    }
                    if input.contains(&format!("set {DNS_KEY}")) {
                        *store = Some(input.to_string());
                    } else if input.starts_with(&format!("remove {DNS_KEY}")) {
                        *store = None;
                    }
                    Some(String::new())
                }
                _ => Some(String::new()),
            }
        }
    }

    const FLUSH: [&str; 2] = ["dscacheutil -flushcache", "killall -HUP mDNSResponder"];

    fn dns(system: &FakeSystem, enabled: bool) -> TunDns {
        TunDns::new(Box::new(system.clone()), enabled)
    }

    #[test]
    fn apply_publishes_both_tun_resolvers_then_flushes() {
        let system = FakeSystem {
            ifconfig: "utun7: flags=8051\n\tinet 10.60.159.89 --> 10.60.159.89\n\tinet6 fde2:ec40:9312:c7fd::1 prefixlen 126\n".into(),
            ..FakeSystem::default()
        };
        dns(&system, true).apply().unwrap();
        let set = format!(
            "scutil <<< d.init\n\
             d.add ServerAddresses * 10.60.159.90 fde2:ec40:9312:c7fd::2\n\
             d.add SupplementalMatchDomains * \"\"\n\
             d.add SupplementalMatchDomainsNoSearch # 1\n\
             set {DNS_KEY}\n\
             quit\n"
        );
        let show = format!("scutil <<< show {DNS_KEY}\nquit\n");
        assert_eq!(
            system.calls(),
            [
                "ifconfig".to_string(),
                set,
                show,
                FLUSH[0].into(),
                FLUSH[1].into()
            ]
        );
    }

    #[test]
    fn apply_leaves_out_ipv6_without_an_ipv6_tun() {
        let system = FakeSystem {
            ifconfig: "utun7: flags=8051\n\tinet 10.60.159.89 --> 10.60.159.89\n".into(),
            ..FakeSystem::default()
        };
        dns(&system, true).apply().unwrap();
        let store = system.store.lock().unwrap().clone().unwrap();
        assert!(store.contains("d.add ServerAddresses * 10.60.159.90\n"));
        assert!(!store.contains(TUN_DNS_V6));
    }

    #[test]
    fn apply_reports_an_entry_scutil_did_not_write() {
        let system = FakeSystem {
            read_only: true,
            ..FakeSystem::default()
        };
        assert!(dns(&system, true).apply().is_err());
    }

    #[test]
    fn remove_deletes_the_key_then_flushes() {
        let system = FakeSystem::default();
        let dns = dns(&system, true);
        dns.apply().unwrap();
        system.calls.lock().unwrap().clear();
        assert!(dns.remove());
        let show = format!("scutil <<< show {DNS_KEY}\nquit\n");
        assert_eq!(
            system.calls(),
            [
                show.clone(),
                format!("scutil <<< remove {DNS_KEY}\nquit\n"),
                show,
                FLUSH[0].into(),
                FLUSH[1].into(),
            ]
        );
        assert!(system.store.lock().unwrap().is_none());
        // Nothing left: a second removal changes nothing.
        system.calls.lock().unwrap().clear();
        assert!(!dns.remove());
        assert_eq!(system.calls().len(), 1);
    }

    #[test]
    fn a_leftover_key_is_removed_at_startup_unless_a_core_runs() {
        let system = FakeSystem::default();
        *system.store.lock().unwrap() = Some(apply_script(true));
        let dns = dns(&system, true);
        assert!(dns.clean_leftover(true, "test"), "left for later");
        assert!(system.store.lock().unwrap().is_some());
        assert!(!dns.clean_leftover(false, "test"));
        assert!(system.store.lock().unwrap().is_none());
        assert!(system
            .calls()
            .contains(&format!("scutil <<< remove {DNS_KEY}\nquit\n")));
    }

    /// A runner that takes its time, like `scutil` on a busy Mac.
    struct Slow(FakeSystem);

    impl Runner for Slow {
        fn run(&self, program: &str, args: &[&str], stdin: Option<&str>) -> Option<String> {
            std::thread::sleep(std::time::Duration::from_millis(100));
            self.0.run(program, args, stdin)
        }
    }

    #[test]
    fn the_worker_thread_keeps_the_callers_waiting_for_nothing_but_flush() {
        let system = FakeSystem::default();
        let mut worker = DnsWorker::threaded(TunDns::new(Box::new(Slow(system.clone())), true));
        let started = std::time::Instant::now();
        worker.apply();
        worker.remove();
        worker.apply();
        assert!(started.elapsed() < std::time::Duration::from_millis(50));
        assert!(worker.flush(std::time::Duration::from_secs(10)));
        assert!(system.store.lock().unwrap().is_some(), "ran in order");
        worker.remove();
        assert!(worker.flush(std::time::Duration::from_secs(10)));
        assert!(system.store.lock().unwrap().is_none());
    }

    #[test]
    fn an_override_from_before_the_new_tun_addresses_is_still_ours() {
        let system = FakeSystem::default();
        *system.store.lock().unwrap() = Some(
            "<dictionary> {\n  ServerAddresses : <array> {\n    0 : 172.19.0.2\n  }\n}\n".into(),
        );
        let dns = dns(&system, true);
        assert!(!dns.clean_leftover(false, "test"));
        assert!(system.store.lock().unwrap().is_none(), "removed");
    }

    #[test]
    fn nothing_runs_off_macos() {
        let system = FakeSystem::default();
        *system.store.lock().unwrap() = Some(apply_script(true));
        let dns = dns(&system, false);
        dns.apply().unwrap();
        assert!(!dns.remove());
        dns.clean_leftover(false, "test");
        assert!(system.calls().is_empty());
    }

    #[test]
    fn the_default_is_disabled_in_tests() {
        assert!(!TunDns::default().enabled);
    }
}
