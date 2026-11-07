//! App-side profile state and direct entrance probes, with no forwarding
//! runtime, inbound listener, tunnel, or rule-set download. Build with
//! `default-features = false` to omit Sail and its TLS/transport graph.
use crate::error::codes;
use crate::probe::{self, Cancel, DefaultInterface, SystemNet};
use crate::profile::{Node, Profile};
use crate::{
    ApplyRequest, ApplyResult, ClearedPin, EntranceResult, Error, IngressInfo, NodeInfo, Pin,
    PinClearReason, ProbeEntrancesRequest, RoutingMode,
};
use chrono::Utc;

/// Host network availability; Unknown permits a bounded probe attempt.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ProbeNetwork {
    #[default]
    Unknown,
    Online,
    Offline,
}

/// Cancellation shared by the host and one or more direct probe requests.
pub struct ProbeCancellation {
    canceller: probe::Canceller,
    cancel: Cancel,
}
impl Default for ProbeCancellation {
    fn default() -> Self {
        let (canceller, cancel) = Cancel::new();
        Self { canceller, cancel }
    }
}
impl ProbeCancellation {
    pub fn cancel(&self) {
        self.canceller.cancel();
    }
}

/// Validated, memory-only control state. Contains profile credentials; never
/// log it. Persistence and transfer to the extension are the host's job.
/// This type cannot start a forwarding runtime and has no Engine handle.
pub struct ProfileSession {
    profile: Profile,
    request: ApplyRequest,
    socket_protect: Option<crate::SocketProtect>,
}

impl ProfileSession {
    /// Validates all profile/rule-set references and host pins with the same
    /// request validator used by Engine. Missing nodes/ingresses reset like
    /// Engine::apply. No rule sets are downloaded or forwarding started.
    pub fn prepare(mut request: ApplyRequest) -> Result<(Self, ApplyResult), Error> {
        let profile = crate::request::validate_request(&request, Utc::now())?;
        let (selected, selection_reset) = match request.selected_node_id.as_deref() {
            Some(id) if profile.nodes.iter().any(|n| n.id == id) => (id.to_owned(), false),
            requested => (
                profile.selection.default_node_id.clone(),
                requested.is_some(),
            ),
        };
        let mut cleared_pins = Vec::new();
        request.pins.retain(|pin| {
            let reason = match profile.nodes.iter().find(|n| n.id == pin.node_id) {
                None => Some(PinClearReason::NodeRemoved),
                Some(node)
                    if !node
                        .ingresses
                        .iter()
                        .any(|i| i.endpoint_key == pin.endpoint_key) =>
                {
                    Some(PinClearReason::IngressRemoved)
                }
                Some(_) => None,
            };
            if let Some(reason) = reason {
                cleared_pins.push(ClearedPin {
                    node_id: pin.node_id.clone(),
                    endpoint_key: pin.endpoint_key.clone(),
                    reason,
                });
                false
            } else {
                true
            }
        });
        request.pins.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        request.selected_node_id = Some(selected.clone());
        let result = ApplyResult {
            applied: true,
            revision: profile.revision.clone(),
            selected_node_id: selected,
            selection_reset,
            cleared_pins,
            switch: None,
            listeners: Vec::new(),
        };
        Ok((
            Self {
                profile,
                request,
                socket_protect: None,
            },
            result,
        ))
    }

    /// Full canonical request, including unchanged profile rules and secrets,
    /// for host-private persistence or transfer to the VPN extension.
    pub fn apply_request(&self) -> ApplyRequest {
        self.request.clone()
    }
    /// Android hosts may keep each direct probe socket outside their VPN.
    /// The callback runs before connect/send and does not require Sail.
    pub fn with_socket_protect(mut self, protect: crate::SocketProtect) -> Self {
        self.socket_protect = Some(protect);
        self
    }
    pub fn revision(&self) -> &str {
        &self.profile.revision
    }
    pub fn nodes(&self) -> Vec<NodeInfo> {
        self.profile.nodes.iter().map(node_info).collect()
    }
    pub fn selected_node(&self) -> NodeInfo {
        node_info(
            self.profile
                .nodes
                .iter()
                .find(|n| Some(&n.id) == self.request.selected_node_id.as_ref())
                .expect("validated selection"),
        )
    }
    pub fn select_node(&mut self, node_id: &str) -> Result<NodeInfo, Error> {
        let node = self
            .profile
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .ok_or_else(|| {
                Error::invalid(
                    codes::NODE_NOT_FOUND,
                    "node_id",
                    "the profile has no node with this id",
                )
            })?;
        self.request.selected_node_id = Some(node_id.into());
        Ok(node_info(node))
    }
    pub fn set_routing_mode(&mut self, mode: RoutingMode) {
        self.request.routing_mode = mode;
    }
    pub fn pins(&self) -> &[Pin] {
        &self.request.pins
    }
    pub fn pin_ingress(&mut self, node_id: &str, endpoint_key: &str) -> Result<(), Error> {
        let node = self
            .profile
            .nodes
            .iter()
            .find(|n| n.id == node_id)
            .ok_or_else(|| {
                Error::invalid(
                    codes::NODE_NOT_FOUND,
                    "node_id",
                    "the profile has no node with this id",
                )
            })?;
        if !node
            .ingresses
            .iter()
            .any(|i| i.endpoint_key == endpoint_key)
        {
            return Err(Error::invalid(
                codes::INGRESS_NOT_FOUND,
                "endpoint_key",
                "the node has no ingress with this key",
            ));
        }
        self.request.pins.retain(|p| p.node_id != node_id);
        self.request.pins.push(Pin::new(node_id, endpoint_key));
        self.request.pins.sort_by(|a, b| a.node_id.cmp(&b.node_id));
        Ok(())
    }
    pub fn unpin_ingress(&mut self, node_id: &str) -> Result<(), Error> {
        if !self.profile.nodes.iter().any(|n| n.id == node_id) {
            return Err(Error::invalid(
                codes::NODE_NOT_FOUND,
                "node_id",
                "the profile has no node with this id",
            ));
        }
        self.request.pins.retain(|p| p.node_id != node_id);
        Ok(())
    }

    /// TCP handshake or ICMP echo to ingress addresses. This measures reachability
    /// and entrance latency, not encrypted proxy authentication/HTTP availability
    /// or download throughput. Literal ingress IP takes precedence over DNS.
    pub async fn probe_entrances(
        &self,
        request: ProbeEntrancesRequest,
        network: ProbeNetwork,
        cancellation: &ProbeCancellation,
    ) -> Result<Vec<EntranceResult>, Error> {
        let network = match network {
            ProbeNetwork::Unknown => DefaultInterface::Unknown,
            ProbeNetwork::Online => DefaultInterface::Present,
            ProbeNetwork::Offline => DefaultInterface::Absent,
        };
        let (results, _) = probe::probe_entrances(
            &self.profile,
            &request,
            network,
            &SystemNet::protected(self.socket_protect.clone()),
            &cancellation.cancel,
        )
        .await?;
        Ok(results)
    }
}

pub(crate) fn node_info(node: &Node) -> NodeInfo {
    NodeInfo {
        id: node.id.clone(),
        name: node.name.clone(),
        entry_key: node.entry_key.clone(),
        entry_label: node.entry_label.clone(),
        protocol: node
            .ingresses
            .first()
            .map(|i| i.protocol.clone())
            .unwrap_or_default(),
        region: node.exit.region.clone(),
        tcp: node.capabilities.tcp,
        udp: node.capabilities.udp,
        ingresses: node
            .ingresses
            .iter()
            .map(|i| IngressInfo {
                endpoint_key: i.endpoint_key.clone(),
                label: i.label.clone().unwrap_or_default(),
                replica_ordinal: i.replica_ordinal,
                role: i.role.clone(),
                protocol: i.protocol.clone(),
            })
            .collect(),
    }
}

#[cfg(test)]
mod tests;
