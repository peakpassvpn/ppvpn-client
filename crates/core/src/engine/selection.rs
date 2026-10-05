//! select_node and pin_ingress (docs/host-integration.md, 4.3), and the
//! node queries built from the applied profile. Both calls change what is
//! applied (the next apply's dedupe compares against it) and, while
//! running, sail's groups: the `selected` selector, a node's selector.

use super::state::Applied;
use super::{not_applied, now, Error, Inner};
use crate::error::codes;
use crate::event::Event;
use crate::profile::Node;
use crate::translate::SELECTED_TAG;
use crate::types::{IngressInfo, NodeInfo};

impl Inner {
    pub(super) async fn select_node(&self, node_id: &str) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        let (running, tag) = {
            let live = self.live();
            let applied = live.applied.as_ref().ok_or_else(not_applied)?;
            // D2: an unknown node is NODE_NOT_FOUND, as pin_ingress.
            applied.node(node_id).ok_or_else(node_not_found)?;
            let tag = applied.translation.node_tags.get(node_id).cloned();
            (live.running, tag)
        };
        if running {
            if let Some(tag) = tag {
                if let Err(e) = self.runtime.select(SELECTED_TAG, &tag).await {
                    return Err(self.runtime_error(&e));
                }
            }
        }
        {
            let mut live = self.live();
            let Some(applied) = live.applied.as_mut() else {
                return Err(not_applied());
            };
            applied.selected = node_id.to_owned();
            let revision = applied.profile.revision.clone();
            self.publish(Event::NodeSelected {
                at: now(),
                revision,
                node_id: node_id.to_owned(),
            });
        }
        self.refresh().await;
        Ok(())
    }

    pub(super) async fn pin_ingress(
        &self,
        node_id: &str,
        endpoint_key: Option<&str>,
    ) -> Result<(), Error> {
        self.admit()?;
        let _op = self.op.lock().await;
        self.admit()?;
        let (running, member) = {
            let live = self.live();
            let applied = live.applied.as_ref().ok_or_else(not_applied)?;
            let node = applied.node(node_id).ok_or_else(node_not_found)?;
            if let Some(key) = endpoint_key {
                if !node.ingresses.iter().any(|i| i.endpoint_key == key) {
                    return Err(Error::invalid(
                        codes::INGRESS_NOT_FOUND,
                        "endpoint_key",
                        "the node has no ingress with this endpoint_key",
                    ));
                }
            }
            (live.running, group_member(applied, node_id, endpoint_key))
        };
        // A single-ingress node has no group: the pin is only kept.
        if let (true, Some((selector, member))) = (running, member) {
            if let Err(e) = self.runtime.select(&selector, &member).await {
                return Err(self.runtime_error(&e));
            }
        }
        {
            let mut live = self.live();
            let Some(applied) = live.applied.as_mut() else {
                return Err(not_applied());
            };
            match endpoint_key {
                Some(key) => applied.pins.insert(node_id.to_owned(), key.to_owned()),
                None => applied.pins.remove(node_id),
            };
            self.publish(Event::NodeIngressPinned {
                at: now(),
                node_id: node_id.to_owned(),
                endpoint_key: endpoint_key.unwrap_or_default().to_owned(),
            });
        }
        self.sync_pinned_checks();
        self.refresh().await;
        Ok(())
    }
}

/// A multi-ingress node's selector and the member a pin selects: the
/// ingress, or the fallback group (automatic). None for a single-ingress
/// node.
fn group_member(
    applied: &Applied,
    node_id: &str,
    endpoint_key: Option<&str>,
) -> Option<(String, String)> {
    let t = &applied.translation;
    let selector = t.node_tags.get(node_id)?;
    let auto = t.groups.get(selector)?;
    let member = match endpoint_key {
        None => auto.clone(),
        Some(key) => t
            .members
            .get(selector)?
            .iter()
            .find(|m| t.ingress_keys.get(*m).map(String::as_str) == Some(key))?
            .clone(),
    };
    Some((selector.clone(), member))
}

fn node_not_found() -> Error {
    Error::invalid(
        codes::NODE_NOT_FOUND,
        "node_id",
        "the profile has no node with this id",
    )
}

/// A node as `list-nodes` shows it: no credentials, no addresses.
pub(super) fn node_info(node: &Node) -> NodeInfo {
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
