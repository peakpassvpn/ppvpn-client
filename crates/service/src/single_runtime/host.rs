//! User-process host of root-owned capture resources. Only opaque grants cross
//! framed IPC; SCM_RIGHTS carries one independently validated utun duplicate.
use super::{
    broker::{self, Grant},
    fdpass, mac_peer, native,
};
use crate::protocol::{self, Command, HandshakeResponse, Request, Response};
use anyhow::{anyhow, bail, Result};
use async_trait::async_trait;
use ppvpn_core::{
    capture::{
        CaptureLease, CaptureLeftover, CaptureResource, CaptureSpec, ForwarderIdentity,
        ResourceKind,
    },
    capture_host::*,
};
use serde_json::json;
use std::{
    os::fd::OwnedFd,
    os::unix::net::UnixStream,
    path::Path,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Weak,
    },
    time::Duration,
};
#[derive(Clone)]
struct Client;
impl Client {
    fn call(
        &self,
        path: &str,
        grant: &Grant,
        descriptor: bool,
    ) -> Result<(Option<OwnedFd>, Option<String>)> {
        self.request(path, grant, descriptor, &mut false)
    }
    fn request(
        &self,
        path: &str,
        grant: &Grant,
        descriptor: bool,
        attempted: &mut bool,
    ) -> Result<(Option<OwnedFd>, Option<String>)> {
        let mut stream = UnixStream::connect(broker::SOCKET)?;
        stream.set_read_timeout(Some(Duration::from_millis(2400)))?;
        stream.set_write_timeout(Some(Duration::from_millis(600)))?;
        mac_peer::verify_exact(
            &stream,
            0,
            crate::mac_identity::SERVICE_IDENTIFIER,
            Path::new(broker::IMAGE),
        )?;
        let hello = Request {
            id: uuid::Uuid::new_v4().to_string(),
            timestamp: protocol::now_epoch(),
            command: Command::Handshake,
            payload: json!({"client_nonce":uuid::Uuid::new_v4().simple().to_string(),"build_id":protocol::SERVICE_BUILD_ID,"protocol_version":1}),
            auth_token: None,
            signature: String::new(),
        };
        broker::write_frame(&mut stream, &hello)?;
        let response: Response = broker::read_frame(&mut stream)?;
        if response.id != hello.id || !response.success {
            bail!("CAPTURE_HANDSHAKE_REJECTED");
        }
        if response
            .data
            .as_ref()
            .and_then(|d| d.get("build_id"))
            .and_then(|v| v.as_str())
            != Some(protocol::SERVICE_BUILD_ID)
            || response
                .data
                .as_ref()
                .and_then(|d| d.get("protocol_version"))
                .and_then(|v| v.as_u64())
                != Some(1)
        {
            bail!("CAPTURE_BROKER_VERSION_MISMATCH");
        }
        let hello: HandshakeResponse = serde_json::from_value(
            response
                .data
                .ok_or_else(|| anyhow!("CAPTURE_HANDSHAKE_INVALID"))?,
        )?;
        let key = hex::decode(hello.auth_key)?;
        if key.len() != 32 || hello.expires_at <= protocol::now_epoch() {
            bail!("CAPTURE_HANDSHAKE_INVALID");
        }
        let request = Request::signed(
            uuid::Uuid::new_v4().to_string(),
            Command::CoreApi,
            json!({"path":path,"body":grant}),
            Some(hello.auth_token),
            &key,
        )?;
        *attempted = true;
        broker::write_frame(&mut stream, &request)?;
        let response: Response = broker::read_frame(&mut stream)?;
        if response.id != request.id || !response.verify_with(&key)? {
            bail!("CAPTURE_REPLY_INVALID");
        }
        if !response.success {
            if path == "prepare"
                && response
                    .data
                    .as_ref()
                    .and_then(|d| d.get("clean"))
                    .and_then(|v| v.as_bool())
                    == Some(true)
            {
                *attempted = false;
            }
            bail!("CAPTURE_BROKER_REJECTED");
        }
        if response
            .data
            .as_ref()
            .and_then(|d| d.get("root_runtime_ids"))
            .and_then(|v| v.as_array())
            .is_none_or(|ids| !ids.is_empty())
        {
            bail!("CAPTURE_BROKER_ROLE_VIOLATION");
        }
        if !descriptor {
            return Ok((None, None));
        }
        let name = response
            .data
            .and_then(|d| {
                d.get("interface")
                    .and_then(|v| v.as_str())
                    .map(str::to_owned)
            })
            .ok_or_else(|| anyhow!("CAPTURE_DEVICE_NAME_MISSING"))?;
        let fd = match fdpass::receive(&mut stream) {
            Ok(fd) => fd,
            Err(e) if e.requires_process_restart() => std::process::abort(),
            Err(_) => bail!("CAPTURE_DESCRIPTOR_INVALID"),
        };
        if native::interface(&fd)? != name {
            bail!("CAPTURE_DEVICE_NAME_MISMATCH");
        }
        Ok((Some(fd), Some(name)))
    }
}
#[derive(Default)]
pub struct Host {
    active: parking_lot::Mutex<Vec<Weak<Attachment>>>,
}
impl Host {
    pub fn lease_lost(&self) -> bool {
        let mut active = self.active.lock();
        active.retain(|w| w.strong_count() > 0);
        active
            .iter()
            .filter_map(Weak::upgrade)
            .any(|a| a.failed.load(Ordering::SeqCst))
    }
}
struct Attachment {
    grant: Grant,
    fd: OwnedFd,
    name: String,
    failed: Arc<AtomicBool>,
    stop: Arc<AtomicBool>,
}
fn leftovers(grant: &Grant) -> Vec<CaptureLeftover> {
    vec![CaptureLeftover {
        kind: ResourceKind::Device,
        resource_id: grant.token.clone(),
    }]
}
#[async_trait]
impl CaptureHostApi for Host {
    async fn prepare(
        &self,
        owner: &ForwarderIdentity,
        epoch: u64,
        spec: &CaptureSpec,
        settings: &CaptureNetworkSettings,
    ) -> CaptureHostResult<Arc<dyn CaptureAttachment>> {
        let CaptureResource::HostLease { lease_id } = &spec.resource else {
            return Err(vec![]);
        };
        let grant = Grant {
            token: lease_id.clone(),
            lease: CaptureLease {
                owner: owner.clone(),
                attachment_epoch: epoch,
            },
            settings: Some(settings.into()),
        };
        if grant.validate().is_err() {
            return Err(vec![]);
        }
        let work = grant.clone();
        let result = tokio::task::spawn_blocking(move || {
            let mut attempted = false;
            let result = (|| {
                let (fd, name) = Client.request("prepare", &work, true, &mut attempted)?;
                Ok::<_, anyhow::Error>((
                    fd.ok_or_else(|| anyhow!("CAPTURE_DEVICE_MISSING"))?,
                    name.ok_or_else(|| anyhow!("CAPTURE_NAME_MISSING"))?,
                ))
            })();
            (result, attempted)
        })
        .await;
        let (fd, name) = match result {
            Ok((Ok(device), _)) => device,
            Ok((Err(_), false)) => return Err(vec![]),
            _ => return Err(leftovers(&grant)),
        };
        let failed = Arc::new(AtomicBool::new(false));
        let stop = Arc::new(AtomicBool::new(false));
        let attachment = Arc::new(Attachment {
            grant: grant.clone(),
            fd,
            name,
            failed: failed.clone(),
            stop: stop.clone(),
        });
        let weak = Arc::downgrade(&attachment);
        self.active.lock().push(weak.clone());
        // Renewal belongs to the forwarding agent, never the UI. A root lease
        // failure wakes agent reconciliation, detaching just native capture.
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(5)).await;
                if stop.load(Ordering::SeqCst) || weak.strong_count() == 0 {
                    break;
                }
                let work = grant.clone();
                if !matches!(
                    tokio::task::spawn_blocking(move || Client.call("renew", &work, false)).await,
                    Ok(Ok(_))
                ) {
                    failed.store(true, Ordering::SeqCst);
                    break;
                }
            }
        });
        Ok(attachment)
    }
}
impl Attachment {
    async fn operation(&self, path: &'static str) -> CaptureHostResult<()> {
        let grant = self.grant.clone();
        match tokio::task::spawn_blocking(move || Client.call(path, &grant, false)).await {
            Ok(Ok(_)) => Ok(()),
            _ => {
                self.failed.store(true, Ordering::SeqCst);
                Err(leftovers(&self.grant))
            }
        }
    }
}
#[async_trait]
impl CaptureAttachment for Attachment {
    fn transport(&self) -> CaptureTransport {
        CaptureTransport::Device
    }
    fn interface_name(&self) -> &str {
        &self.name
    }
    fn duplicate_device(&self) -> std::io::Result<OwnedFd> {
        self.fd.try_clone()
    }
    async fn commit_attach(&self) -> CaptureHostResult<()> {
        self.operation("attach").await
    }
    async fn rollback_attach(&self) -> CaptureHostResult<()> {
        self.stop.store(true, Ordering::SeqCst);
        self.operation("release").await
    }
    async fn commit_detach(&self) -> CaptureHostResult<()> {
        self.operation("detach").await
    }
    async fn rollback_detach(&self) -> CaptureHostResult<()> {
        self.operation("restore").await
    }
}
impl Drop for Attachment {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
        // Native detach drops its final attachment before completing the core
        // transaction. Settle release here so a following attach cannot race an
        // old disarmed root grant. Merely queueing a thread left that slot busy.
        // This is bounded control-plane IPC; a Tokio worker is replaced while
        // waiting, rather than blocking the forwarding executor's worker pool.
        let release = || {
            let _ = Client.call("release", &self.grant, false);
        };
        if tokio::runtime::Handle::try_current()
            .is_ok_and(|h| h.runtime_flavor() == tokio::runtime::RuntimeFlavor::MultiThread)
        {
            tokio::task::block_in_place(release);
        } else {
            release();
        }
        // If the root channel fails, its independent expiry and persisted
        // ledger remain authoritative; no successor can silently steal it.
    }
}
