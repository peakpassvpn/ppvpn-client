//! The engine in the mobile tunnel process (#448, docs/mobile.md): the iOS
//! packet tunnel extension and Android's VpnService. The native tunnel
//! builds the TUN from [`ppvpn_core::tun_settings`] and calls the C
//! functions below (include/ppvpn_tunnel.h, the contract); the UI process
//! talks Core API v1 to the engine over a Unix socket ([`server`], the wire
//! format of #448 comment 6393069163) and the host runs the health check
//! the UI process cannot run from the background ([`health`]).
//!
//! One host per process, in [`host`]. Every C function catches panics: an
//! unwind must not cross into Swift or Kotlin. iOS links the C functions
//! (a static library); Android loads libppvpn_tunnel.so and calls them
//! through JNI ([`android`]).
#![cfg(unix)]

#[cfg(target_os = "android")]
mod android;
mod health;
mod host;
mod log;
mod server;

use std::ffi::{c_char, c_void, CStr, CString};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;

use ppvpn_core::Platform;
use ppvpn_engine_host::ApiError;
use serde_json::Value;

pub use host::{ExitCallback, Options};

pub const PLATFORM_IOS: i32 = 1;
pub const PLATFORM_ANDROID: i32 = 2;

fn platform(value: i32) -> Option<Platform> {
    match value {
        PLATFORM_IOS => Some(Platform::Ios),
        PLATFORM_ANDROID => Some(Platform::Android),
        _ => None,
    }
}

pub(crate) fn api_error(code: &str, message: impl Into<String>, retryable: bool) -> ApiError {
    ApiError {
        code: code.to_string(),
        field: None,
        message: message.into(),
        retryable,
    }
}

pub(crate) fn request_invalid(message: impl Into<String>) -> ApiError {
    api_error("REQUEST_INVALID", message, false)
}

/// A string for the caller, who frees it with [`ppvpn_tunnel_free_string`].
fn owned(text: String) -> *mut c_char {
    // JSON from serde has no NUL; anything else is cut at one.
    let text = match CString::new(text) {
        Ok(text) => text,
        Err(error) => {
            let at = error.nul_position();
            let mut bytes = error.into_vec();
            bytes.truncate(at);
            CString::new(bytes).unwrap_or_default()
        }
    };
    text.into_raw()
}

fn error_json(error: &ApiError) -> *mut c_char {
    owned(serde_json::to_string(error).unwrap_or_default())
}

/// NULL on success, else the error as JSON.
fn result(outcome: std::thread::Result<Result<(), ApiError>>) -> *mut c_char {
    match outcome {
        Ok(Ok(())) => std::ptr::null_mut(),
        Ok(Err(error)) => error_json(&error),
        Err(_) => error_json(&api_error(
            "TUNNEL_INTERNAL",
            "the tunnel host panicked",
            false,
        )),
    }
}

/// A required UTF-8 argument.
///
/// # Safety
/// `text` is NULL or a NUL-terminated string valid for the call.
unsafe fn text<'a>(text: *const c_char, name: &str) -> Result<&'a str, ApiError> {
    if text.is_null() {
        return Err(request_invalid(format!("{name} is required")));
    }
    CStr::from_ptr(text)
        .to_str()
        .map_err(|_| request_invalid(format!("{name} is not UTF-8")))
}

/// The tunnel to build, as JSON; NULL for an unknown platform.
#[no_mangle]
pub extern "C" fn ppvpn_tunnel_settings(platform_id: i32) -> *mut c_char {
    catch_unwind(|| {
        let settings = platform(platform_id).and_then(ppvpn_core::tun_settings)?;
        serde_json::to_string(&settings).ok().map(owned)
    })
    .ok()
    .flatten()
    .unwrap_or(std::ptr::null_mut())
}

/// Starts the host; NULL once it serves the socket, else the error.
///
/// # Safety
/// The strings are NULL or NUL-terminated and valid for the call; `on_exit`
/// and `context` stay valid until [`ppvpn_tunnel_stop`] returns.
#[no_mangle]
pub unsafe extern "C" fn ppvpn_tunnel_start(
    tun_fd: i32,
    socket_path: *const c_char,
    state_dir: *const c_char,
    platform_id: i32,
    options: *const c_char,
    on_exit: Option<extern "C" fn(*mut c_void, *const c_char)>,
    context: *mut c_void,
) -> *mut c_char {
    result(catch_unwind(AssertUnwindSafe(|| {
        if tun_fd < 0 {
            return Err(request_invalid("tun_fd must be an open file descriptor"));
        }
        let socket = PathBuf::from(text(socket_path, "socket_path")?);
        let state = PathBuf::from(text(state_dir, "state_dir")?);
        let platform = platform(platform_id)
            .ok_or_else(|| request_invalid(format!("unknown platform {platform_id}")))?;
        let options = if options.is_null() {
            Options::default()
        } else {
            Options::parse(text(options, "options")?)?
        };
        let exit = on_exit.map(|function| ExitCallback::new(function, context));
        host::start(tun_fd, &socket, &state, platform, options, exit)
    })))
}

/// Stops the host; nothing without one.
#[no_mangle]
pub extern "C" fn ppvpn_tunnel_stop() {
    let _ = catch_unwind(host::stop);
}

/// The device's network (ppvpn_core::NetworkState as JSON).
///
/// # Safety
/// `network_state` is NULL or NUL-terminated and valid for the call.
#[no_mangle]
pub unsafe extern "C" fn ppvpn_tunnel_network_changed(network_state: *const c_char) -> *mut c_char {
    result(catch_unwind(AssertUnwindSafe(|| {
        network_changed(text(network_state, "network_state")?)
    })))
}

pub(crate) fn network_changed(json: &str) -> Result<(), ApiError> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| request_invalid(format!("network_state: {error}")))?;
    let state = serde_json::from_value(value)
        .map_err(|error| request_invalid(format!("network_state: {error}")))?;
    host::network_changed(state)
}

#[no_mangle]
pub extern "C" fn ppvpn_tunnel_sleep() {
    let _ = catch_unwind(host::sleep);
}

#[no_mangle]
pub extern "C" fn ppvpn_tunnel_wake() {
    let _ = catch_unwind(host::wake);
}

#[no_mangle]
pub extern "C" fn ppvpn_tunnel_version() -> *const c_char {
    concat!(env!("CARGO_PKG_VERSION"), "\0").as_ptr().cast()
}

/// Frees a string this library returned.
///
/// # Safety
/// `s` is NULL or a string from this library, freed once.
#[no_mangle]
pub unsafe extern "C" fn ppvpn_tunnel_free_string(s: *mut c_char) {
    if !s.is_null() {
        drop(CString::from_raw(s));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn take(text: *mut c_char) -> Option<String> {
        if text.is_null() {
            return None;
        }
        let owned = unsafe { CStr::from_ptr(text) }
            .to_str()
            .unwrap()
            .to_string();
        unsafe { ppvpn_tunnel_free_string(text) };
        Some(owned)
    }

    #[test]
    fn settings_are_the_engines() {
        let json = take(ppvpn_tunnel_settings(PLATFORM_IOS)).unwrap();
        let settings: ppvpn_core::TunSettings = serde_json::from_str(&json).unwrap();
        assert_eq!(Some(settings), ppvpn_core::tun_settings(Platform::Ios));
        assert!(take(ppvpn_tunnel_settings(PLATFORM_ANDROID)).is_some());
        assert!(take(ppvpn_tunnel_settings(0)).is_none());
    }

    #[test]
    fn version_is_the_crates() {
        let version = unsafe { CStr::from_ptr(ppvpn_tunnel_version()) };
        assert_eq!(version.to_str().unwrap(), env!("CARGO_PKG_VERSION"));
    }

    fn code(error: Option<String>) -> String {
        let error: Value = serde_json::from_str(&error.expect("an error")).unwrap();
        error["code"].as_str().unwrap().to_string()
    }

    #[test]
    fn bad_arguments_are_refused() {
        let socket = CString::new("/tmp/x.sock").unwrap();
        let state = CString::new("/tmp/x").unwrap();
        let start = |fd, socket: *const c_char, platform| {
            take(unsafe {
                ppvpn_tunnel_start(
                    fd,
                    socket,
                    state.as_ptr(),
                    platform,
                    std::ptr::null(),
                    None,
                    std::ptr::null_mut(),
                )
            })
        };
        assert_eq!(
            code(start(-1, socket.as_ptr(), PLATFORM_IOS)),
            "REQUEST_INVALID"
        );
        assert_eq!(
            code(start(3, std::ptr::null(), PLATFORM_IOS)),
            "REQUEST_INVALID"
        );
        assert_eq!(code(start(3, socket.as_ptr(), 9)), "REQUEST_INVALID");

        let network = |json: &str| {
            let json = CString::new(json).unwrap();
            take(unsafe { ppvpn_tunnel_network_changed(json.as_ptr()) })
        };
        assert_eq!(code(network("not json")), "REQUEST_INVALID");
        assert_eq!(code(network(r#"{"type": "satellite"}"#)), "REQUEST_INVALID");
        assert_eq!(
            code(take(unsafe {
                ppvpn_tunnel_network_changed(std::ptr::null())
            })),
            "REQUEST_INVALID"
        );
    }

    #[test]
    fn stop_sleep_and_wake_without_a_host_do_nothing() {
        let _serial = host::test_lock();
        ppvpn_tunnel_stop();
        ppvpn_tunnel_sleep();
        ppvpn_tunnel_wake();
    }
}
