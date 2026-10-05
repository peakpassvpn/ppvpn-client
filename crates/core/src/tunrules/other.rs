//! Only Linux routes the TUN by policy rules; elsewhere there is nothing
//! here to guard (macOS and Windows get their own routing checks, #214 N2).

use std::io;

use tokio::sync::watch;

use super::{Scope, TunRoutingStatus};

pub(crate) struct Guard;

impl Guard {
    pub(crate) fn start(_scope: Scope) -> (Guard, watch::Receiver<TunRoutingStatus>) {
        let (_, statuses) = watch::channel(TunRoutingStatus::Unguarded {
            error: "no policy routing to guard on this platform".into(),
        });
        (Guard, statuses)
    }

    pub(crate) fn check(&self, _reason: &str) {}

    pub(crate) fn stop(self) {}
}

pub(crate) fn sweep(_scope: &Scope) -> io::Result<usize> {
    Ok(0)
}
