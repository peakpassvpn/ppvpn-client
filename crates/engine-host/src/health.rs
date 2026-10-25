//! The health check's policy for a TUN instance (#438): when to check, and
//! what a round's result calls for. No I/O: the host runs the checks (the
//! entrance probe, the API host's direct request and its capture, the
//! proxied path) and classifies each round; desktop's enhanced mode and the
//! mobile tunnel host (#448) both go by it.
//!
//! - A path failure that is no leak is rechecked after `recheck`, and
//!   recovered from only when the recheck and the next round fail too: a
//!   lost probe on a lossy link is not a broken tunnel.
//! - After a network change, path failures are the network settling for
//!   `network_grace`, rechecked every `network_recheck`.
//! - When only the API host is unreachable on the direct path, the tunnel
//!   works: no recovery, and the checks back off to `api_recheck_max`.
//! - Traffic that bypasses the tunnel (a leak) is recovered from at once.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

/// The pauses of the policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HealthTiming {
    /// Before the quick recheck of a round that failed on the path.
    pub recheck: Duration,
    /// Between checks while the network settles after a change.
    pub network_recheck: Duration,
    /// How long after a network change a path failure is put down to it.
    pub network_grace: Duration,
    /// The longest pause while only the API host is unreachable.
    pub api_recheck_max: Duration,
}

impl Default for HealthTiming {
    fn default() -> Self {
        Self {
            recheck: Duration::from_secs(5),
            network_recheck: Duration::from_secs(2),
            network_grace: Duration::from_secs(30),
            api_recheck_max: Duration::from_secs(5 * 60),
        }
    }
}

/// What one round of checks found, as the host classifies it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Round {
    Healthy,
    /// The tunnel's own checks passed; only the API host's direct request
    /// failed.
    ApiUnreachable,
    /// A failure on the network path (the entrance, the proxied path) that
    /// is no leak.
    PathFailed,
    /// Anything else: a leak (traffic bypassing the tunnel, its routing
    /// broken), the core not serving, another revision.
    Failed,
}

/// What a round calls for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Healthy. `api_cleared`: the API host is reachable again, after
    /// rounds where it was not.
    Healthy { api_cleared: bool },
    /// Stay on and say so; a healthy round for the tunnel. `misses`: such
    /// rounds in a row.
    ApiUnreachable { misses: u32 },
    /// Check again after [`Health::interval`] before recovering.
    Recheck,
    /// Recover.
    Recover,
}

/// A path failure on its way to recovery.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Strike {
    None,
    /// A round failed: rechecked after `recheck`.
    Rechecking,
    /// The recheck failed too: the next round decides.
    Failed,
}

/// One run's health policy: from the core going on until it goes off.
#[derive(Debug, Clone)]
pub struct Health {
    timing: HealthTiming,
    /// Until when a path failure is put down to a recent network change.
    settling_until: Option<Instant>,
    strike: Strike,
    /// Rounds in a row where only the API host was unreachable.
    api_misses: u32,
}

impl Health {
    pub fn new(timing: HealthTiming) -> Self {
        Self {
            timing,
            settling_until: None,
            strike: Strike::None,
            api_misses: 0,
        }
    }

    /// The OS reported a network change at `now`.
    pub fn network_changed(&mut self, now: Instant) {
        self.settling_until = Some(now + self.timing.network_grace);
    }

    /// Whether the network is settling after a change.
    pub fn settling(&self, now: Instant) -> bool {
        self.settling_until.is_some_and(|until| now < until)
    }

    /// The pause before the next round, `base` being the host's interval.
    pub fn interval(&self, base: Duration, now: Instant) -> Duration {
        if self.settling(now) {
            self.timing.network_recheck
        } else if self.strike == Strike::Rechecking {
            self.timing.recheck
        } else if self.api_misses > 0 {
            api_recheck(base, self.api_misses, self.timing.api_recheck_max)
        } else {
            base
        }
    }

    /// Whether `round` falls in the network's grace: a path failure while
    /// the network settles is checked again without counting (the host
    /// waits [`Health::interval`]). A healthy round ends the grace.
    pub fn in_grace(&mut self, round: Round, now: Instant) -> bool {
        match round {
            Round::Healthy => {
                self.settling_until = None;
                false
            }
            Round::ApiUnreachable | Round::PathFailed => self.settling(now),
            Round::Failed => false,
        }
    }

    /// What `round` calls for, outside the network's grace.
    pub fn judge(&mut self, round: Round) -> Verdict {
        match round {
            Round::Healthy => {
                self.strike = Strike::None;
                let api_cleared = self.api_misses > 0;
                self.api_misses = 0;
                Verdict::Healthy { api_cleared }
            }
            Round::ApiUnreachable => {
                self.strike = Strike::None;
                self.api_misses = self.api_misses.saturating_add(1);
                Verdict::ApiUnreachable {
                    misses: self.api_misses,
                }
            }
            Round::PathFailed if self.strike != Strike::Failed => {
                self.strike = match self.strike {
                    Strike::None => Strike::Rechecking,
                    _ => Strike::Failed,
                };
                Verdict::Recheck
            }
            Round::PathFailed | Round::Failed => Verdict::Recover,
        }
    }
}

/// The pause after `misses` rounds in a row where only the API host was
/// unreachable: `interval` doubled per round after the first, up to `max`
/// (never below `interval`).
pub fn api_recheck(interval: Duration, misses: u32, max: Duration) -> Duration {
    interval
        .saturating_mul(1u32 << misses.saturating_sub(1).min(16))
        .min(max.max(interval))
}

/// One automatic reconnect per episode; the episode ends after two
/// consecutive healthy checks or an explicit user action.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RecoveryEpisode {
    used: bool,
    stable_checks: u8,
}

impl RecoveryEpisode {
    pub fn reset(&mut self) {
        *self = Self::default();
    }

    pub fn note_health_success(&mut self) {
        self.stable_checks = self.stable_checks.saturating_add(1);
        if self.stable_checks >= 2 {
            self.used = false;
        }
    }

    pub fn claim(&mut self) -> bool {
        self.stable_checks = 0;
        if self.used {
            return false;
        }
        self.used = true;
        true
    }
}

/// Automatic reconnects for TUN routing another program changed (another
/// VPN taking the routes): at most this many in any [`TUN_ROUTING_WINDOW`]
/// (docs/host-integration.md, `Degraded{TunRoutingBroken}`). Past that the
/// host stops fighting the other program and asks the user.
pub const TUN_ROUTING_RECONNECTS: usize = 3;
pub const TUN_ROUTING_WINDOW: Duration = Duration::from_secs(10 * 60);

/// Automatic reconnects because another program changed the TUN's routing,
/// at most [`TUN_ROUTING_RECONNECTS`] in any [`TUN_ROUTING_WINDOW`]. The
/// routing stays broken until the next start, so each one is a reconnect;
/// another VPN that keeps taking the routes back would otherwise be fought
/// forever. A user's connect, retry or take-over starts afresh.
#[derive(Debug, Default)]
pub struct RoutingReconnects {
    at: VecDeque<Instant>,
}

impl RoutingReconnects {
    pub fn reset(&mut self) {
        self.at.clear();
    }

    /// Takes one reconnect at `now`; `false` when the window's are used up.
    pub fn claim(&mut self, now: Instant) -> bool {
        while self
            .at
            .front()
            .is_some_and(|at| now.saturating_duration_since(*at) >= TUN_ROUTING_WINDOW)
        {
            self.at.pop_front();
        }
        if self.at.len() >= TUN_ROUTING_RECONNECTS {
            return false;
        }
        self.at.push_back(now);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BASE: Duration = Duration::from_secs(30);

    fn health() -> Health {
        Health::new(HealthTiming::default())
    }

    #[test]
    fn a_path_failure_recovers_after_the_recheck_and_the_next_round() {
        let now = Instant::now();
        let mut h = health();
        assert_eq!(h.interval(BASE, now), BASE);
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(h.interval(BASE, now), Duration::from_secs(5), "the recheck");
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(h.interval(BASE, now), BASE, "the next round decides");
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recover);
    }

    #[test]
    fn a_healthy_round_forgets_the_strikes() {
        let mut h = health();
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(
            h.judge(Round::Healthy),
            Verdict::Healthy { api_cleared: false }
        );
        // One lost probe in every few rounds never recovers (#438).
        for _ in 0..12 {
            assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
            assert_eq!(
                h.judge(Round::Healthy),
                Verdict::Healthy { api_cleared: false }
            );
            assert_eq!(
                h.judge(Round::Healthy),
                Verdict::Healthy { api_cleared: false }
            );
        }
    }

    #[test]
    fn other_failures_recover_at_once() {
        let mut h = health();
        assert_eq!(h.judge(Round::Failed), Verdict::Recover);
        // A leak in the middle of a strike too.
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(h.judge(Round::Failed), Verdict::Recover);
    }

    #[test]
    fn an_unreachable_api_host_stays_on_and_backs_off() {
        let now = Instant::now();
        let mut h = health();
        let mut pauses = Vec::new();
        for misses in 1..=6 {
            assert_eq!(
                h.judge(Round::ApiUnreachable),
                Verdict::ApiUnreachable { misses }
            );
            pauses.push(h.interval(BASE, now).as_secs());
        }
        assert_eq!(pauses, [30, 60, 120, 240, 300, 300]);
        assert_eq!(
            h.judge(Round::Healthy),
            Verdict::Healthy { api_cleared: true }
        );
        assert_eq!(h.interval(BASE, now), BASE);
        assert_eq!(
            h.judge(Round::Healthy),
            Verdict::Healthy { api_cleared: false }
        );
    }

    #[test]
    fn an_unreachable_api_host_ends_a_strike() {
        let mut h = health();
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
        assert_eq!(
            h.judge(Round::ApiUnreachable),
            Verdict::ApiUnreachable { misses: 1 }
        );
        assert_eq!(h.judge(Round::PathFailed), Verdict::Recheck);
    }

    #[test]
    fn path_failures_after_a_network_change_are_the_network_settling() {
        let now = Instant::now();
        let mut h = health();
        h.network_changed(now);
        assert!(h.settling(now));
        assert_eq!(h.interval(BASE, now), Duration::from_secs(2));
        assert!(h.in_grace(Round::PathFailed, now));
        assert!(h.in_grace(Round::ApiUnreachable, now));
        // A leak gets no grace.
        assert!(!h.in_grace(Round::Failed, now));
        // Past the grace, failures count again.
        let later = now + Duration::from_secs(30);
        assert!(!h.settling(later));
        assert!(!h.in_grace(Round::PathFailed, later));
        assert_eq!(h.interval(BASE, later), BASE);
        // A healthy round ends the grace early.
        h.network_changed(now);
        assert!(!h.in_grace(Round::Healthy, now));
        assert!(!h.settling(now));
        assert!(!h.in_grace(Round::PathFailed, now));
    }

    #[test]
    fn api_rechecks_never_go_below_the_interval() {
        let max = Duration::from_secs(300);
        let delays: Vec<u64> = (1..=6)
            .map(|misses| api_recheck(BASE, misses, max).as_secs())
            .collect();
        assert_eq!(delays, [30, 60, 120, 240, 300, 300]);
        let long = Duration::from_secs(600);
        assert_eq!(api_recheck(long, 3, max), long);
        assert_eq!(api_recheck(BASE, u32::MAX, max), max);
    }

    #[test]
    fn automatic_recovery_is_bounded_until_two_stable_health_checks() {
        let mut episode = RecoveryEpisode::default();
        assert!(episode.claim());
        assert!(!episode.claim());
        episode.note_health_success();
        assert!(
            !episode.claim(),
            "one healthy check is not enough (and claim resets the streak)"
        );
        episode.note_health_success();
        episode.note_health_success();
        assert!(episode.claim());
        episode.reset();
        assert!(episode.claim());
    }

    #[test]
    fn tun_routing_reconnects_are_bounded_per_rolling_window() {
        let start = Instant::now();
        let at = |seconds: u64| start + Duration::from_secs(seconds);
        let mut budget = RoutingReconnects::default();
        assert!(budget.claim(at(0)));
        assert!(budget.claim(at(60)));
        assert!(budget.claim(at(120)));
        assert!(!budget.claim(at(180)), "a fourth within 10 minutes");
        assert!(!budget.claim(at(599)));
        // The first one left the window: one more.
        assert!(budget.claim(at(600)));
        assert!(!budget.claim(at(601)));
        // A user's retry starts afresh.
        budget.reset();
        assert!(budget.claim(at(602)));
        assert!(budget.claim(at(602)));
        assert!(budget.claim(at(602)));
        assert!(!budget.claim(at(602)));
    }
}
