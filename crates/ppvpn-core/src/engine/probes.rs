//! Entrance and availability probes (docs/host-integration.md, 4.5): the
//! Engine's checks around `crate::probe`, and their events.

use super::{not_applied, Error, Inner};
use crate::error::codes;
use crate::probe::{self, Cancel, DefaultInterface, Net};
use crate::types::{
    AvailabilityResult, EntranceResult, ProbeAvailabilityRequest, ProbeEntrancesRequest,
};

impl Inner {
    /// What is known of the default interface. Its source is `on_network`
    /// (sail's network events, E1b), not wired yet: until then it is
    /// Unknown, which probes as Present does; offline is Absent.
    pub(super) fn default_interface(&self) -> DefaultInterface {
        if self.live().offline {
            DefaultInterface::Absent
        } else {
            DefaultInterface::Unknown
        }
    }

    /// Every ingress of the applied profile's nodes (`request.node_ids`:
    /// only those), directly; running or not, as Go.
    pub(super) async fn probe_entrances(
        &self,
        request: ProbeEntrancesRequest,
        net: &dyn Net,
    ) -> Result<Vec<EntranceResult>, Error> {
        let profile = {
            let live = self.live();
            live.applied
                .as_ref()
                .map(|a| a.profile.clone())
                .ok_or_else(not_applied)?
        };
        let (results, events) = probe::probe_entrances(
            &profile,
            &request,
            self.default_interface(),
            net,
            &Cancel::never(),
        )
        .await?;
        for event in events {
            self.publish(event);
        }
        Ok(results)
    }

    /// A GET through the node's outbound, which is where its local proxy
    /// user leads. After the instance's LOCAL_PROXY_DISABLED check, as Go:
    /// PROFILE_NOT_APPLIED, then CORE_NOT_RUNNING.
    pub(super) async fn probe_availability(
        &self,
        request: ProbeAvailabilityRequest,
    ) -> Result<AvailabilityResult, Error> {
        let node_tags = {
            let live = self.live();
            let applied = live.applied.as_ref().ok_or_else(not_applied)?;
            if !live.running {
                return Err(Error::new(
                    codes::CORE_NOT_RUNNING,
                    true,
                    "the instance must be started before probing availability",
                ));
            }
            applied.translation.node_tags.clone()
        };
        let (result, event) = probe::probe_availability(
            self.runtime.as_ref(),
            &node_tags,
            &request,
            self.default_interface(),
            &Cancel::never(),
        )
        .await?;
        self.publish(event);
        Ok(result)
    }
}
