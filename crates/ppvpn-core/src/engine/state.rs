//! What an instance holds between calls, the state machine
//! (docs/host-integration.md, section 5) and the snapshot built from it.
//! Held under a std mutex for short, synchronous sections only: queries
//! never wait on a lifecycle call.

use std::collections::{BTreeMap, HashMap};

use chrono::{DateTime, Utc};

use crate::profile::{Node, Profile};
use crate::request::RoutingMode;
use crate::runtime::{GroupInfo, RuntimeConnection, RuntimeTraffic};
use crate::status::{
    DegradedReason, EngineState, FatalReason, IngressHealth, IngressStatus, NodeStatus, TunRouting,
};
use crate::translate::Translation;

/// The applied profile and the host's choices on top of it: what the
/// apply dedupe compares and what every rebuild translates.
#[derive(Debug, Clone)]
pub(crate) struct Applied {
    pub profile: Profile,
    pub mode: RoutingMode,
    /// Always a node of `profile`.
    pub selected: String,
    /// node id → endpoint_key, each an ingress of `profile`.
    pub pins: BTreeMap<String, String>,
    /// The tag maps of the last translation (they do not depend on the
    /// selection or the pins).
    pub translation: Translation,
}

impl Applied {
    pub(crate) fn node(&self, id: &str) -> Option<&Node> {
        self.profile.nodes.iter().find(|n| n.id == id)
    }
}

/// What the TUN routing guard reports (Linux tunrules; macOS and Windows
/// later). Its source is the guard module (separate PR).
#[allow(dead_code)] // sent by the guard, not wired yet
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum TunRoutingSignal {
    /// Routing was deleted and is being put back: Degraded.
    Restoring,
    /// The guard did not start; routing is as installed: Degraded.
    Unguarded,
    /// Back in place: out of both Degraded reasons.
    Restored { missing: Vec<String> },
    /// Could not be put back; traffic may bypass the TUN: Fatal.
    Broken { missing: Vec<String>, error: String },
}

/// The last failover switch of a node's group.
#[derive(Debug, Clone)]
pub(crate) struct Switched {
    pub previous_endpoint_key: String,
    pub at: DateTime<Utc>,
}

#[derive(Debug, Default)]
pub(crate) struct Live {
    pub shut_down: bool,
    pub fatal: Option<FatalReason>,
    pub applied: Option<Applied>,
    pub running: bool,
    /// Bumped on every start and stop: a refresh that read the runtime of
    /// an earlier run is dropped.
    pub run: u64,
    /// No default interface (sail's network events, E1b).
    pub offline: bool,
    /// A full restart is under way: sail's states in between (a start that
    /// fails and is put back) are not the instance's.
    pub restarting: bool,
    /// The TUN's routing when not in place (None: ok).
    pub tun_routing: Option<TunRouting>,
    /// This run goes without the shared local proxy listener.
    pub local_proxy_unavailable: bool,
    /// The state last reported (StateChanged).
    pub state: EngineState,
    /// While running: sail's groups by tag, as last read.
    pub groups: HashMap<String, GroupInfo>,
    /// node id → its last ingress switch, while running.
    pub switched: HashMap<String, Switched>,
    /// The runtime's counters as last read, and when; kept after a stop.
    pub traffic: RuntimeTraffic,
    pub traffic_at: Option<DateTime<Utc>>,
    /// While running: the connections as last read.
    pub connections: Vec<RuntimeConnection>,
}

impl Live {
    /// The state these facts make.
    pub(crate) fn state_now(&self) -> EngineState {
        if self.shut_down {
            return EngineState::Stopped;
        }
        if let Some(reason) = &self.fatal {
            return EngineState::Fatal {
                reason: reason.clone(),
            };
        }
        if self.applied.is_none() {
            return EngineState::Stopped;
        }
        if !self.running {
            return EngineState::Configured;
        }
        let mut reasons = Vec::new();
        if self.offline {
            reasons.push(DegradedReason::NoDefaultInterface);
        }
        match self.tun_routing {
            Some(TunRouting::Restoring) => reasons.push(DegradedReason::TunRoutingRestoring),
            Some(TunRouting::Unguarded) => reasons.push(DegradedReason::TunRoutingUnguarded),
            _ => {}
        }
        if self.local_proxy_unavailable {
            reasons.push(DegradedReason::LocalProxyUnavailable);
        }
        if reasons.is_empty() {
            EngineState::Running
        } else {
            EngineState::Degraded { reasons }
        }
    }

    /// Forgets what only a running runtime knows.
    pub(crate) fn clear_runtime(&mut self) {
        self.groups.clear();
        self.connections.clear();
        self.tun_routing = None;
        self.local_proxy_unavailable = false;
        self.switched.clear();
    }

    /// The ingress tag a multi-ingress node uses now: the pinned member, or
    /// the fallback group's. None for a single-ingress node or unknown.
    fn current_member(&self, applied: &Applied, node_tag: &str) -> Option<String> {
        let auto = applied.translation.groups.get(node_tag)?;
        let selector = self.groups.get(node_tag)?;
        if selector.now == *auto {
            self.groups.get(auto).map(|g| g.now.clone())
        } else {
            Some(selector.now.clone())
        }
    }

    /// Every node's pin and, while running, its ingresses' health
    /// (`get-status.nodes`, as Go's nodeStatusesLocked): health only for a
    /// multi-ingress node's failover members; a member sail has not checked
    /// yet counts as healthy, as Go's failover starts.
    pub(crate) fn node_statuses(&self) -> Vec<NodeStatus> {
        let Some(applied) = &self.applied else {
            return Vec::new();
        };
        let t = &applied.translation;
        applied
            .profile
            .nodes
            .iter()
            .map(|node| {
                let tag = t.node_tags.get(&node.id).cloned().unwrap_or_default();
                let fallback = t.groups.get(&tag).and_then(|auto| self.groups.get(auto));
                let current = if self.running {
                    self.current_member(applied, &tag)
                        .and_then(|member| t.ingress_keys.get(&member).cloned())
                } else {
                    None
                };
                let ingresses = node
                    .ingresses
                    .iter()
                    .map(|ingress| {
                        let member = fallback.and_then(|g| {
                            g.members
                                .iter()
                                .find(|m| t.ingress_keys.get(&m.tag) == Some(&ingress.endpoint_key))
                        });
                        IngressHealth {
                            endpoint_key: ingress.endpoint_key.clone(),
                            role: ingress.role.clone(),
                            label: ingress.label.clone().unwrap_or_default(),
                            healthy: member
                                .filter(|_| self.running)
                                .map(|m| m.alive.unwrap_or(true)),
                            last_check_at: member
                                .filter(|_| self.running)
                                .and_then(|m| m.last_check)
                                .map(DateTime::<Utc>::from),
                            consecutive_failures: member
                                .filter(|_| self.running)
                                .map_or(0, |m| m.consecutive_failures),
                            active: current.as_deref() == Some(ingress.endpoint_key.as_str()),
                        }
                    })
                    .collect();
                NodeStatus {
                    node_id: node.id.clone(),
                    pinned_endpoint_key: applied.pins.get(&node.id).cloned(),
                    ingresses,
                }
            })
            .collect()
    }

    /// The ingress the selected node uses, while running
    /// (`get-status.selected_ingress`).
    pub(crate) fn selected_ingress(&self) -> Option<IngressStatus> {
        if !self.running {
            return None;
        }
        let applied = self.applied.as_ref()?;
        let node = applied.node(&applied.selected)?;
        let t = &applied.translation;
        let key = match node.ingresses.as_slice() {
            [only] => only.endpoint_key.clone(),
            _ => {
                let tag = t.node_tags.get(&node.id)?;
                let member = self.current_member(applied, tag)?;
                t.ingress_keys.get(&member)?.clone()
            }
        };
        let ingress = node.ingresses.iter().find(|i| i.endpoint_key == key)?;
        let switched = self.switched.get(&node.id);
        Some(IngressStatus {
            endpoint_key: key.clone(),
            label: ingress.label.clone().unwrap_or_default(),
            previous_endpoint_key: switched
                .map(|s| s.previous_endpoint_key.clone())
                .unwrap_or_default(),
            role: ingress.role.clone(),
            switched_at: switched.map(|s| s.at),
        })
    }
}
