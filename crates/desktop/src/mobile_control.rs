//! The mobile App is a control plane. Forwarding belongs exclusively to the
//! system VPN host; even an accidental Standard start must fail closed.

#[cfg(any(test, target_os = "android", target_os = "ios"))]
use crate::core_ipc::BoxFuture;
use crate::errors::{ClientErrorInfo, ErrorCode};
#[cfg(any(test, target_os = "android", target_os = "ios"))]
use crate::standard::{CoreLauncher, Launched};

#[cfg(any(test, target_os = "android", target_os = "ios"))]
pub(crate) struct NoAppEngine;

#[cfg(any(test, target_os = "android", target_os = "ios"))]
impl CoreLauncher for NoAppEngine {
    fn launch(&self) -> BoxFuture<'_, Result<Launched, ClientErrorInfo>> {
        Box::pin(async {
            Err(ClientErrorInfo::new(
                ErrorCode::StandardCoreFailed,
                "MOBILE_APP_FORWARDING_DISABLED",
            ))
        })
    }
}

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, Weak};
use std::time::Duration;

use crate::errors::ClientError;
use crate::{Client, ProbeMethod, ProbeResult};
use ppvpn_core::control::{ProbeCancellation, ProbeNetwork, ProfileSession};

#[derive(Default)]
pub(crate) struct MobileControl {
    network: Mutex<ProbeNetwork>,
    profile: Mutex<Option<(String, Option<String>)>>,
    generation: AtomicU64,
    probes: Mutex<Vec<Weak<ProbeCancellation>>>,
}

impl MobileControl {
    pub(crate) fn cancel(&self) {
        let mut probes = self.probes.lock().unwrap();
        self.generation.fetch_add(1, Ordering::SeqCst);
        for probe in probes.drain(..).filter_map(|p| p.upgrade()) {
            probe.cancel();
        }
    }

    pub(crate) fn profile_changed(&self, revision: &str, team: Option<&str>) {
        let next = (revision.to_string(), team.map(str::to_string));
        let mut profile = self.profile.lock().unwrap();
        if profile.as_ref() == Some(&next) {
            return;
        }
        *profile = Some(next);
        drop(profile);
        self.cancel();
    }

    pub(crate) fn network_changed(&self, state: ppvpn_core::NetworkState) {
        *self.network.lock().unwrap() = if state.interface.is_some() {
            ProbeNetwork::Online
        } else {
            ProbeNetwork::Offline
        };
        self.cancel();
    }
}

impl Client {
    /// Validate and retain profile choices without Engine, Sail, listeners,
    /// state-directory locks or rule-set downloads.
    pub(crate) fn prepare_mobile_profile(&self, raw: &[u8]) -> Result<ProfileSession, ClientError> {
        let value = serde_json::from_slice(raw).map_err(|error| {
            ClientError::failed(
                ErrorCode::ProfileInvalid,
                format!("PROFILE_JSON_INVALID: {error}"),
            )
        })?;
        let body = crate::core_ipc::profile_request(
            &value,
            Some(&crate::core_ipc::rule_set_hosts(&self.config.api_base)),
            Some(self.routing.get()),
            &crate::core_ipc::ApplyChoices::new(self.selected_node(), &self.ingress_pins.get()),
        );
        let request = serde_json::from_value(body).map_err(|error| {
            ClientError::failed(
                ErrorCode::ProfileInvalid,
                format!("PROFILE_REQUEST_INVALID: {error}"),
            )
        })?;
        ProfileSession::prepare(request)
            .map(|(session, _)| session)
            .map_err(|error| {
                let code = if error.code == ppvpn_core::codes::PROFILE_EXPIRED {
                    ErrorCode::ProfileExpired
                } else {
                    ErrorCode::ProfileInvalid
                };
                ClientError::failed(code, error.to_string())
            })
    }

    pub(crate) async fn probe_mobile_direct(
        &self,
        method: ProbeMethod,
        node_ids: Vec<String>,
        on_result: &(dyn Fn(ProbeResult) + Send + Sync),
    ) -> Result<(), ClientError> {
        let profile = self
            .current_profile()
            .ok_or(ClientError::StandardNotReady)?;
        let session = self.prepare_mobile_profile(&profile.raw)?;
        self.mobile_control
            .probe_profile(session, method, node_ids, on_result)
            .await
    }
}

impl MobileControl {
    async fn probe_profile(
        &self,
        session: ProfileSession,
        method: ProbeMethod,
        node_ids: Vec<String>,
        on_result: &(dyn Fn(ProbeResult) + Send + Sync),
    ) -> Result<(), ClientError> {
        let ids = if node_ids.is_empty() {
            session.nodes().into_iter().map(|node| node.id).collect()
        } else {
            node_ids
        };
        let mut seen = std::collections::HashSet::new();
        let ids: Vec<String> = ids
            .into_iter()
            .filter(|id| seen.insert(id.clone()))
            .collect();
        let core_method = match method {
            ProbeMethod::Icmp => ppvpn_core::ProbeMethod::Icmp,
            ProbeMethod::Tcp => ppvpn_core::ProbeMethod::Tcp,
            ProbeMethod::Connect => {
                return Err(ClientError::failed(ErrorCode::ProbeFailed, "VPN_REQUIRED"))
            }
        };
        let cancellation = Arc::new(ProbeCancellation::default());
        let generation = {
            let mut probes = self.probes.lock().unwrap();
            probes.retain(|probe| probe.strong_count() > 0);
            probes.push(Arc::downgrade(&cancellation));
            self.generation.load(Ordering::SeqCst)
        };
        let network = *self.network.lock().unwrap();
        let request = ppvpn_core::ProbeEntrancesRequest::new(core_method, 5_000, 4)
            .with_node_ids(ids.clone());
        let results = tokio::time::timeout(
            Duration::from_secs(12),
            session.probe_entrances(request, network, &cancellation),
        )
        .await;
        if generation != self.generation.load(Ordering::SeqCst) {
            return Err(ClientError::Cancelled);
        }
        let values = match results {
            Ok(Ok(results)) => results,
            Ok(Err(error)) => {
                let code = crate::core_ipc::probe_error_code(error.code);
                for id in ids {
                    on_result(crate::core_ipc::failed_probe(
                        &id,
                        method,
                        ClientErrorInfo::new(code, error.code),
                    ));
                }
                return Ok(());
            }
            Err(_) => {
                cancellation.cancel();
                for id in ids {
                    on_result(crate::core_ipc::failed_probe(
                        &id,
                        method,
                        ClientErrorInfo::new(ErrorCode::Timeout, "DIRECT_PROBE_TIMEOUT"),
                    ));
                }
                return Ok(());
            }
        };
        for result in values {
            let value = serde_json::to_value(&result)
                .map_err(|error| ClientError::failed(ErrorCode::ProbeFailed, error.to_string()))?;
            on_result(crate::core_ipc::entrance_to_probe(
                &value,
                method,
                &result.node_id,
            ));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
