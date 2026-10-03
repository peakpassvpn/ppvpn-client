//! The shared local proxy and the system proxy listener
//! (docs/host-integration.md, 4.6): the device state read at `new`, what
//! the translation gets of it, the credentials and the listener toggle.
//!
//! Lock order: `live` before `proxies`; `live` is never taken while
//! `proxies` is held.

use std::collections::BTreeMap;
use std::sync::{Mutex, MutexGuard};

use super::{now, Error, Inner};
use crate::config::{EngineConfig, Role};
use crate::error::codes;
use crate::event::Event;
use crate::localproxy::{LocalProxyState, PROTOCOLS};
use crate::profile::Profile;
use crate::request::RoutingMode;
use crate::status::{LocalProxyStatus, SystemProxyStatus};
use crate::translate;
use crate::types::{LocalProxyCredential, LocalProxyMetadata};

/// Where the system proxy listener listens (the translation's inbound).
const SYSTEM_PROXY_LISTEN: &str = "127.0.0.1";

/// A Standard instance's listeners: the shared local proxy's state and the
/// system proxy listener's toggle.
pub(super) struct Proxies {
    state: LocalProxyState,
    /// The host turned the system proxy listener on. Not persisted: off at
    /// `new`, as Go.
    system_proxy: bool,
    /// Its port, chosen when it was turned on and again at each start.
    system_port: u16,
}

impl Proxies {
    /// Reads or creates the state in `state_dir`: on a Standard instance
    /// with a local proxy, or one that may host the system proxy listener
    /// (its port is kept there too).
    pub(super) fn open(config: &EngineConfig) -> Result<Option<Mutex<Proxies>>, Error> {
        if config.role != Role::Standard || (config.local_proxy.is_none() && !config.system_proxy) {
            return Ok(None);
        }
        let local = config.local_proxy.clone().unwrap_or_default();
        let state = LocalProxyState::open(&config.state_dir, &local)?;
        Ok(Some(Mutex::new(Proxies {
            state,
            system_proxy: false,
            system_port: 0,
        })))
    }
}

impl Inner {
    fn proxies(&self) -> Option<MutexGuard<'_, Proxies>> {
        self.proxies
            .as_ref()
            .map(|proxies| proxies.lock().expect("proxies"))
    }

    /// The local proxy's state, on an instance that has one.
    fn local_proxy(&self) -> Result<MutexGuard<'_, Proxies>, Error> {
        self.config
            .local_proxy
            .as_ref()
            .and_then(|_| self.proxies())
            .ok_or_else(local_proxy_disabled)
    }

    /// `Options.local_proxy`: the shared inbound and its users.
    pub(super) fn local_proxy_options(&self) -> Option<translate::LocalProxy> {
        self.local_proxy().ok().map(|p| p.state.translate_options())
    }

    /// `Options.system_proxy_port`: the listener while turned on.
    pub(super) fn system_proxy_options(&self) -> Option<u16> {
        let proxies = self.proxies()?;
        (proxies.system_proxy && proxies.system_port != 0).then_some(proxies.system_port)
    }

    /// Before `start` opens the listeners (never while they are open): a
    /// shared port taken meanwhile moves (`LocalProxyEndpointChanged`), and
    /// the system proxy listener's port is checked again, as Go's start.
    pub(super) fn prepare_listeners(&self) -> Result<(), Error> {
        let moved = {
            let Some(mut proxies) = self.proxies() else {
                return Ok(());
            };
            let moved = if self.config.local_proxy.is_some() {
                proxies.state.reconcile_port()?
            } else {
                None
            };
            if proxies.system_proxy {
                proxies.system_port = proxies
                    .state
                    .system_proxy_port(true)
                    .map_err(start_failed)?;
            }
            moved
        };
        if let Some(event) = moved {
            self.publish(event);
        }
        Ok(())
    }

    /// `status.local_proxy`: on an instance with a local proxy.
    pub(super) fn local_proxy_status(&self, running: bool) -> Option<LocalProxyStatus> {
        self.local_proxy().ok().map(|p| p.state.status(running))
    }

    /// `status.system_proxy`: listening while running and turned on.
    pub(super) fn system_proxy_status(&self, running: bool) -> SystemProxyStatus {
        let mut status = SystemProxyStatus {
            available: self.config.role == Role::Standard && self.config.system_proxy,
            ..SystemProxyStatus::default()
        };
        if let Some(proxies) = self.proxies() {
            status.enabled = proxies.system_proxy;
            if proxies.system_proxy && proxies.system_port != 0 {
                status.listen = SYSTEM_PROXY_LISTEN.into();
                status.port = proxies.system_port;
                status.protocols = PROTOCOLS.iter().map(|p| (*p).to_owned()).collect();
                status.listening = running;
            }
        }
        status
    }

    pub(super) fn local_proxy_metadata(&self) -> Result<Vec<LocalProxyMetadata>, Error> {
        let live = self.live();
        let profile = live.applied.as_ref().map(|a| &a.profile);
        Ok(self.local_proxy()?.state.metadata(profile))
    }

    pub(super) fn local_proxy_credential(
        &self,
        node_id: &str,
    ) -> Result<LocalProxyCredential, Error> {
        let live = self.live();
        let profile = live.applied.as_ref().map(|a| &a.profile);
        self.local_proxy()?.state.credential(profile, node_id)
    }

    pub(super) fn local_proxy_routed_credential(&self) -> Result<LocalProxyCredential, Error> {
        Ok(self.local_proxy()?.state.routed_credential())
    }

    /// Turns the system proxy listener on or off (idempotent). Before a
    /// start it only takes effect at the start; while running the listener
    /// alone is added or removed (a reload neither opens nor closes one),
    /// and nothing else is touched.
    pub(super) async fn set_system_proxy_listener(
        &self,
        enabled: bool,
    ) -> Result<SystemProxyStatus, Error> {
        let _op = self.op.lock().await;
        self.admit()?;
        let (running, applied) = {
            let live = self.live();
            let applied = live.applied.as_ref().map(|a| {
                (
                    a.profile.clone(),
                    a.mode,
                    a.selected.clone(),
                    a.pins.clone(),
                )
            });
            (live.running, applied)
        };
        {
            let Some(mut proxies) = self.proxies() else {
                return Err(system_proxy_unavailable());
            };
            if proxies.system_proxy == enabled {
                drop(proxies);
                return Ok(self.system_proxy_status(running));
            }
            if enabled {
                // The listener is not open: its persisted port is checked.
                proxies.system_port = proxies
                    .state
                    .system_proxy_port(true)
                    .map_err(start_failed)?;
            }
            proxies.system_proxy = enabled;
        }
        if let (true, Some((profile, mode, selected, pins))) = (running, &applied) {
            if let Err(error) = self
                .toggle_listener(enabled, profile, *mode, selected, pins)
                .await
            {
                if let Some(mut proxies) = self.proxies() {
                    proxies.system_proxy = !enabled;
                }
                return Err(if enabled && error.code != codes::CORE_PANICKED {
                    start_failed(error)
                } else {
                    error
                });
            }
        }
        let revision = applied
            .as_ref()
            .map(|(profile, ..)| profile.revision.clone())
            .unwrap_or_default();
        self.publish(Event::SystemProxyChanged {
            at: now(),
            revision,
            message: if enabled { "enabled" } else { "disabled" }.into(),
        });
        Ok(self.system_proxy_status(running))
    }

    /// While running: the system proxy listener alone is added or removed,
    /// and the translation the next reload or start uses follows.
    async fn toggle_listener(
        &self,
        enabled: bool,
        profile: &Profile,
        mode: RoutingMode,
        selected: &str,
        pins: &BTreeMap<String, String>,
    ) -> Result<(), Error> {
        let translation = translate::translate(profile, &self.options(mode, selected, pins))?;
        let result = if enabled {
            let inbound = system_proxy_inbound(&translation.json)?;
            self.runtime.add_inbound(&inbound).await
        } else {
            self.runtime
                .remove_inbound(translate::SYSTEM_PROXY_INBOUND_TAG)
                .await
        };
        if let Err(e) = result {
            return Err(self.runtime_error(&e));
        }
        let mut live = self.live();
        if let Some(applied) = live.applied.as_mut() {
            applied.translation = translation;
        }
        Ok(())
    }
}

pub(super) fn local_proxy_disabled() -> Error {
    Error::new(
        codes::LOCAL_PROXY_DISABLED,
        false,
        "this instance has no local proxy",
    )
}

pub(super) fn system_proxy_unavailable() -> Error {
    Error::new(
        codes::SYSTEM_PROXY_UNAVAILABLE,
        false,
        "this instance cannot host the system proxy listener",
    )
}

/// The listener cannot be opened: no port, or the runtime refused it.
fn start_failed(error: Error) -> Error {
    Error::new(
        codes::SYSTEM_PROXY_START_FAILED,
        true,
        format!("system proxy listener: {}", error.message),
    )
}

/// The system proxy inbound of a translation, as JSON.
fn system_proxy_inbound(config: &str) -> Result<String, Error> {
    let config: serde_json::Value = serde_json::from_str(config).map_err(|e| {
        Error::new(
            codes::CORE_OPERATION_FAILED,
            false,
            format!("translation: {e}"),
        )
    })?;
    config["inbounds"]
        .as_array()
        .into_iter()
        .flatten()
        .find(|i| i["tag"] == translate::SYSTEM_PROXY_INBOUND_TAG)
        .map(|i| i.to_string())
        .ok_or_else(|| {
            Error::new(
                codes::CORE_OPERATION_FAILED,
                false,
                "translation: no system proxy inbound",
            )
        })
}
