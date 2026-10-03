//! ppvpn-service — Windows service that hosts ppvpn-core with the
//! privileges required for transparent routing.
//!
//! Architecture (mirrored from clash-verge-service):
//!   - Registered as Windows service `ppvpn_service` by install.rs
//!   - Service dispatcher enters `service_entry`, which starts the IPC server
//!   - IPC server listens on named pipe `\\.\pipe\ppvpn-service`
//!   - Main app uses session-bound Connect / RenewLease / Disconnect commands

mod core;
mod ipc;
mod logfile;
mod macdns;
#[cfg(any(target_os = "linux", test))]
mod netclean;
mod protocol;
mod watch;

use log::info;

fn setup_logger() {
    if let Err(error) = logfile::init_service_logger() {
        eprintln!("cannot open the service log: {error}");
    }
}

/// Stops a core whose lease lapsed, and notices a core that exited on its
/// own (its watchers learn within a tick).
fn start_lease_watchdog() {
    std::thread::spawn(|| loop {
        std::thread::sleep(std::time::Duration::from_millis(250));
        let mut core = core::CORE.lock();
        core.expire_lease();
        core.reap_exited();
    });
}

/// What a previous service instance killed while connected (kill -9,
/// crash, systemd's stop timeout) can leave behind, and the core binary's
/// first run.
#[cfg(unix)]
fn clean_up_after_previous_instance() {
    let started = std::time::Instant::now();
    {
        // Under the lock: a Connect that came first owns what is there now.
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut manager = core::CORE.lock();
        if !manager.has_core() {
            let core_running = core::core_process_running();
            // Its core's policy rules.
            #[cfg(target_os = "linux")]
            netclean::clean_if_no_core(&netclean::SystemRunner, core_running, "service start");
            // Its system DNS override: the TUN it points at is gone.
            #[cfg(target_os = "macos")]
            manager.clean_dns_leftover(core_running);
        }
    }
    #[cfg(target_os = "macos")]
    core::warm_up_core_binary();
    info!(
        "startup clean-up done in {} ms",
        started.elapsed().as_millis()
    );
}

#[cfg(windows)]
fn main() -> windows_service::Result<()> {
    setup_logger();
    info!(
        "ppvpn-service v{} ({}) starting; logs in {}",
        env!("CARGO_PKG_VERSION"),
        protocol::SERVICE_BUILD_ID,
        logfile::log_dir().display()
    );
    start_lease_watchdog();
    service_module::dispatch()
}

/// Enters the stopping state (Connect and friends are refused from now on,
/// see `ipc::begin_stopping`), tells every watch connection `stopping` and
/// closes them, then stops ppvpn-core in order (`/v1/stop`, then closing its
/// lifetime pipe, then a kill after the grace period) so it removes its TUN
/// device, routes and DNS settings, and exits. The core lock stays held
/// until the process is gone.
#[cfg(unix)]
fn stop_data_plane_and_exit() -> ! {
    stop_data_plane();
    log::logger().flush();
    std::process::exit(0)
}

/// Stops taking work, tells the watchers, stops the core and waits for
/// the system DNS changes.
fn stop_data_plane() {
    ipc::begin_stopping();
    let started = std::time::Instant::now();
    let mut core = core::stop_sequence(&core::CORE, &watch::HUB, watch::CLOSE_GRACE);
    if let Err(error) = core.stop_because(core::stop_reason::SERVICE_STOPPING) {
        log::error!("failed to stop data plane during service shutdown: {error}");
    }
    core.flush_dns();
    info!(
        "data plane stopped in {} ms; service exiting",
        started.elapsed().as_millis()
    );
}

#[cfg(not(windows))]
fn main() {
    setup_logger();
    info!(
        "ppvpn-service v{} ({}) starting (Unix privileged service); logs in {}",
        env!("CARGO_PKG_VERSION"),
        protocol::SERVICE_BUILD_ID,
        logfile::log_dir().display()
    );
    start_lease_watchdog();
    let Ok(rt) = tokio::runtime::Runtime::new() else {
        return;
    };
    // The IPC server accepts with blocking calls, so it gets a thread of
    // its own: run on this one (inside a select! with the signals, as it
    // was), it never yielded and SIGTERM was never seen. launchd / systemd
    // then killed the service (and its core) after the stop timeout, and
    // the core's routes stayed behind.
    let handle = rt.handle().clone();
    let server = std::thread::Builder::new()
        .name("ipc-server".into())
        .spawn(move || {
            if let Err(error) = handle.block_on(ipc::run_ipc_server()) {
                log::error!("ipc server error: {error:#}");
            }
            // Also reached when the IPC server fails: never leave a core
            // behind.
            stop_data_plane_and_exit()
        });
    if let Err(error) = server {
        log::error!("cannot start the IPC server thread: {error}");
        stop_data_plane_and_exit()
    }
    // After the IPC server: a client that installed or restarted the
    // service waits for it to answer, and these can take a while.
    std::thread::spawn(clean_up_after_previous_instance);
    rt.block_on(async {
        use tokio::signal::unix::{signal, SignalKind};
        // launchd (bootout) and systemd (stop) send SIGTERM; the default
        // action would kill the service with the core's routes still up.
        match (
            signal(SignalKind::terminate()),
            signal(SignalKind::interrupt()),
        ) {
            (Ok(mut terminate), Ok(mut interrupt)) => {
                tokio::select! {
                    _ = terminate.recv() => info!("SIGTERM received; stopping the data plane"),
                    _ = interrupt.recv() => info!("SIGINT received; stopping the data plane"),
                }
            }
            _ => {
                log::error!("cannot install SIGTERM/SIGINT handlers");
                std::future::pending::<()>().await;
            }
        }
    });
    stop_data_plane_and_exit()
}

#[cfg(windows)]
mod service_module {
    use super::ipc;
    use log::{error, info};
    use std::ffi::OsString;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::OnceLock;
    use std::time::Duration;
    use windows_service::service_control_handler::ServiceStatusHandle;
    use windows_service::{
        define_windows_service,
        service::{
            ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
            ServiceType,
        },
        service_control_handler::{self, ServiceControlHandlerResult},
        service_dispatcher, Result,
    };

    const SERVICE_NAME: &str = "ppvpn_service";
    const SERVICE_TYPE: ServiceType = ServiceType::OWN_PROCESS;
    /// SCM's patience while stopping (the core's stop takes up to ~17 s).
    const STOP_WAIT_HINT: Duration = Duration::from_secs(30);

    static STATUS: OnceLock<ServiceStatusHandle> = OnceLock::new();
    static STOPPING: AtomicBool = AtomicBool::new(false);

    fn report(state: ServiceState, controls: ServiceControlAccept, wait_hint: Duration) {
        if let Some(handle) = STATUS.get() {
            let status = ServiceStatus {
                service_type: SERVICE_TYPE,
                current_state: state,
                controls_accepted: controls,
                exit_code: ServiceExitCode::Win32(0),
                checkpoint: 0,
                wait_hint,
                process_id: None,
            };
            if let Err(error) = handle.set_service_status(status) {
                error!("cannot report service state {state:?}: {error}");
            }
        }
    }

    /// Off the control handler's thread: the handler answers SCM at once
    /// (exiting inside it left `sc stop` with "109: The pipe has been
    /// ended" although the service stopped fine), and SCM hears
    /// STOP_PENDING, then STOPPED before the process exits.
    fn stop_and_exit() {
        report(
            ServiceState::StopPending,
            ServiceControlAccept::empty(),
            STOP_WAIT_HINT,
        );
        super::stop_data_plane();
        report(
            ServiceState::Stopped,
            ServiceControlAccept::empty(),
            Duration::default(),
        );
        log::logger().flush();
        std::process::exit(0)
    }

    pub fn dispatch() -> Result<()> {
        service_dispatcher::start(SERVICE_NAME, ffi_service_main)
    }

    define_windows_service!(ffi_service_main, service_main);

    fn service_main(_args: Vec<OsString>) {
        if let Err(e) = run_service() {
            error!("service_main failed: {e}");
        }
    }

    fn run_service() -> Result<()> {
        let handle = service_control_handler::register(
            SERVICE_NAME,
            |event| -> ServiceControlHandlerResult {
                match event {
                    ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
                    ServiceControl::Stop | ServiceControl::Shutdown => {
                        // ppvpn-core owns the active TUN routes: they are
                        // removed (the core stopped in order) before the
                        // process exits, so Windows gets its previous
                        // network state back. Once, whatever SCM sends.
                        if !STOPPING.swap(true, Ordering::SeqCst) {
                            std::thread::spawn(stop_and_exit);
                        }
                        ServiceControlHandlerResult::NoError
                    }
                    _ => ServiceControlHandlerResult::NotImplemented,
                }
            },
        )?;
        let _ = STATUS.set(handle);
        handle.set_service_status(ServiceStatus {
            service_type: SERVICE_TYPE,
            current_state: ServiceState::Running,
            controls_accepted: ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
            exit_code: ServiceExitCode::Win32(0),
            checkpoint: 0,
            wait_hint: Duration::default(),
            process_id: None,
        })?;

        info!("service entering IPC loop");

        if let Ok(rt) = tokio::runtime::Runtime::new() {
            rt.block_on(async {
                if let Err(e) = ipc::run_ipc_server().await {
                    error!("ipc server error: {e}");
                }
            });
        }

        Ok(())
    }
}
