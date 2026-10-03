//! Removes routing state a killed ppvpn-core left behind (Linux).
//!
//! ppvpn-core's TUN auto-route installs policy rules at priorities
//! [`RULE_PRIORITIES`] and routes in table [`ROUTE_TABLE`], and removes them
//! when it stops. A core that is SIGKILLed (a service stop that ran into
//! systemd's `TimeoutStopSec`) cannot, and its rules stay, e.g.
//! `9091: from all iif tun0 lookup 2091 [detached]`, surviving a service
//! restart; so do the rules of a core that crashed or was killed while the
//! service ran. The service removes them when it starts, before it starts a
//! core and after its core stopped, as long as no privileged ppvpn-core
//! process is running (the state could be a live core's).
//!
//! macOS and Windows need no equivalent: the core's routes are bound to its
//! utun / Wintun interface, which the OS removes with the process.

use std::ops::RangeInclusive;

/// Policy-rule priorities ppvpn-core's auto-route uses.
pub const RULE_PRIORITIES: RangeInclusive<u32> = 9091..=9101;
/// Routing table ppvpn-core's auto-route uses.
pub const ROUTE_TABLE: &str = "2091";

/// Runs a system command; `Some(stdout)` when it exited successfully.
pub trait CommandRunner {
    fn run(&self, program: &str, args: &[&str]) -> Option<String>;
}

/// The real `ip` command.
#[cfg_attr(not(target_os = "linux"), allow(dead_code))]
pub struct SystemRunner;

impl CommandRunner for SystemRunner {
    fn run(&self, program: &str, args: &[&str]) -> Option<String> {
        let output = std::process::Command::new(program)
            .args(args)
            .stdin(std::process::Stdio::null())
            .output()
            .ok()?;
        output
            .status
            .success()
            .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

/// Priority of one `ip rule show` line (`"9091:\tfrom all …"`).
fn rule_priority(line: &str) -> Option<u32> {
    line.trim_start().split(':').next()?.trim().parse().ok()
}

/// Removes our rules and table routes for IPv4 and IPv6; returns what was
/// removed (for the log).
pub fn clean_stale_routes(runner: &dyn CommandRunner) -> Vec<String> {
    let mut removed = Vec::new();
    for family in ["-4", "-6"] {
        if let Some(rules) = runner.run("ip", &[family, "rule", "show"]) {
            for line in rules.lines() {
                let Some(priority) = rule_priority(line) else {
                    continue;
                };
                if !RULE_PRIORITIES.contains(&priority) {
                    continue;
                }
                let priority = priority.to_string();
                // One rule per call: several may share a priority.
                if runner
                    .run("ip", &[family, "rule", "del", "priority", &priority])
                    .is_some()
                {
                    removed.push(format!("ip {family} rule {}", line.trim()));
                }
            }
        }
        let routes = runner
            .run("ip", &[family, "route", "show", "table", ROUTE_TABLE])
            .unwrap_or_default();
        if !routes.trim().is_empty()
            && runner
                .run("ip", &[family, "route", "flush", "table", ROUTE_TABLE])
                .is_some()
        {
            removed.push(format!(
                "ip {family} table {ROUTE_TABLE}: {} route(s)",
                routes
                    .lines()
                    .filter(|line| !line.trim().is_empty())
                    .count()
            ));
        }
    }
    removed
}

/// [`clean_stale_routes`] unless a core is running; logs what it removed.
/// `when` names the moment (service start, before a core starts).
pub fn clean_if_no_core(runner: &dyn CommandRunner, core_running: bool, when: &str) {
    if core_running {
        log::info!("{when}: a ppvpn-core process is running; leaving routing state alone");
        return;
    }
    for entry in clean_stale_routes(runner) {
        log::warn!("{when}: removed stale routing state left by a killed core: {entry}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    /// Answers scripted outputs and records every call.
    #[derive(Default)]
    struct FakeRunner {
        outputs: HashMap<String, String>,
        calls: Mutex<Vec<String>>,
    }

    impl FakeRunner {
        fn with(outputs: &[(&str, &str)]) -> Self {
            Self {
                outputs: outputs
                    .iter()
                    .map(|(command, output)| (command.to_string(), output.to_string()))
                    .collect(),
                calls: Mutex::default(),
            }
        }

        fn calls(&self) -> Vec<String> {
            self.calls.lock().unwrap().clone()
        }
    }

    impl CommandRunner for FakeRunner {
        fn run(&self, program: &str, args: &[&str]) -> Option<String> {
            let command = format!("{program} {}", args.join(" "));
            self.calls.lock().unwrap().push(command.clone());
            if command.contains(" del ") || command.contains(" flush ") {
                return Some(String::new());
            }
            Some(self.outputs.get(&command).cloned().unwrap_or_default())
        }
    }

    const RULES_V4: &str = "0:\tfrom all lookup local\n\
        9091:\tfrom all iif tun0 lookup 2091 [detached]\n\
        9092:\tnot from all dport 53 lookup main suppress_prefixlength 0\n\
        9092:\tfrom all ipproto icmp goto 9101\n\
        9101:\tfrom all lookup 2091\n\
        9102:\tfrom all lookup other\n\
        32766:\tfrom all lookup main\n\
        32767:\tfrom all lookup default\n";

    #[test]
    fn removes_our_rules_and_table_only() {
        let runner = FakeRunner::with(&[
            ("ip -4 rule show", RULES_V4),
            (
                "ip -4 route show table 2091",
                "default dev tun0 scope link\n",
            ),
            ("ip -6 rule show", "0:\tfrom all lookup local\n"),
            ("ip -6 route show table 2091", ""),
        ]);
        let removed = clean_stale_routes(&runner);
        let changes: Vec<String> = runner
            .calls()
            .into_iter()
            .filter(|call| call.contains(" del ") || call.contains(" flush "))
            .collect();
        assert_eq!(
            changes,
            [
                "ip -4 rule del priority 9091",
                "ip -4 rule del priority 9092",
                "ip -4 rule del priority 9092",
                "ip -4 rule del priority 9101",
                "ip -4 route flush table 2091",
            ]
        );
        assert_eq!(removed.len(), 5);
        assert!(removed[0].contains("iif tun0 lookup 2091"));
    }

    #[test]
    fn nothing_is_touched_while_a_core_runs() {
        let runner = FakeRunner::with(&[("ip -4 rule show", RULES_V4)]);
        clean_if_no_core(&runner, true, "test");
        assert!(runner.calls().is_empty());
        clean_if_no_core(&runner, false, "test");
        assert!(runner
            .calls()
            .contains(&"ip -4 rule del priority 9091".to_string()));
    }

    #[test]
    fn a_clean_system_needs_no_changes() {
        let runner = FakeRunner::with(&[(
            "ip -4 rule show",
            "0:\tfrom all lookup local\n32766:\tfrom all lookup main\n",
        )]);
        assert!(clean_stale_routes(&runner).is_empty());
        assert!(runner
            .calls()
            .iter()
            .all(|call| !call.contains(" del ") && !call.contains(" flush ")));
    }
}
