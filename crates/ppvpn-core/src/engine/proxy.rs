//! The shared local proxy and the system proxy listener
//! (docs/host-integration.md, 4.6): the device state read at `new`, what
//! the translation gets of it, the credentials and the listener toggle.
//!
//! Lock order: `live` before `proxies`; `live` is never taken while
//! `proxies` is held.

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use tokio::task::JoinHandle;

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

/// The first retry of a local proxy listener that could not be opened,
/// doubling up to [`RETRY_MAX`].
const RETRY_FIRST: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(30);

/// A Standard instance's listeners: the shared local proxy's state and the
/// system proxy listener's toggle.
pub(super) struct Proxies {
    state: LocalProxyState,
    /// The host turned the system proxy listener on. Not persisted: off at
    /// `new`, as Go.
    system_proxy: bool,
    /// Its port, chosen when it was turned on and again at each start.
    system_port: u16,
    /// The shared listener could not be opened: this run goes without it
    /// (`Degraded{LocalProxyUnavailable}`) and `retry` tries again.
    unavailable: bool,
    retry: Option<JoinHandle<()>>,
    /// Tests: how many more times opening the shared listener fails.
    #[cfg(test)]
    refuse: u32,
}

impl Proxies {
    /// Before the shared listener opens: its port, moved when taken.
    fn reconcile(&mut self) -> Result<Option<Event>, Error> {
        #[cfg(test)]
        if self.refuse > 0 {
            self.refuse -= 1;
            return Err(Error::new(
                codes::CORE_OPERATION_FAILED,
                true,
                "refused by the test",
            ));
        }
        self.state.reconcile_port()
    }
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
            unavailable: false,
            retry: None,
            #[cfg(test)]
            refuse: 0,
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

    /// `Options.local_proxy`: the shared inbound and its users, unless
    /// this run goes without it.
    pub(super) fn local_proxy_options(&self) -> Option<translate::LocalProxy> {
        self.local_proxy()
            .ok()
            .filter(|p| !p.unavailable)
            .map(|p| p.state.translate_options())
    }

    /// `Options.system_proxy_port`: the listener while turned on.
    pub(super) fn system_proxy_options(&self) -> Option<u16> {
        let proxies = self.proxies()?;
        (proxies.system_proxy && proxies.system_port != 0).then_some(proxies.system_port)
    }

    /// Before `start` opens the listeners (never while they are open): a
    /// shared port taken meanwhile moves (`LocalProxyEndpointChanged`), and
    /// the system proxy listener's port is checked again, as Go's start.
    /// A shared listener that cannot be opened is left out of the run
    /// (section 4.6): the start goes on without it.
    pub(super) fn prepare_listeners(&self) -> Result<(), Error> {
        let moved = {
            let Some(mut proxies) = self.proxies() else {
                return Ok(());
            };
            proxies.unavailable = false;
            let moved = if self.config.local_proxy.is_some() {
                proxies.reconcile().unwrap_or_else(|error| {
                    tracing::warn!(error = %error.message, "local proxy listener unavailable");
                    proxies.unavailable = true;
                    None
                })
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

    /// After a start that failed with the shared listener in it: whether
    /// the start is worth trying again without it (it was in it).
    pub(super) fn leave_out_local_proxy(&self) -> bool {
        match self.local_proxy() {
            Ok(mut proxies) if !proxies.unavailable => {
                proxies.unavailable = true;
                true
            }
            _ => false,
        }
    }

    /// Whether this run goes without the shared listener.
    pub(super) fn local_proxy_left_out(&self) -> bool {
        self.local_proxy().is_ok_and(|p| p.unavailable)
    }

    /// After a start without the shared listener: tries to open it with
    /// backoff ([`RETRY_FIRST`] doubling to [`RETRY_MAX`]) while run `run`
    /// lasts. The stop or shutdown cancels it.
    pub(super) fn retry_local_proxy(self: &Arc<Self>, run: u64) {
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            return;
        };
        let weak = Arc::downgrade(self);
        let task = handle.spawn(async move {
            let mut delay = RETRY_FIRST;
            loop {
                tokio::time::sleep(delay).await;
                delay = (delay * 2).min(RETRY_MAX);
                let Some(inner) = weak.upgrade() else { return };
                if inner.open_local_proxy(run).await {
                    return;
                }
            }
        });
        if let Some(mut proxies) = self.proxies() {
            if let Some(previous) = proxies.retry.replace(task) {
                previous.abort();
            }
        }
    }

    /// One retry: true when there is nothing more to retry (opened, or the
    /// run is over).
    async fn open_local_proxy(&self, run: u64) -> bool {
        let _op = self.op.lock().await;
        let (profile, mode, selected, pins) = {
            let live = self.live();
            let Some(a) = live
                .applied
                .as_ref()
                .filter(|_| live.running && !live.shut_down && live.run == run)
            else {
                return true;
            };
            (
                a.profile.clone(),
                a.mode,
                a.selected.clone(),
                a.pins.clone(),
            )
        };
        let moved = {
            let Some(mut proxies) = self.proxies() else {
                return true;
            };
            match proxies.reconcile() {
                Ok(moved) => {
                    proxies.unavailable = false;
                    moved
                }
                Err(error) => {
                    tracing::debug!(error = %error.message, "local proxy listener still unavailable");
                    return false;
                }
            }
        };
        if let Some(event) = moved {
            self.publish(event);
        }
        let opened = match translate::translate(&profile, &self.options(mode, &selected, &pins)) {
            Ok(translation) => {
                match inbound_of(&translation.json, translate::LOCAL_PROXY_INBOUND_TAG) {
                    Ok(inbound) => match self.runtime.add_inbound(&inbound).await {
                        Ok(()) => Ok(translation),
                        Err(e) => Err(self.runtime_error(&e)),
                    },
                    Err(e) => Err(e),
                }
            }
            Err(e) => Err(e),
        };
        let translation = match opened {
            Ok(translation) => translation,
            Err(error) => {
                if let Some(mut proxies) = self.proxies() {
                    proxies.unavailable = true;
                }
                tracing::warn!(error = %error.message, "local proxy listener still unavailable");
                return false;
            }
        };
        if let Some(mut proxies) = self.proxies() {
            proxies.retry = None;
        }
        let mut live = self.live();
        live.local_proxy_unavailable = false;
        if let Some(applied) = live.applied.as_mut() {
            applied.translation = translation;
        }
        self.settle(&mut live);
        drop(live);
        tracing::info!("local proxy listener open");
        true
    }

    /// At stop or shutdown: a pending retry goes with the run.
    pub(super) fn local_proxy_stopped(&self) {
        if let Some(mut proxies) = self.proxies() {
            if let Some(retry) = proxies.retry.take() {
                retry.abort();
            }
            proxies.unavailable = false;
        }
    }

    /// Tests: opening the shared listener fails `times` more times.
    #[cfg(test)]
    pub(super) fn refuse_local_proxy(&self, times: u32) {
        if let Some(mut proxies) = self.proxies() {
            proxies.refuse = times;
        }
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
            let inbound = inbound_of(&translation.json, translate::SYSTEM_PROXY_INBOUND_TAG)?;
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

/// The inbound tagged `tag` of a translation, as JSON.
fn inbound_of(config: &str, tag: &str) -> Result<String, Error> {
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
        .find(|i| i["tag"] == tag)
        .map(|i| i.to_string())
        .ok_or_else(|| {
            Error::new(
                codes::CORE_OPERATION_FAILED,
                false,
                format!("translation: no {tag} inbound"),
            )
        })
}
