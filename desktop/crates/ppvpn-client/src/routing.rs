//! The routing mode (rules / global): a per-device choice, persisted in
//! `client-settings.json` next to the connection mode and never synced. The
//! cores get it with every `apply-profile` (ppvpn-core 0.5.6+); switching it
//! re-applies the running profile without a reconnect.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use crate::RoutingMode;

/// First ppvpn-core release whose `apply-profile` accepts `routing_mode`.
/// Older cores decode request bodies strictly and reject the field.
const ROUTING_MODE_MIN_CORE: (u64, u64, u64) = (0, 5, 6);

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

/// `core_version` (`GetVersion`) accepts `routing_mode`.
pub(crate) fn core_accepts_routing_mode(core_version: &str) -> bool {
    crate::core_ipc::core_version_at_least(core_version, ROUTING_MODE_MIN_CORE)
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
    fn only_cores_from_0_5_6_get_the_mode() {
        assert!(core_accepts_routing_mode("0.5.6"));
        assert!(core_accepts_routing_mode("v0.6.0-rc.1"));
        assert!(!core_accepts_routing_mode("0.5.5"));
        assert!(!core_accepts_routing_mode("fake"));
        assert_eq!(parse(Some("global")), RoutingMode::Global);
        assert_eq!(parse(Some("whatever")), RoutingMode::Rules);
        assert_eq!(parse(None), RoutingMode::Rules);
        let cell = RoutingModeCell::new(RoutingMode::Global);
        assert_eq!(wire_name(cell.get()), "global");
        cell.set(RoutingMode::Rules);
        assert_eq!(wire_name(cell.get()), "rules");
    }
}
