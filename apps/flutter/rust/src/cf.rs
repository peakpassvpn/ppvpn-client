//! macOS: the few CoreFoundation calls the app needs, declared by hand (no
//! crate): the native app's user defaults (settings.rs) and keeping the
//! sign-in out of Time Machine (hooks.rs).

use std::ffi::{c_char, c_void};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

type CFTypeRef = *const c_void;
type CFStringRef = *const c_void;

const UTF8: u32 = 0x0800_0100; // kCFStringEncodingUTF8

#[link(name = "CoreFoundation", kind = "framework")]
extern "C" {
    static kCFBooleanTrue: CFTypeRef;
    static kCFURLIsExcludedFromBackupKey: CFStringRef;

    fn CFRelease(value: CFTypeRef);
    fn CFGetTypeID(value: CFTypeRef) -> usize;
    fn CFStringGetTypeID() -> usize;
    fn CFStringCreateWithBytes(
        allocator: CFTypeRef,
        bytes: *const u8,
        length: isize,
        encoding: u32,
        external: u8,
    ) -> CFStringRef;
    fn CFStringGetLength(string: CFStringRef) -> isize;
    fn CFStringGetMaximumSizeForEncoding(length: isize, encoding: u32) -> isize;
    fn CFStringGetCString(
        string: CFStringRef,
        buffer: *mut c_char,
        size: isize,
        encoding: u32,
    ) -> u8;
    fn CFPreferencesCopyAppValue(key: CFStringRef, application: CFStringRef) -> CFTypeRef;
    fn CFPreferencesSetAppValue(key: CFStringRef, value: CFTypeRef, application: CFStringRef);
    fn CFPreferencesAppSynchronize(application: CFStringRef) -> u8;
    fn CFURLCreateFromFileSystemRepresentation(
        allocator: CFTypeRef,
        path: *const u8,
        length: isize,
        directory: u8,
    ) -> CFTypeRef;
    fn CFURLSetResourcePropertyForKey(
        url: CFTypeRef,
        key: CFStringRef,
        value: CFTypeRef,
        error: *mut CFTypeRef,
    ) -> u8;
}

/// An owned CoreFoundation object, released on drop; never null.
struct Owned(CFTypeRef);

impl Owned {
    fn new(value: CFTypeRef) -> Option<Self> {
        (!value.is_null()).then_some(Self(value))
    }

    fn string(text: &str) -> Option<Self> {
        // SAFETY: the bytes are valid UTF-8 of the given length; CF copies them.
        Self::new(unsafe {
            CFStringCreateWithBytes(
                std::ptr::null(),
                text.as_ptr(),
                text.len() as isize,
                UTF8,
                0,
            )
        })
    }
}

impl Drop for Owned {
    fn drop(&mut self) {
        // SAFETY: an object this side owns (a Create or Copy result).
        unsafe { CFRelease(self.0) }
    }
}

/// The string value of `key` in the preferences of `domain` (the current
/// user's, as `UserDefaults.standard` of that app reads them); None when
/// unset or not a string.
pub(crate) fn preference(domain: &str, key: &str) -> Option<String> {
    let (domain, key) = (Owned::string(domain)?, Owned::string(key)?);
    // SAFETY: valid strings; the result is ours to release (Copy).
    let value = Owned::new(unsafe { CFPreferencesCopyAppValue(key.0, domain.0) })?;
    // SAFETY: a valid object.
    if unsafe { CFGetTypeID(value.0) != CFStringGetTypeID() } {
        return None;
    }
    // SAFETY: `value` is a CFString; the buffer is as large as CF asks for.
    unsafe {
        let size = CFStringGetMaximumSizeForEncoding(CFStringGetLength(value.0), UTF8) + 1;
        let mut buffer = vec![0u8; usize::try_from(size).ok()?];
        if CFStringGetCString(value.0, buffer.as_mut_ptr().cast(), size, UTF8) == 0 {
            return None;
        }
        let end = buffer.iter().position(|&byte| byte == 0)?;
        buffer.truncate(end);
        String::from_utf8(buffer).ok()
    }
}

/// Sets string values in the preferences of `domain` and writes them out.
pub(crate) fn set_preferences(domain: &str, values: &[(&str, &str)]) -> Result<(), String> {
    let failed = || format!("cannot write the preferences of {domain}");
    let domain_ref = Owned::string(domain).ok_or_else(failed)?;
    for (key, value) in values {
        let (key, value) = (
            Owned::string(key).ok_or_else(failed)?,
            Owned::string(value).ok_or_else(failed)?,
        );
        // SAFETY: valid strings; CF retains what it keeps.
        unsafe { CFPreferencesSetAppValue(key.0, value.0, domain_ref.0) };
    }
    // SAFETY: a valid string.
    if unsafe { CFPreferencesAppSynchronize(domain_ref.0) } == 0 {
        return Err(failed());
    }
    Ok(())
}

/// Marks `path` as excluded from Time Machine backups, as
/// `URLResourceValues.isExcludedFromBackup` does. Best effort.
pub(crate) fn exclude_from_backup(path: &Path) {
    let bytes = path.as_os_str().as_bytes();
    // SAFETY: the path's bytes and length; the URL is ours to release.
    let Some(url) = Owned::new(unsafe {
        CFURLCreateFromFileSystemRepresentation(
            std::ptr::null(),
            bytes.as_ptr(),
            bytes.len() as isize,
            0,
        )
    }) else {
        return;
    };
    // SAFETY: a valid URL and the framework's own key and value constants.
    unsafe {
        CFURLSetResourcePropertyForKey(
            url.0,
            kCFURLIsExcludedFromBackupKey,
            kCFBooleanTrue,
            std::ptr::null_mut(),
        );
    }
}
