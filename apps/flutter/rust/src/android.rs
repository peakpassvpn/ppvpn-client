//! Android: the app's Kotlin from the UI process (runner/android), over JNI.
//! For now the saved sign-in's key (#473): `com.peakpassvpn.ppvpn.Credentials`
//! seals and opens the blob with an AES-GCM key in the Android Keystore.
//!
//! MainActivity loads this library (System.loadLibrary) before Dart opens
//! it by the same name, which gives Dart that same copy. So JNI_OnLoad
//! runs here, on the VM's thread, where FindClass sees the app's classes:
//! the class is looked up once there. The calls come later from the
//! client's threads, which FindClass would not serve.
#![allow(non_snake_case)] // JNI's names

use std::ffi::c_void;
use std::sync::OnceLock;

use jni::objects::{JByteArray, JClass, JValue};
use jni::refs::{Global, Reference as _};
use jni::strings::JNIStr;
use jni::sys::jint;
use jni::{jni_sig, jni_str, JavaVM};
use ppvpn_client::PlatformError;

static CREDENTIALS: OnceLock<Global<JClass<'static>>> = OnceLock::new();

/// Keeps the VM (jni's singleton) and the app's Credentials class.
///
/// # Safety
/// Called by the VM loading this library, with its own pointer.
#[no_mangle]
pub unsafe extern "system" fn JNI_OnLoad(
    vm: *mut jni::sys::JavaVM,
    _reserved: *mut c_void,
) -> jint {
    let vm = JavaVM::from_raw(vm);
    let found = vm.attach_current_thread(|env| -> jni::errors::Result<()> {
        let class = env.find_class(jni_str!("com/peakpassvpn/ppvpn/Credentials"))?;
        let _ = CREDENTIALS.set(env.new_global_ref(class)?);
        Ok(())
    });
    if let Err(error) = found {
        tracing::error!(%error, "com.peakpassvpn.ppvpn.Credentials not found: no saved sign-in");
    }
    jni::sys::JNI_VERSION_1_6
}

fn failure(message: impl std::fmt::Display) -> PlatformError {
    PlatformError::Failed {
        message: format!("keystore: {message}"),
    }
}

/// The blob sealed by the app's key (made on first use).
pub(crate) fn seal(blob: &[u8]) -> Result<Vec<u8>, PlatformError> {
    call(jni_str!("seal"), blob)?.ok_or_else(|| failure("seal returned nothing"))
}

/// The blob of a sealed one; None when its key is gone or it does not open
/// with it.
pub(crate) fn open(sealed: &[u8]) -> Result<Option<Vec<u8>>, PlatformError> {
    call(jni_str!("open"), sealed)
}

/// `static byte[] Credentials.<method>(byte[])`, null as None. A Java
/// exception comes back as the error.
fn call(method: &'static JNIStr, input: &[u8]) -> Result<Option<Vec<u8>>, PlatformError> {
    let class = CREDENTIALS
        .get()
        .ok_or_else(|| failure("the app's Credentials class was not found at load"))?;
    let vm = JavaVM::singleton().map_err(failure)?;
    vm.attach_current_thread(|env| -> jni::errors::Result<Option<Vec<u8>>> {
        let input = env.byte_array_from_slice(input)?;
        let output = env
            .call_static_method(class, method, jni_sig!("([B)[B"), &[JValue::Object(&input)])?
            .l()?;
        if output.is_null() {
            return Ok(None);
        }
        let output = env.cast_local::<JByteArray>(output)?;
        Ok(Some(env.convert_byte_array(&output)?))
    })
    .map_err(failure)
}
