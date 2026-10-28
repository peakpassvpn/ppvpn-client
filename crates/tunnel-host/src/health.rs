//! The health check of the running engine, in the tunnel process: the UI
//! process is suspended in the background, when a tunnel breaks most often
//! (locked screen, a network change). The policy is desktop's
//! (`ppvpn_engine_host::health`, #438); a round here is the engine serving
//! with a selected node whose entrance answers. Desktop's direct request,
//! capture check and proxied path need a local proxy and a path outside the
//! tunnel, which a mobile tunnel has neither of.
//!
//! - **Recover:** stop and start the engine in process (the TUN stays,
//!   the system's VPN does not blink), once per episode; the episode ends
//!   after two healthy rounds or a user's start.
//! - **Give up:** a second recovery in an episode, or the engine `Fatal`
//!   (it cannot heal, and the profile lives only in the UI process): the
//!   native side is told through `on_exit` and ends the tunnel, so the
//!   device is not left behind a dead one.
//! - **Offline:** no rounds while the native side says the device is
//!   offline: failures there say nothing about the tunnel.
//!
//! Each step is a `health` frame on the watch connections:
//! `{"kind": "health", "state": "healthy" | "settling" | "rechecking" |
//! "recovering" | "failed", "detail"?}`.

use std::time::Duration;

use ppvpn_core::{EngineState, EventItem, EventKind, ProbeEntrancesRequest, ProbeMethod};
use ppvpn_engine_host::health::{Health, HealthTiming, RecoveryEpisode, Round, Verdict};
use serde_json::json;
use tokio::sync::mpsc;
use tokio::time::Instant;

use crate::host::{ExitCallback, Shared, Signal};

/// Between rounds, as desktop's enhanced mode.
const INTERVAL: Duration = Duration::from_secs(30);
const ENTRANCE_TIMEOUT_MS: u64 = 5_000;
/// While the network settles: a quicker answer.
const SETTLING_ENTRANCE_TIMEOUT_MS: u64 = 2_000;

pub(crate) async fn run(
    shared: Shared,
    mut signals: mpsc::UnboundedReceiver<Signal>,
    mut exit: Option<ExitCallback>,
) {
    let mut shutdown = shared.shutdown.clone();
    let mut states = shared.engine.subscribe(&[EventKind::StateChanged]);
    let mut health = Health::new(HealthTiming::default());
    let mut episode = RecoveryEpisode::default();
    let mut next = Instant::now() + INTERVAL;
    loop {
        tokio::select! {
            _ = tokio::time::sleep_until(next) => {}
            signal = signals.recv() => {
                match signal {
                    Some(Signal::Network) => health.network_changed(std::time::Instant::now()),
                    Some(Signal::UserStart) => {
                        episode.reset();
                        health = Health::new(HealthTiming::default());
                    }
                    None => return,
                }
                next = Instant::now() + health.interval(INTERVAL, std::time::Instant::now());
                continue;
            }
            item = states.recv() => {
                let state = match item {
                    Some(EventItem::Event { .. } | EventItem::Lagged { .. }) => shared.engine.status().state,
                    Some(_) => continue,
                    None => return,
                };
                if let EngineState::Fatal { reason } = state {
                    let reason = serde_json::to_string(&reason).unwrap_or_default();
                    give_up(&shared, &mut exit, "TUNNEL_ENGINE_FATAL", &reason);
                    return;
                }
                continue;
            }
            _ = shutdown.changed() => return,
        }

        let now = std::time::Instant::now();
        let round = round(&shared, health.settling(now)).await;
        next = Instant::now() + health.interval(INTERVAL, std::time::Instant::now());
        let Some(round) = round else {
            continue;
        };
        if health.in_grace(round, now) {
            publish(&shared, "settling", None);
            next = Instant::now() + health.interval(INTERVAL, std::time::Instant::now());
            continue;
        }
        match health.judge(round) {
            Verdict::Healthy { .. } | Verdict::ApiUnreachable { .. } => {
                episode.note_health_success();
                publish(&shared, "healthy", None);
            }
            Verdict::Recheck => publish(&shared, "rechecking", None),
            Verdict::Recover if episode.claim() => {
                publish(&shared, "recovering", None);
                shared.log.line("health: restarting the engine");
                let restarted = async {
                    shared.engine.stop().await?;
                    shared.engine.start().await
                }
                .await;
                if let Err(error) = restarted {
                    let detail = format!("{}: {}", error.code, error.message);
                    give_up(&shared, &mut exit, "TUNNEL_HEALTH_FAILED", &detail);
                    return;
                }
                health = Health::new(HealthTiming::default());
            }
            Verdict::Recover => {
                give_up(
                    &shared,
                    &mut exit,
                    "TUNNEL_HEALTH_FAILED",
                    "the tunnel failed again after a restart",
                );
                return;
            }
        }
        next = Instant::now() + health.interval(INTERVAL, std::time::Instant::now());
    }
}

/// One round; `None` when there is nothing to check (not started, or the
/// device offline).
async fn round(shared: &Shared, settling: bool) -> Option<Round> {
    if crate::host::offline() {
        return None;
    }
    let status = shared.engine.status();
    match status.state {
        EngineState::Running | EngineState::Degraded { .. } => {}
        _ => return None,
    }
    let Some(node_id) = status.selected_node_id else {
        return Some(Round::Failed);
    };
    let timeout = if settling {
        SETTLING_ENTRANCE_TIMEOUT_MS
    } else {
        ENTRANCE_TIMEOUT_MS
    };
    let request =
        ProbeEntrancesRequest::new(ProbeMethod::Tcp, timeout, 4).with_node_ids(vec![node_id]);
    Some(match shared.engine.probe_entrances(request).await {
        Ok(results) if results.first().is_some_and(|result| result.success) => Round::Healthy,
        _ => Round::PathFailed,
    })
}

fn publish(shared: &Shared, state: &str, detail: Option<&str>) {
    let mut frame = json!({ "kind": "health", "state": state });
    if let Some(detail) = detail {
        frame["detail"] = json!(detail);
    }
    let _ = shared.health.send(frame);
}

fn give_up(shared: &Shared, exit: &mut Option<ExitCallback>, code: &str, detail: &str) {
    shared.log.line(&format!("giving up: {code}: {detail}"));
    publish(shared, "failed", Some(format!("{code}: {detail}").as_str()));
    if let Some(exit) = exit.take() {
        exit.call(code, detail);
    }
}
