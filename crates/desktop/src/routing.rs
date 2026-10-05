//! The routing mode (rules / global): a per-device choice, persisted in
//! `client-settings.json` next to the connection mode and never synced. The
//! cores get it with every `apply-profile`; switching it
//! re-applies the running profile without a reconnect.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::RoutingMode;

/// `routing_mode` in the core's `apply-profile`.
pub(crate) fn wire_name(mode: RoutingMode) -> &'static str {
    match mode {
        RoutingMode::Rules => "rules",
        RoutingMode::Global => "global",
    }
}

pub(crate) fn parse(name: Option<&str>) -> RoutingMode {
    match name {
        Some("global") => RoutingMode::Global,
        _ => RoutingMode::Rules,
    }
}

/// The current mode, shared by the client and both cores.
#[derive(Clone, Debug, Default)]
pub(crate) struct RoutingModeCell(Arc<AtomicBool>);

impl RoutingModeCell {
    pub(crate) fn new(mode: RoutingMode) -> Self {
        let cell = Self::default();
        cell.set(mode);
        cell
    }

    pub(crate) fn get(&self) -> RoutingMode {
        if self.0.load(Ordering::SeqCst) {
            RoutingMode::Global
        } else {
            RoutingMode::Rules
        }
    }

    pub(crate) fn set(&self, mode: RoutingMode) {
        self.0.store(mode == RoutingMode::Global, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_mode_round_trips() {
        assert_eq!(parse(Some("global")), RoutingMode::Global);
        assert_eq!(parse(Some("whatever")), RoutingMode::Rules);
        assert_eq!(parse(None), RoutingMode::Rules);
        let cell = RoutingModeCell::new(RoutingMode::Global);
        assert_eq!(wire_name(cell.get()), "global");
        cell.set(RoutingMode::Rules);
        assert_eq!(wire_name(cell.get()), "rules");
    }
}
