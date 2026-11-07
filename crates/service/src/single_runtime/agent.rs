//! Explicit per-user agent mode. Packaging has not enabled a LaunchAgent yet.
//! No root broker or UI-embedded engine is started from this entry point.
use super::{
    endpoint::Endpoint,
    gateway::{EngineForwarder, Gateway},
    mac_peer, server,
};
use anyhow::{anyhow, Result};
use std::{path::Path, sync::Arc};
const INSTALLED_SERVICE:&str="/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle/Contents/MacOS/ppvpn-service";
const INSTALLED_UI: &str = "/Applications/PPVPN.app/Contents/MacOS/PPVPN";
pub fn validate_installation() -> Result<u32> {
    let uid = unsafe { libc::geteuid() };
    if uid == 0 || uid != unsafe { libc::getuid() } {
        return Err(anyhow!("USER_RUNTIME_ROOT_FORBIDDEN"));
    }
    // Executed image and installed identity are fixed policy; no request or
    // environment override can grant an arbitrary helper/application identity.
    let image = Path::new(INSTALLED_SERVICE);
    mac_peer::secure_installed_image(image)?;
    if std::env::current_exe()?.canonicalize()? != image {
        return Err(anyhow!("USER_RUNTIME_INSTALL_IMAGE_MISMATCH"));
    }
    let requirement = crate::mac_identity::requirement(
        crate::mac_identity::SERVICE_IDENTIFIER,
        crate::mac_identity::team_id(),
    )?;
    crate::mac_identity::check_file(image, &requirement)?;
    Ok(uid)
}
pub fn run(dir: &Path, log_dir: &Path) -> Result<()> {
    let uid = validate_installation()?;
    super::endpoint::private_directory(log_dir, uid)?;
    let endpoint = Endpoint::bind(dir, uid)?;
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ppvpn-user-core")
        .build()?;
    runtime.block_on(async {
        let physical = super::native::physical().unwrap_or_else(|_|ppvpn_core::NetworkState::offline());
        let capture_host = Arc::new(super::host::Host::default());
        let config = ppvpn_core::EngineConfig::new(
            ppvpn_core::Role::Shared,
            ppvpn_core::Platform::Macos,
            dir.join("state"),
        )
        .with_capture_host(ppvpn_core::capture_host::CaptureHost::new(capture_host.clone()))
        .with_local_proxy(ppvpn_core::LocalProxyConfig::default())
        .with_system_proxy(true)
        .with_log(ppvpn_core::LogConfig::new(ppvpn_core::LogLevel::Info, ppvpn_core::LogSink::Channel));
        let backend = EngineForwarder::new(config).await?;
        backend.network(physical.clone())?;
        let monitor_backend = backend.clone();
        let monitor = tokio::spawn(async move {
            let mut previous = physical;
            loop {
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                let current = tokio::task::spawn_blocking(super::native::physical).await
                    .ok().and_then(Result::ok).unwrap_or_else(ppvpn_core::NetworkState::offline);
                if current != previous && monitor_backend.network(current.clone()).is_ok() { previous = current; }
                if (capture_host.lease_lost() || monitor_backend.capture_expired())
                    && capture_host.reconciliation_ready().await {
                    let _ = monitor_backend.reconcile_lost_capture().await;
                }
            }
        });
        let gateway = Gateway::new(backend.clone(), uid)?;
        let writer = Arc::new(parking_lot::Mutex::new(tracing_appender::rolling::RollingFileAppender::builder()
            .rotation(tracing_appender::rolling::Rotation::DAILY).filename_prefix("ppvpn-core")
            .filename_suffix("log").max_log_files(7).build(log_dir)?));
        let mut lines = backend.logs();
        let (log_stop, mut log_stopped) = tokio::sync::oneshot::channel::<()>();
        let logging = tokio::spawn(async move {
            loop {
                let line = tokio::select! { _=&mut log_stopped=>break, line=lines.recv()=>line };
                let Some(mut line) = line else { break };
                if !line.ends_with('\n') { line.push('\n'); }
                let writer = writer.clone();
                let _ = tokio::task::spawn_blocking(move || { use std::io::Write; writer.lock().write_all(line.as_bytes()) }).await;
            }
        });
        let verify: server::VerifyPeer = Arc::new(move |stream| {
            mac_peer::verify(
                stream,
                uid,
                crate::mac_identity::CLIENT_IDENTIFIER,
                Path::new(INSTALLED_UI),
            )
        });
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())?;
        let stop = async {
            tokio::select! { _=tokio::signal::ctrl_c()=>(),_=terminate.recv()=>(),_=backend.until_fatal()=>() }
        };
        let served = server::serve(endpoint, gateway, verify, stop).await;
        // UI socket closes do not reach this branch. The agent owns forwarding
        // shutdown; root resource expiry never stops this forwarding owner.
        monitor.abort();
        let stopped = backend.shutdown().await;
        let _ = log_stop.send(());
        let _ = logging.await;
        served?;
        stopped
    })
}
