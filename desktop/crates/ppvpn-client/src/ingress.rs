//! Ingress pinning: the user can fix a node to one of its ingresses
//! (`Replica::endpoint_key`) instead of the core's automatic failover.
//!
//! - The pins are this device's choice: persisted in `client-settings.json`
//!   (`ingress_pins`: node id → endpoint key), never synced.
//! - ppvpn-core 0.5.7+ applies a pin live (`/v1/pin-ingress`) and keeps it
//!   across applies of the same process only, so the client sends the pins
//!   after every apply and connect, and the monitor re-sends any pin a core
//!   does not report (`GetStatus.nodes[].pinned_endpoint_key`), e.g. after a
//!   restart. Older cores (and services) refuse the call; that is ignored.
//! - A pinned ingress that fails stays pinned (the core never switches away);
//!   the apps tell the user from `snapshot.node_ingresses` (`healthy`).
//! - A profile refresh without the pinned endpoint key drops the pin (back
//!   to automatic) and lists it in `snapshot.cleared_ingress_pins` until
//!   `Client::dismiss_cleared_ingress_pins`.

use std::collections::BTreeMap;
use std::path::Path;
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::core_ipc::{self, CoreTransport};
use crate::errors::{ClientError, ErrorCode};
use crate::storage::CLIENT_SETTINGS_FILE;
use crate::{Client, IngressPin, Node, NodeIngresses};

/// Node id → pinned endpoint key.
pub(crate) type Pins = BTreeMap<String, String>;

/// `client-settings.json` key.
const SETTINGS_KEY: &str = "ingress_pins";

/// The current pins, shared by the client and both cores.
#[derive(Clone, Debug, Default)]
pub(crate) struct PinsCell(Arc<Mutex<Pins>>);

impl PinsCell {
    pub(crate) fn new(pins: Pins) -> Self {
        Self(Arc::new(Mutex::new(pins)))
    }

    pub(crate) fn get(&self) -> Pins {
        self.0.lock().map(|pins| pins.clone()).unwrap_or_default()
    }

    pub(crate) fn set(&self, pins: Pins) {
        if let Ok(mut current) = self.0.lock() {
            *current = pins;
        }
    }
}

pub(crate) fn records(pins: &Pins) -> Vec<IngressPin> {
    pins.iter()
        .map(|(node_id, endpoint_key)| IngressPin {
            node_id: node_id.clone(),
            endpoint_key: endpoint_key.clone(),
        })
        .collect()
}

/// The persisted pins; none when unset or unreadable.
pub(crate) fn load(data_dir: &str) -> Pins {
    std::fs::read(Path::new(data_dir).join(CLIENT_SETTINGS_FILE))
        .ok()
        .and_then(|raw| serde_json::from_slice::<Value>(&raw).ok())
        .and_then(|settings| settings.get(SETTINGS_KEY).cloned())
        .and_then(|pins| serde_json::from_value::<Pins>(pins).ok())
        .unwrap_or_default()
}

pub(crate) fn save(data_dir: &str, pins: &Pins) -> Result<(), String> {
    crate::connection::save_setting_value(data_dir, SETTINGS_KEY, serde_json::json!(pins))
}

/// Drops the pins whose node is in `nodes` but no longer has the pinned
/// endpoint key; returns them. Pins of nodes not in `nodes` stay (they may
/// belong to another team's profile).
pub(crate) fn prune(pins: &mut Pins, nodes: &[Node]) -> Vec<IngressPin> {
    let mut cleared = Vec::new();
    pins.retain(|node_id, endpoint_key| {
        let gone = nodes.iter().any(|node| {
            &node.id == node_id
                && !node
                    .replicas
                    .iter()
                    .any(|replica| &replica.endpoint_key == endpoint_key)
        });
        if gone {
            cleared.push(IngressPin {
                node_id: node_id.clone(),
                endpoint_key: endpoint_key.clone(),
            });
        }
        !gone
    });
    cleared
}

/// Sends the pins a core does not have yet. `reported` is the core's
/// `GetStatus.nodes` (`None`: unknown, every pin is sent). Nodes the core
/// reports pinned without a pin here are set back to automatic. Best effort:
/// a core or service without `/v1/pin-ingress`, or a node the core does not
/// know, is logged and skipped.
pub(crate) async fn push(
    core: &dyn CoreTransport,
    pins: &Pins,
    reported: Option<&[NodeIngresses]>,
    label: &str,
) {
    let mut wanted: Vec<(String, Option<String>)> = Vec::new();
    match reported {
        None => wanted.extend(
            pins.iter()
                .map(|(node, key)| (node.clone(), Some(key.clone()))),
        ),
        Some(nodes) => {
            for node in nodes {
                let desired = pins.get(&node.node_id);
                if desired != node.pinned_endpoint_key.as_ref() {
                    wanted.push((node.node_id.clone(), desired.cloned()));
                }
            }
        }
    }
    for (node_id, endpoint_key) in wanted {
        pin_one(core, &node_id, endpoint_key.as_deref(), label).await;
    }
}

/// One `/v1/pin-ingress` call, best effort (see [`push`]).
pub(crate) async fn pin_one(
    core: &dyn CoreTransport,
    node_id: &str,
    endpoint_key: Option<&str>,
    label: &str,
) {
    match core_ipc::pin_ingress(core, node_id, endpoint_key).await {
        Ok(()) => tracing::info!(
            %node_id,
            endpoint_key = endpoint_key.unwrap_or("auto"),
            "{label}: ingress pin applied"
        ),
        Err(error) => {
            tracing::debug!(%node_id, "{label}: pin ingress skipped ({})", error.detail())
        }
    }
}

impl Client {
    /// `Client::pin_ingress`: validates against the current profile,
    /// persists, and applies to the running cores.
    pub(crate) async fn set_ingress_pin(
        &self,
        node_id: String,
        endpoint_key: Option<String>,
    ) -> Result<(), ClientError> {
        self.current_session()?;
        let Some(node) = self
            .profile_nodes()
            .into_iter()
            .find(|node| node.id == node_id)
        else {
            return Err(self.report(ClientError::failed(ErrorCode::NodeNotFound, node_id)));
        };
        if let Some(key) = &endpoint_key {
            if !node
                .replicas
                .iter()
                .any(|replica| &replica.endpoint_key == key)
            {
                return Err(self.report(ClientError::failed(
                    ErrorCode::NodeNotFound,
                    format!("INGRESS_NOT_FOUND: {node_id} {key}"),
                )));
            }
        }
        let mut pins = self.ingress_pins.get();
        match &endpoint_key {
            Some(key) => pins.insert(node_id.clone(), key.clone()),
            None => pins.remove(&node_id),
        };
        save(&self.config.data_dir, &pins).map_err(|detail| {
            self.report(ClientError::failed(ErrorCode::LocalStorageFailed, detail))
        })?;
        tracing::info!(
            %node_id,
            endpoint_key = endpoint_key.as_deref().unwrap_or("auto"),
            "ingress pin set"
        );
        self.ingress_pins.set(pins.clone());
        self.update(|snapshot| {
            snapshot.ingress_pins = records(&pins);
            snapshot.last_error = None;
        });
        if let Ok(transport) = self.standard.transport() {
            pin_one(
                transport.as_ref(),
                &node_id,
                endpoint_key.as_deref(),
                "standard core",
            )
            .await;
        }
        self.enhanced
            .pin_ingress_live(&node_id, endpoint_key.as_deref())
            .await;
        Ok(())
    }

    /// Sends the pins to the standard core (after an apply).
    pub(crate) async fn push_standard_pins(&self) {
        if let Ok(transport) = self.standard.transport() {
            push(
                transport.as_ref(),
                &self.ingress_pins.get(),
                None,
                "standard core",
            )
            .await;
        }
    }

    /// A new profile: pins of ingresses it no longer has go back to
    /// automatic and are reported. Called with the new node list.
    pub(crate) fn prune_ingress_pins(&self, nodes: &[Node]) {
        let mut pins = self.ingress_pins.get();
        let cleared = prune(&mut pins, nodes);
        if cleared.is_empty() {
            return;
        }
        if let Err(detail) = save(&self.config.data_dir, &pins) {
            tracing::warn!("persist ingress pins: {detail}");
        }
        for pin in &cleared {
            tracing::info!(
                node_id = %pin.node_id,
                endpoint_key = %pin.endpoint_key,
                "ingress pin cleared: the profile no longer has it"
            );
        }
        self.ingress_pins.set(pins.clone());
        self.update(|snapshot| {
            snapshot.ingress_pins = records(&pins);
            for pin in cleared {
                if !snapshot.cleared_ingress_pins.contains(&pin) {
                    snapshot.cleared_ingress_pins.push(pin);
                }
            }
        });
    }

    pub(crate) fn clear_cleared_ingress_pins(&self) {
        self.update(|snapshot| snapshot.cleared_ingress_pins.clear());
    }

    pub(crate) fn set_node_ingresses(&self, nodes: Vec<NodeIngresses>) {
        let changed = self
            .snapshot
            .lock()
            .map(|snapshot| snapshot.node_ingresses != nodes)
            .unwrap_or(false);
        if changed {
            self.update(|snapshot| snapshot.node_ingresses = nodes);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Replica;

    fn node(id: &str, keys: &[&str]) -> Node {
        Node {
            id: id.into(),
            name: id.into(),
            entry_key: "std".into(),
            entry_label: None,
            exit_region: None,
            exit_country_code: None,
            udp: true,
            replicas: keys
                .iter()
                .enumerate()
                .map(|(ordinal, key)| Replica {
                    endpoint_key: (*key).into(),
                    replica_ordinal: ordinal as u32,
                    protocol: "shadowsocks".into(),
                    label: None,
                })
                .collect(),
        }
    }

    #[test]
    fn pins_of_ingresses_the_profile_dropped_are_cleared() {
        let mut pins = Pins::from([
            ("a".to_string(), "a1".to_string()),
            ("b".to_string(), "b9".to_string()),
            ("other-team".to_string(), "x1".to_string()),
        ]);
        let cleared = prune(&mut pins, &[node("a", &["a1", "a2"]), node("b", &["b1"])]);
        assert_eq!(
            cleared,
            vec![IngressPin {
                node_id: "b".into(),
                endpoint_key: "b9".into()
            }]
        );
        assert_eq!(pins.len(), 2, "a stays, another team's node stays");
    }

    #[test]
    fn pins_persist_next_to_the_other_settings() {
        let dir = std::env::temp_dir().join(format!("ppvpn-pins-{}", uuid::Uuid::new_v4()));
        let dir = dir.to_string_lossy().to_string();
        assert!(load(&dir).is_empty());
        crate::connection::save_routing_mode(&dir, crate::RoutingMode::Global).unwrap();
        save(&dir, &Pins::from([("n1".to_string(), "k2".to_string())])).unwrap();
        assert_eq!(load(&dir).get("n1").map(String::as_str), Some("k2"));
        assert_eq!(
            crate::connection::load_routing_mode(&dir),
            crate::RoutingMode::Global,
            "other keys kept"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }
}
