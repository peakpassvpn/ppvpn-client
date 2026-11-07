//! iOS public packet transport. Native callbacks own settings and the write
//! barrier; queues are bounded and scoped to one monotonically unique epoch.
use async_trait::async_trait;
use ppvpn_core::{capture::*, capture_host::*, Engine, Platform};
use ppvpn_engine_host::ApiError;
use std::ffi::{c_char, c_void, CString};
use std::path::Path;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
use tokio::sync::{mpsc, watch};

type Settings = extern "C" fn(*mut c_void, u64, *const c_char) -> i32;
struct Channels {
    generation: u64,
    owner: Arc<SettingsCallbacks>,
    input: mpsc::Sender<Vec<u8>>,
    output: std::sync::mpsc::Receiver<Vec<u8>>,
    end: watch::Sender<bool>,
}
static CHANNELS: Mutex<Option<Channels>> = Mutex::new(None);
static GENERATION: AtomicU64 = AtomicU64::new(0);
const LEASE: &str = "ios-public-packet-flow";

pub(crate) fn send(generation: u64, bytes: &[u8]) -> bool {
    let slot = CHANNELS.lock().unwrap_or_else(|p| p.into_inner());
    slot.as_ref()
        .filter(|c| c.generation == generation)
        .is_some_and(|c| c.input.try_send(bytes.to_vec()).is_ok())
}
pub(crate) fn receive(generation: u64, bytes: &mut [u8]) -> usize {
    let slot = CHANNELS.lock().unwrap_or_else(|p| p.into_inner());
    let Some(c) = slot.as_ref().filter(|c| c.generation == generation) else {
        return 0;
    };
    let Ok(packet) = c.output.try_recv() else {
        return 0;
    };
    if packet.len() > bytes.len() {
        return 0;
    }
    bytes[..packet.len()].copy_from_slice(&packet);
    packet.len()
}
#[derive(Clone)]
struct Provider {
    settings: Settings,
    context: usize,
    callbacks: Arc<SettingsCallbacks>,
}
/// A ticket is reserved before spawn_blocking, so stop also drains callbacks
/// still queued in Tokio. This fences the same C context as the exit callback.
#[derive(Default)]
struct SettingsCallbacks {
    state: Mutex<(bool, usize)>,
    done: Condvar,
}
struct SettingsCall(Arc<SettingsCallbacks>);
impl SettingsCallbacks {
    fn enter(self: &Arc<Self>) -> Option<SettingsCall> {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        if state.0 {
            return None;
        }
        state.1 += 1;
        Some(SettingsCall(self.clone()))
    }
    fn close_and_wait(&self) {
        let mut state = self.state.lock().unwrap_or_else(|p| p.into_inner());
        state.0 = true;
        self.done.notify_all();
        while state.1 != 0 {
            state = self.done.wait(state).unwrap_or_else(|p| p.into_inner());
        }
    }
}
impl Drop for SettingsCall {
    fn drop(&mut self) {
        let mut state = self.0.state.lock().unwrap_or_else(|p| p.into_inner());
        state.1 -= 1;
        self.0.done.notify_all();
    }
}

fn clear_callbacks(callbacks: &Arc<SettingsCallbacks>) {
    let mut slot = CHANNELS.lock().unwrap_or_else(|p| p.into_inner());
    if slot
        .as_ref()
        .is_some_and(|c| Arc::ptr_eq(&c.owner, callbacks))
    {
        if let Some(channels) = slot.take() {
            let _ = channels.end.send(true);
        }
    }
}
struct Attachment {
    provider: Provider,
    generation: u64,
    settings: String,
}
fn dirty(generation: u64) -> Vec<CaptureLeftover> {
    vec![CaptureLeftover {
        kind: ResourceKind::Worker,
        resource_id: format!("ios-packet-{generation}"),
    }]
}
impl Attachment {
    async fn configure(&self, attached: bool) -> CaptureHostResult<()> {
        let provider = self.provider.clone();
        let generation = self.generation;
        let settings = attached.then(|| self.settings.clone());
        let call = self
            .provider
            .callbacks
            .enter()
            .ok_or_else(|| dirty(generation))?;
        // Native uses its own serial packet queue. Never block a runtime worker
        // waiting for NetworkExtension's asynchronous settings completion.
        tokio::task::spawn_blocking(move || {
            let _call = call;
            let json = settings.map(|s| CString::new(s).unwrap());
            let result = (provider.settings)(
                provider.context as *mut c_void,
                generation,
                json.as_ref().map_or(std::ptr::null(), |s| s.as_ptr()),
            );
            if result == 1 {
                Ok(())
            } else {
                Err(dirty(generation))
            }
        })
        .await
        .unwrap_or_else(|_| Err(dirty(generation)))
    }
    fn clear(&self) {
        let mut slot = CHANNELS.lock().unwrap_or_else(|p| p.into_inner());
        if slot
            .as_ref()
            .is_some_and(|c| c.generation == self.generation)
        {
            if let Some(channels) = slot.take() {
                let _ = channels.end.send(true);
            }
        }
    }
}
#[async_trait]
impl CaptureHostApi for Provider {
    async fn prepare(
        &self,
        _: &ForwarderIdentity,
        _: u64,
        spec: &CaptureSpec,
        settings: &CaptureNetworkSettings,
    ) -> CaptureHostResult<Arc<dyn CaptureAttachment>> {
        if spec.resource
            != (CaptureResource::HostLease {
                lease_id: LEASE.into(),
            })
            || !spec.ipv6
        {
            return Err(Vec::new());
        }
        let generation = GENERATION.fetch_add(1, Ordering::SeqCst) + 1;
        let json = serde_json::json!({"mtu": settings.mtu,
            "ipv4_address":settings.ipv4_address,"ipv4_routes":["0.0.0.0/0"],
            "ipv6_address":settings.ipv6_address,"ipv6_routes":["::/0"],
            "dns_servers":settings.dns_servers})
        .to_string();
        Ok(Arc::new(Attachment {
            provider: self.clone(),
            generation,
            settings: json,
        }))
    }
}
#[async_trait]
impl CaptureAttachment for Attachment {
    fn transport(&self) -> CaptureTransport {
        CaptureTransport::Packets { capacity: 64 }
    }
    fn interface_name(&self) -> &str {
        ""
    }
    fn publish_packets(&self, mut flow: CapturePacketFlow) -> CaptureHostResult<()> {
        let (input, mut rx) = mpsc::channel(64);
        let (output, down) = std::sync::mpsc::sync_channel(64);
        let (end, mut stopped) = watch::channel(false);
        let mut stopped_down = stopped.clone();
        // Match close -> clear ordering: a timed-out old operation may not
        // publish its transport after its host/context has been released.
        let callbacks = self
            .provider
            .callbacks
            .state
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if callbacks.0 {
            return Err(dirty(self.generation));
        }
        let mut slot = CHANNELS.lock().unwrap_or_else(|p| p.into_inner());
        if slot.is_some() {
            return Err(dirty(self.generation));
        }
        *slot = Some(Channels {
            generation: self.generation,
            owner: self.provider.callbacks.clone(),
            input,
            output: down,
            end,
        });
        drop(slot);
        drop(callbacks);
        tokio::spawn(async move {
            loop {
                tokio::select! { biased;
                    _ = stopped.changed() => return,
                    packet = rx.recv() => {
                        let Some(packet) = packet else { return; };
                        tokio::select! { biased;
                            _ = stopped.changed() => return,
                            result = flow.uplink.send(packet) => if result.is_err() { return; }
                        }
                    }
                }
            }
        });
        tokio::spawn(async move {
            loop {
                tokio::select! { biased;
                    _ = stopped_down.changed() => return,
                    packet = flow.downlink.recv() => match packet {
                        Some(packet) => { let _ = output.try_send(packet); },
                        None => return,
                    }
                }
            }
        });
        Ok(())
    }
    async fn commit_attach(&self) -> CaptureHostResult<()> {
        self.configure(true).await
    }
    async fn rollback_attach(&self) -> CaptureHostResult<()> {
        let result = self.configure(false).await;
        self.clear();
        result
    }
    async fn commit_detach(&self) -> CaptureHostResult<()> {
        let result = self.configure(false).await;
        if result.is_ok() {
            self.clear();
        }
        result
    }
    async fn rollback_detach(&self) -> CaptureHostResult<()> {
        self.configure(true).await
    }
}
pub(crate) fn start(
    socket: &Path,
    state: &Path,
    settings: Settings,
    exit: Option<extern "C" fn(*mut c_void, *const c_char)>,
    context: *mut c_void,
) -> Result<(), ApiError> {
    start_with_callbacks(
        socket,
        state,
        settings,
        exit,
        context,
        Arc::new(SettingsCallbacks::default()),
    )
}
fn start_with_callbacks(
    socket: &Path,
    state: &Path,
    settings: Settings,
    exit: Option<extern "C" fn(*mut c_void, *const c_char)>,
    context: *mut c_void,
    callbacks: Arc<SettingsCallbacks>,
) -> Result<(), ApiError> {
    let provider = Provider {
        settings,
        context: context as usize,
        callbacks,
    };
    let callbacks = provider.callbacks.clone();
    let address = context as usize;
    let callback = crate::host::ExitCallback::from_fn_and_drain(
        move |reason| {
            if let Some(exit) = exit {
                exit(address as *mut c_void, reason.as_ptr());
            }
        },
        move || {
            callbacks.close_and_wait();
            clear_callbacks(&callbacks);
        },
    );
    crate::host::start_with(
        -1,
        socket,
        state,
        Platform::Ios,
        Default::default(),
        Some(callback),
        |mut config| {
            config.tun = None;
            config.with_capture_host(CaptureHost::new(Arc::new(provider)))
        },
    )
}
pub(crate) async fn ensure_capture(engine: &Engine) -> Result<(), ApiError> {
    if !engine.capture_capabilities().supports_live_capture() {
        return Ok(());
    }
    let status = engine.capture_status().map_err(ApiError::from)?;
    if status.phase == CapturePhase::Attached {
        return Ok(());
    }
    engine
        .attach_capture(AttachCapture {
            protocol_version: 1,
            expected_epoch: status.epoch,
            spec: CaptureSpec {
                resource: CaptureResource::HostLease {
                    lease_id: LEASE.into(),
                },
                ipv6: true,
            },
        })
        .await
        .map_err(ApiError::from)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn generation_fences_both_directions_and_admission_is_bounded() {
        let _serial = crate::host::test_lock();
        let (input, mut rx) = mpsc::channel(1);
        let (output, down) = std::sync::mpsc::sync_channel(1);
        let (end, _) = watch::channel(false);
        *CHANNELS.lock().unwrap() = Some(Channels {
            generation: 123,
            owner: Arc::new(SettingsCallbacks::default()),
            input,
            output: down,
            end,
        });
        assert!(!send(122, &[0x45, 1]));
        assert!(send(123, &[0x45, 2]));
        assert!(!send(123, &[0x45, 3]));
        assert_eq!(rx.try_recv().unwrap(), [0x45, 2]);
        output.try_send(vec![0x60, 7]).unwrap();
        let mut buffer = [0; 8];
        assert_eq!(receive(122, &mut buffer), 0);
        assert_eq!(receive(123, &mut buffer), 2);
        assert_eq!(&buffer[..2], &[0x60, 7]);
        CHANNELS.lock().unwrap().take();
        assert!(!send(123, &[0x45]));
        assert_eq!(receive(123, &mut buffer), 0);
    }
    static CURRENT: AtomicU64 = AtomicU64::new(0);
    extern "C" fn accept(_: *mut c_void, generation: u64, settings: *const c_char) -> i32 {
        CURRENT.store(
            if settings.is_null() { 0 } else { generation },
            Ordering::SeqCst,
        );
        1
    }
    fn udp_packet(port: u16) -> Vec<u8> {
        let payload = b"public-packet-bridge";
        let mut packet = vec![0_u8; 28 + payload.len()];
        packet[0] = 0x45;
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        packet[8] = 64;
        packet[9] = 17;
        packet[12..16].copy_from_slice(&[10, 60, 159, 89]);
        packet[16..20].copy_from_slice(&[127, 0, 0, 1]);
        packet[20..22].copy_from_slice(&42000_u16.to_be_bytes());
        packet[22..24].copy_from_slice(&port.to_be_bytes());
        packet[24..26].copy_from_slice(&((payload.len() + 8) as u16).to_be_bytes());
        packet[28..].copy_from_slice(payload);
        let mut sum: u32 = packet[..20]
            .as_chunks::<2>()
            .0
            .iter()
            .map(|p| u32::from(u16::from_be_bytes([p[0], p[1]])))
            .sum();
        while sum > 0xffff {
            sum = (sum & 0xffff) + (sum >> 16);
        }
        packet[10..12].copy_from_slice(&(!(sum as u16)).to_be_bytes());
        packet
    }
    #[test]
    fn native_ios_owner_echoes_public_packets_and_fences_reattachment() {
        // Serialize process-global C ABI state outside the async runtime.
        let _serial = crate::host::test_lock();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .unwrap();
        runtime.block_on(async {
            let dir = std::env::temp_dir().join(format!("ppvpn-packet-ffi-{}", std::process::id()));
            std::fs::create_dir_all(&dir).unwrap();
            let engine = Engine::new(
                ppvpn_core::EngineConfig::new(ppvpn_core::Role::Tun, Platform::Ios, &dir)
                    .with_capture_host(CaptureHost::new(Arc::new(Provider {
                        settings: accept,
                        context: 0,
                        callbacks: Arc::new(SettingsCallbacks::default()),
                    }))),
            )
            .await
            .unwrap();
            let mut profile: serde_json::Value = serde_json::from_slice(include_bytes!(
                "../../../testdata/golden/contract/profiles/base.json"
            ))
            .unwrap();
            profile["routing"]["final"] = serde_json::json!({"type":"direct"});
            engine
                .apply(ppvpn_core::ApplyRequest::new(
                    serde_json::to_vec(&profile).unwrap(),
                ))
                .await
                .unwrap();
            let mut network = ppvpn_core::NetworkState::default();
            network.interface = Some("lo0".into());
            network.addresses = vec!["127.0.0.1/8".into()];
            engine.set_network_state(network).unwrap();
            engine.start().await.unwrap();
            let owner = engine.runtime_observation().owner;
            let echo = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
            let port = echo.local_addr().unwrap().port();
            let task = tokio::spawn(async move {
                let mut buffer = [0; 256];
                while let Ok((n, peer)) = echo.recv_from(&mut buffer).await {
                    echo.send_to(&buffer[..n], peer).await.unwrap();
                }
            });
            let packet = udp_packet(port);
            let mut previous = 0;
            for _ in 0..2 {
                ensure_capture(&engine).await.unwrap();
                let generation = CURRENT.load(Ordering::SeqCst);
                assert_ne!(generation, previous);
                if previous != 0 {
                    assert!(!send(previous, &packet));
                }
                assert_eq!(
                    unsafe {
                        crate::ppvpn_tunnel_packet_send(generation, packet.as_ptr(), packet.len())
                    },
                    1
                );
                let mut buffer = vec![0; 65535];
                let length = tokio::time::timeout(std::time::Duration::from_secs(5), async {
                    loop {
                        let n = unsafe {
                            crate::ppvpn_tunnel_packet_receive(
                                generation,
                                buffer.as_mut_ptr(),
                                buffer.len(),
                            )
                        };
                        if n > 0 {
                            break n;
                        }
                        tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    }
                })
                .await
                .unwrap();
                assert!(buffer[..length]
                    .windows(b"public-packet-bridge".len())
                    .any(|p| p == b"public-packet-bridge"));
                let status = engine.capture_status().unwrap();
                engine
                    .detach_capture(DetachCapture {
                        protocol_version: 1,
                        expected_epoch: status.epoch,
                        lease: status.lease.unwrap(),
                    })
                    .await
                    .unwrap();
                assert_eq!(CURRENT.load(Ordering::SeqCst), 0);
                assert!(!send(generation, &packet));
                assert_eq!(receive(generation, &mut buffer), 0);
                assert_eq!(engine.runtime_observation().owner, owner);
                previous = generation;
            }
            engine.shutdown().await.unwrap();
            task.abort();
            std::fs::remove_dir_all(&dir).unwrap();
        });
    }
    #[test]
    fn host_stop_drains_queued_packet_callbacks_even_without_an_exit_notification() {
        let _serial = crate::host::test_lock();
        let callbacks = Arc::new(SettingsCallbacks::default());
        let queued = callbacks.enter().unwrap();
        let dir =
            std::env::temp_dir().join(format!("ppvpn-packet-callback-stop-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        start_with_callbacks(
            &dir.join("tunnel.sock"),
            &dir.join("state"),
            refuse,
            None,
            std::ptr::null_mut(),
            callbacks.clone(),
        )
        .unwrap();
        let (input, _) = mpsc::channel(1);
        let (_, output) = std::sync::mpsc::sync_channel(1);
        let (end, _) = watch::channel(false);
        *CHANNELS.lock().unwrap() = Some(Channels {
            generation: 987,
            owner: callbacks.clone(),
            input,
            output,
            end,
        });
        let (done, finished) = std::sync::mpsc::channel();
        let stop = std::thread::spawn(move || {
            crate::host::stop();
            done.send(()).unwrap();
        });
        {
            let mut state = callbacks.state.lock().unwrap();
            while !state.0 {
                let (next, timeout) = callbacks
                    .done
                    .wait_timeout(state, std::time::Duration::from_secs(5))
                    .unwrap();
                assert!(!timeout.timed_out());
                state = next;
            }
        }
        assert!(
            callbacks.enter().is_none(),
            "stop must close callback admission"
        );
        assert!(
            finished.try_recv().is_err(),
            "queued work still owns the native context"
        );
        let refused = start(
            &dir.join("new.sock"),
            &dir.join("new-state"),
            refuse,
            None,
            std::ptr::null_mut(),
        )
        .unwrap_err();
        assert_eq!(refused.code, "TUNNEL_ALREADY_RUNNING");
        drop(queued);
        finished
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        stop.join().unwrap();
        assert!(
            CHANNELS.lock().unwrap().is_none(),
            "old packet queues must be discarded"
        );
        assert!(
            callbacks.enter().is_none(),
            "no callback may enter after stop"
        );
        std::fs::remove_dir_all(dir).unwrap();
    }
    extern "C" fn refuse(_: *mut c_void, _: u64, _: *const c_char) -> i32 {
        0
    }
    #[tokio::test]
    async fn uncertain_native_settlement_is_never_a_clean_rollback() {
        let a = Attachment {
            provider: Provider {
                settings: refuse,
                context: 0,
                callbacks: Arc::new(SettingsCallbacks::default()),
            },
            generation: 234,
            settings: "{}".into(),
        };
        assert_eq!(a.configure(true).await.unwrap_err(), dirty(234));
        assert_eq!(a.configure(false).await.unwrap_err(), dirty(234));
    }
}
