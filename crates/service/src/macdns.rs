//! Removes the macOS system DNS override earlier service versions published
//! (only called on macOS; compiled everywhere so its tests run everywhere).
//!
//! Earlier services pointed the system resolver at the TUN themselves: while
//! the core ran they wrote [`DNS_KEY`] with `scutil`, a supplemental
//! resolver matching every domain. Since Sail 0.17.0, Sail writes an entry
//! of its own when it opens the utun and removes it on stop (a temporary key
//! under another service id, which the system also drops when the process
//! dies), so the service writes nothing. But an older service that was
//! killed while connected, or upgraded with its key in place, can have left
//! [`DNS_KEY`] behind: it outlives the process that wrote it (until a
//! reboot), and with no TUN answering, the queries it takes fail. The
//! service removes it at start, the uninstaller too.

#![cfg_attr(not(target_os = "macos"), allow(dead_code))]

/// The DNS entity earlier services published, under a dynamic-store service
/// id of their own (`com.peakpassvpn.ppvpn.tun`, never a real network
/// service).
pub const DNS_KEY: &str = "State:/Network/Service/com.peakpassvpn.ppvpn.tun/DNS";
/// The TUN resolver those entries pointed at.
const TUN_DNS_V4: &str = "10.60.159.90";
/// The TUN resolver before ppvpn-core 0.5.7 (172.19.0.2, the sing-box
/// default other tunnels use too): an override an older service left behind
/// is still recognised as ours and removed.
const OLD_TUN_DNS_V4: &str = "172.19.0.2";

/// Runs a system command, feeding `stdin`; `Some(stdout)` when it exited
/// successfully.
pub trait Runner {
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

/// `scutil` script that removes [`DNS_KEY`].
fn remove_script() -> String {
    format!("remove {DNS_KEY}\nquit\n")
}

fn show_script() -> String {
    format!("show {DNS_KEY}\nquit\n")
}

/// [`DNS_KEY`] holds an override of ours. `scutil` exits 0 even when it
/// cannot read or write a key, so the entry itself is the evidence.
fn present(runner: &dyn Runner) -> bool {
    runner
        .run("scutil", &[], Some(&show_script()))
        .is_some_and(|shown| shown.contains(TUN_DNS_V4) || shown.contains(OLD_TUN_DNS_V4))
}

/// Removes a leftover override, then flushes the resolver caches; true when
/// one was there. `when` names the moment for the log.
pub fn remove_leftover_with(runner: &dyn Runner, when: &str) -> bool {
    if !present(runner) {
        return false;
    }
    let _ = runner.run("scutil", &[], Some(&remove_script()));
    if present(runner) {
        log::error!("{when}: scutil did not remove {DNS_KEY}");
    } else {
        log::warn!("{when}: removed the TUN DNS override an older service left behind");
    }
    let _ = runner.run("dscacheutil", &["-flushcache"], None);
    let _ = runner.run("killall", &["-HUP", "mDNSResponder"], None);
    true
}

/// [`remove_leftover_with`] the real commands.
pub fn remove_leftover(when: &str) -> bool {
    remove_leftover_with(&SystemRunner, when)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    /// Records every command; `scutil show` answers what the fake dynamic
    /// store holds, `remove` empties it.
    #[derive(Default)]
    struct FakeSystem {
        calls: Mutex<Vec<String>>,
        store: Mutex<Option<String>>,
        read_only: bool,
    }

    impl FakeSystem {
        fn holding(entry: &str) -> Self {
            Self {
                store: Mutex::new(Some(entry.into())),
                ..Self::default()
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }

        fn holds_an_entry(&self) -> bool {
            self.store.lock().unwrap().is_some()
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
            if program == "scutil" {
                let input = stdin.unwrap_or_default();
                let mut store = self.store.lock().unwrap();
                if input.starts_with("show ") {
                    return Some(store.clone().unwrap_or_else(|| "  No such key\n".into()));
                }
                if self.read_only {
                    return Some("  Permission denied\n".into());
                }
                if input.starts_with(&format!("remove {DNS_KEY}")) {
                    *store = None;
                }
            }
            Some(String::new())
        }
    }

    /// `scutil show` of what an earlier service published.
    const LEFTOVER: &str = "<dictionary> {\n  ServerAddresses : <array> {\n    0 : 10.60.159.90\n    1 : fde2:ec40:9312:c7fd::2\n  }\n  SupplementalMatchDomains : <array> {\n    0 : \n  }\n  SupplementalMatchDomainsNoSearch : 1\n}\n";

    #[test]
    fn a_leftover_override_is_removed_then_the_caches_flushed() {
        let system = FakeSystem::holding(LEFTOVER);
        assert!(remove_leftover_with(&system, "test"));
        assert!(!system.holds_an_entry());
        let show = format!("scutil <<< show {DNS_KEY}\nquit\n");
        assert_eq!(
            system.calls(),
            [
                show.clone(),
                format!("scutil <<< remove {DNS_KEY}\nquit\n"),
                show,
                "dscacheutil -flushcache".into(),
                "killall -HUP mDNSResponder".into(),
            ]
        );
    }

    #[test]
    fn nothing_changes_without_a_leftover() {
        let system = FakeSystem::default();
        assert!(!remove_leftover_with(&system, "test"));
        assert_eq!(system.calls().len(), 1, "only the lookup");
    }

    #[test]
    fn an_override_from_before_the_new_tun_addresses_is_still_ours() {
        let system = FakeSystem::holding(
            "<dictionary> {\n  ServerAddresses : <array> {\n    0 : 172.19.0.2\n  }\n}\n",
        );
        assert!(remove_leftover_with(&system, "test"));
        assert!(!system.holds_an_entry(), "removed");
    }

    #[test]
    fn an_entry_that_is_not_ours_stays() {
        let system = FakeSystem::holding(
            "<dictionary> {\n  ServerAddresses : <array> {\n    0 : 192.0.2.53\n  }\n}\n",
        );
        assert!(!remove_leftover_with(&system, "test"));
        assert!(system.holds_an_entry());
    }

    #[test]
    fn a_removal_scutil_refused_is_logged_not_retried() {
        let system = FakeSystem {
            read_only: true,
            ..FakeSystem::holding(LEFTOVER)
        };
        assert!(remove_leftover_with(&system, "test"));
        assert!(system.holds_an_entry());
        assert_eq!(system.calls().len(), 5);
    }
}
