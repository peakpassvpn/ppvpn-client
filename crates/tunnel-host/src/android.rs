//! Android's binding (#448): TunnelService, in the app's `:tunnel` process,
//! loads libppvpn_tunnel.so and calls `com.peakpassvpn.ppvpn.tunnel.TunnelHost`
//! (apps/flutter/runner/android), whose native methods are below. They are
//! the C functions of lib.rs with Java's types: an error is the same JSON,
//! returned as a String (null on success). The listener protects the
//! engine's sockets (`VpnService.protect`) and hears `on_exit`.
//!
//! The JNI calls run under jni's `with_env`, which catches panics: a Rust
//! error or panic there throws a RuntimeException in Java.
#![allow(non_snake_case)] // JNI's names

use std::ffi::{c_char, c_void, CStr};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};

use jni::errors::ThrowRuntimeExAndDefault;
use jni::objects::{JClass, JObject, JString};
use jni::refs::{Global, Reference as _};
use jni::sys::{jint, jstring};
use jni::{jni_sig, jni_str, Env, EnvUnowned, JValue, JavaVM};
use ppvpn_core::{Platform, SocketProtect};
use ppvpn_engine_host::ApiError;

use crate::host::{self, ExitCallback, Options};
use crate::request_invalid;

type Listener = Arc<Global<JObject<'static>>>;

/// The running host's listener, for `on_exit`.
static LISTENER: Mutex<Option<Listener>> = Mutex::new(None);

fn lock() -> MutexGuard<'static, Option<Listener>> {
    LISTENER
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Keeps the VM for the host's threads (jni's singleton), which call the
/// listener.
///
/// # Safety
/// Called by the VM loading this library, with its own pointer.
#[no_mangle]
pub unsafe extern "system" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    _reserved: *mut c_void,
) -> jint {
    let _ = JavaVM::from_raw(vm);
    jni::sys::JNI_VERSION_1_6
}

/// `f`'s text as a Java String, or null.
fn answer<'local>(
    env: &mut EnvUnowned<'local>,
    f: impl FnOnce(&mut Env<'local>) -> jni::errors::Result<Option<String>>,
) -> jstring {
    env.with_env(|env| -> jni::errors::Result<_> {
        f(env)?.map(|text| JString::from_str(env, text)).transpose()
    })
    .resolve::<ThrowRuntimeExAndDefault>()
    .map_or(std::ptr::null_mut(), |text| text.into_raw())
}

/// Runs `f` for a method that returns nothing.
fn run(env: &mut EnvUnowned<'_>, f: impl FnOnce()) {
    env.with_env(|_| -> jni::errors::Result<()> {
        f();
        Ok(())
    })
    .resolve::<ThrowRuntimeExAndDefault>()
}

fn failure(outcome: Result<(), ApiError>) -> Option<String> {
    outcome
        .err()
        .map(|error| serde_json::to_string(&error).unwrap_or_default())
}

/// `String settings()`: the tunnel to build, as JSON.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_settings<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jstring {
    answer(&mut env, |_| {
        Ok(ppvpn_core::tun_settings(Platform::Android)
            .and_then(|settings| serde_json::to_string(&settings).ok()))
    })
}

/// `String? start(int tunFd, String socketPath, String stateDir,
/// String? options, Listener listener)`: null once the host serves the
/// socket, else the error.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_start<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    tun_fd: jint,
    socket_path: JString<'local>,
    state_dir: JString<'local>,
    options: JString<'local>,
    listener: JObject<'local>,
) -> jstring {
    answer(&mut env, |env| {
        let socket = socket_path.try_to_string(env)?;
        let state = state_dir.try_to_string(env)?;
        let options = if options.is_null() {
            None
        } else {
            Some(options.try_to_string(env)?)
        };
        if listener.is_null() {
            return Ok(failure(Err(request_invalid("listener is required"))));
        }
        let listener = Arc::new(env.new_global_ref(&listener)?);
        Ok(failure(start(tun_fd, socket, state, options, listener)))
    })
}

fn start(
    tun_fd: i32,
    socket: String,
    state: String,
    options: Option<String>,
    listener: Listener,
) -> Result<(), ApiError> {
    if tun_fd < 0 {
        return Err(request_invalid("tun_fd must be an open file descriptor"));
    }
    let options = match options {
        Some(json) => Options::parse(&json)?,
        None => Options::default(),
    };
    let protect = {
        let listener = listener.clone();
        SocketProtect::new(move |fd| protect(&listener, fd))
    };
    let previous = lock().replace(listener);
    let started = host::start_with(
        tun_fd,
        &PathBuf::from(socket),
        &PathBuf::from(state),
        Platform::Android,
        options,
        Some(ExitCallback::new(on_exit, std::ptr::null_mut())),
        |config| config.with_socket_protect(protect),
    );
    if started.is_err() {
        // A host already running keeps its listener.
        *lock() = previous;
    }
    started
}

/// `VpnService.protect`, through the listener, on the engine's thread.
fn protect(listener: &Global<JObject<'static>>, fd: i32) -> std::io::Result<()> {
    let vm = JavaVM::singleton().map_err(|error| std::io::Error::other(error.to_string()))?;
    let protected = vm
        .attach_current_thread(|env| -> jni::errors::Result<bool> {
            env.call_method(
                listener,
                jni_str!("protect"),
                jni_sig!("(I)Z"),
                &[JValue::Int(fd)],
            )?
            .z()
        })
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    if protected {
        Ok(())
    } else {
        Err(std::io::Error::other(
            "VpnService.protect refused the socket",
        ))
    }
}

/// The host gave up: `Listener.onExit(reason)`, on the host's exit thread.
extern "C" fn on_exit(_context: *mut c_void, reason: *const c_char) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(listener) = lock().clone() else {
            return;
        };
        // Safety: the host passes a NUL-terminated string, valid for the call.
        let reason = unsafe { CStr::from_ptr(reason) }
            .to_string_lossy()
            .into_owned();
        let Ok(vm) = JavaVM::singleton() else {
            return;
        };
        let _ = vm.attach_current_thread_for_scope(|env| -> jni::errors::Result<()> {
            let reason = JString::from_str(env, reason)?;
            env.call_method(
                &*listener,
                jni_str!("onExit"),
                jni_sig!("(Ljava/lang/String;)V"),
                &[JValue::Object(&reason)],
            )?;
            Ok(())
        });
    }));
}

/// `void stop()`.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_stop<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    run(&mut env, || {
        host::stop();
        *lock() = None;
    });
}

/// `String? networkChanged(String networkState)`.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_networkChanged<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
    network_state: JString<'local>,
) -> jstring {
    answer(&mut env, |env| {
        let json = network_state.try_to_string(env)?;
        Ok(failure(crate::network_changed(&json)))
    })
}

/// `void sleep()`.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_sleep<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    run(&mut env, host::sleep);
}

/// `void wake()`.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_wake<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) {
    run(&mut env, host::wake);
}

/// `String version()`.
#[no_mangle]
pub extern "system" fn Java_com_peakpassvpn_ppvpn_tunnel_TunnelHost_version<'local>(
    mut env: EnvUnowned<'local>,
    _class: JClass<'local>,
) -> jstring {
    answer(&mut env, |_| {
        Ok(Some(env!("CARGO_PKG_VERSION").to_string()))
    })
}
