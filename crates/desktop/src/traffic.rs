//! Once-per-second throughput while signed in: the standard-mode core
//! (system proxy and local proxies) plus the enhanced-mode core when it is on,
//! summed into one [`TrafficSample`] for [`crate::ClientListener::on_traffic`].

use std::time::Duration;

use tokio::time::Instant;

use crate::core_ipc;
use crate::enhanced::TrafficMeter;
use crate::session::ClientRef;
use crate::{Client, TrafficSample};

const TRAFFIC_INTERVAL: Duration = if cfg!(test) {
    Duration::from_millis(50)
} else {
    Duration::from_secs(1)
};

/// Adds one source's cumulative counters to `sample`; a source that is not
/// running resets its meter so a restart starts from a clean baseline.
pub(crate) fn accumulate(
    sample: &mut TrafficSample,
    meter: &mut TrafficMeter,
    totals: Option<(u64, u64)>,
    now: Instant,
) {
    match totals {
        Some((up, down)) => {
            let part = meter.sample(up, down, now);
            sample.up_bps = sample.up_bps.saturating_add(part.up_bps);
            sample.down_bps = sample.down_bps.saturating_add(part.down_bps);
            sample.up_total = sample.up_total.saturating_add(part.up_total);
            sample.down_total = sample.down_total.saturating_add(part.down_total);
        }
        None => *meter = TrafficMeter::default(),
    }
}

impl Client {
    /// Starts the sampler of `session`; it ends with the session (reset
    /// aborts it) or at shutdown.
    pub(crate) fn spawn_traffic_loop(&self, session: u64) {
        let weak = self.this.clone();
        let task = self.runtime.spawn(async move {
            let mut standard_meter = TrafficMeter::default();
            let mut enhanced_meter = TrafficMeter::default();
            loop {
                tokio::time::sleep(TRAFFIC_INTERVAL).await;
                let Some(client) = ClientRef::upgrade(&weak) else {
                    return;
                };
                if !client.session_is(session) || client.is_shut_down() {
                    return;
                }
                let standard = match client.standard.transport() {
                    Ok(transport) => core_ipc::get_traffic(transport.as_ref()).await.ok(),
                    Err(_) => None,
                };
                let enhanced = client.enhanced.traffic_totals().await;
                let now = Instant::now();
                let mut sample = TrafficSample {
                    up_bps: 0,
                    down_bps: 0,
                    up_total: 0,
                    down_total: 0,
                };
                accumulate(&mut sample, &mut standard_meter, standard, now);
                accumulate(&mut sample, &mut enhanced_meter, enhanced, now);
                if !client.session_is(session) {
                    return;
                }
                client.listener.on_traffic(sample);
            }
        });
        let mut state = self.session_state();
        if state.is_session(session) {
            if let Some(previous) = state.traffic_task.replace(task) {
                previous.abort();
            }
        } else {
            task.abort();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn empty() -> TrafficSample {
        TrafficSample {
            up_bps: 0,
            down_bps: 0,
            up_total: 0,
            down_total: 0,
        }
    }

    #[test]
    fn sources_are_summed_and_a_stopped_source_resets() {
        let start = Instant::now();
        let (mut standard, mut enhanced) = (TrafficMeter::default(), TrafficMeter::default());
        let mut first = empty();
        accumulate(&mut first, &mut standard, Some((100, 1_000)), start);
        accumulate(&mut first, &mut enhanced, Some((50, 500)), start);
        assert_eq!((first.up_bps, first.down_bps), (0, 0));
        assert_eq!((first.up_total, first.down_total), (150, 1_500));

        let later = start + Duration::from_secs(1);
        let mut second = empty();
        accumulate(&mut second, &mut standard, Some((300, 2_000)), later);
        accumulate(&mut second, &mut enhanced, Some((150, 700)), later);
        assert_eq!((second.up_bps, second.down_bps), (300, 1_200));

        // Enhanced mode turned off: only the standard core counts.
        let third_at = later + Duration::from_secs(1);
        let mut third = empty();
        accumulate(&mut third, &mut standard, Some((400, 2_100)), third_at);
        accumulate(&mut third, &mut enhanced, None, third_at);
        assert_eq!((third.up_bps, third.down_bps), (100, 100));

        // Back on with fresh counters: the first tick is a baseline.
        let fourth_at = third_at + Duration::from_secs(1);
        let mut fourth = empty();
        accumulate(&mut fourth, &mut enhanced, Some((10, 10)), fourth_at);
        assert_eq!((fourth.up_bps, fourth.down_bps), (0, 0));
    }
}
