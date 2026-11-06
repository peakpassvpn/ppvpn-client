//! Dynamic-owner capture translation preparation. No native mutation occurs
//! here; the frozen Sail driver will apply a plan under the Engine lease.
use super::{Error, Inner};
use crate::{error::codes, translate};

impl Inner {
    /// Called while holding op, so profile/selection/pins/DNS cannot change
    /// between plan construction and a native capture transaction.
    #[allow(dead_code)] // Native driver integration waits for frozen Sail capture support.
    pub(super) fn shared_capture_translation(
        &self,
        capture: Option<&crate::capture::CaptureSpec>,
    ) -> Result<translate::Translation, Error> {
        if !self.config.uses_dynamic_capture() {
            return Err(Error::new(
                codes::CORE_OPERATION_FAILED,
                false,
                "capture plans require a dynamic forwarding owner",
            ));
        }
        let applied = self.live().applied.clone().ok_or_else(super::not_applied)?;
        let mut options = self.options(applied.mode, &applied.selected, &applied.pins);
        if let Some(capture) = capture {
            let mut inbound = options.dns_policy.clone().ok_or_else(|| {
                Error::new(
                    codes::CORE_OPERATION_FAILED,
                    false,
                    "shared DNS policy is absent",
                )
            })?;
            inbound.ipv6 = capture.ipv6;
            // Broker/host resources already own interface configuration and
            // routes. Native adoption must not duplicate auto_route setup.
            // Frozen Sail dynamic TUN deliberately rejects auto_route for
            // both host handles and internally created managed devices.
            inbound.desktop = false;
            options.tun = Some(inbound);
        }
        translate::translate(&applied.profile, &options)
    }
}

#[cfg(test)]
mod tests;
