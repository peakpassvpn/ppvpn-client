//! macOS client identity: the code signature of the process at the other end
//! of the socket (docs/security.md, "桌面 IPC").
//!
//! Releases are signed with our Apple Developer ID (#471), so the requirement
//! is Apple's for a Developer ID signature of our team: anchored at Apple's
//! root, the Developer ID intermediate and leaf, and the Team ID in the
//! leaf's subject (OU), which only Apple issues. Unlike a pin of one
//! certificate, it holds across certificate renewals. The Team ID lives in
//! macos_team_id only; the release builds refuse a signing identity of
//! another team (tools/desktop/ci/build-macos-native.sh,
//! tools/cli/build-release.sh).
//!
//! ppvpn-service-install checks the service binary against the same
//! requirement before it copies it to root (src/install.rs).
// Both binaries include this module and use different parts of it.
#![allow(dead_code)]

use anyhow::{anyhow, Result};
use core_foundation::base::{CFType, TCFType};
use core_foundation::data::CFData;
use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
use core_foundation::number::CFNumber;
use core_foundation::string::{CFString, CFStringRef};
use security_framework::os::macos::code_signing::{
    Flags, GuestAttributes, SecCode, SecRequirement,
};
use std::str::FromStr;

/// Bundle identifier of PPVPN.app (apps/flutter/scripts/ci-build.sh).
pub const CLIENT_IDENTIFIER: &str = "com.peakpassvpn.ppvpn.desktop";
/// The service binary's signing identifier (apps/macos/scripts/package-app.sh).
pub const SERVICE_IDENTIFIER: &str = "com.peakpassvpn.ppvpn.service";
/// ppvpn-service-install's, which a silent update runs (update.rs).
pub const INSTALL_IDENTIFIER: &str = "com.peakpassvpn.ppvpn.service-install";

const TEAM_ID: &str = include_str!("macos_team_id");

/// kSecCodeSignatureRuntime: the hardened runtime, which keeps other code
/// (DYLD_INSERT_LIBRARIES, unsigned libraries) out of the client.
const SIGNATURE_RUNTIME: u32 = 0x0001_0000;
/// kSecCSSigningInformation.
const SIGNING_INFORMATION: u32 = 1 << 1;

#[link(name = "Security", kind = "framework")]
extern "C" {
    static kSecCodeInfoFlags: CFStringRef;
    fn SecCodeCopySigningInformation(
        code: *const std::ffi::c_void,
        flags: u32,
        information: *mut CFDictionaryRef,
    ) -> i32;
}

/// Our Apple Developer Team ID.
pub fn team_id() -> &'static str {
    TEAM_ID.trim()
}

/// The requirement a client must meet: our identifier, signed with a
/// Developer ID of `team`.
pub fn client_requirement(team: &str) -> Result<SecRequirement> {
    requirement(CLIENT_IDENTIFIER, team)
}

/// `identifier`, signed with a Developer ID Application certificate of
/// `team` (the designated requirement codesign gives such a signature).
pub fn requirement(identifier: &str, team: &str) -> Result<SecRequirement> {
    SecRequirement::from_str(&requirement_text(identifier, team)?)
        .map_err(|e| anyhow!("CLIENT_REQUIREMENT_INVALID:{e}"))
}

fn requirement_text(identifier: &str, team: &str) -> Result<String> {
    if team.len() != 10
        || !team
            .bytes()
            .all(|b| b.is_ascii_uppercase() || b.is_ascii_digit())
    {
        return Err(anyhow!("TEAM_ID_INVALID"));
    }
    Ok(format!(
        "anchor apple generic and identifier \"{identifier}\" \
         and certificate 1[field.1.2.840.113635.100.6.2.6] \
         and certificate leaf[field.1.2.840.113635.100.6.1.13] \
         and certificate leaf[subject.OU] = \"{team}\""
    ))
}

/// Checks the running process behind an audit token (from LOCAL_PEERTOKEN):
/// the kernel's view of the code, not a path that could be swapped after
/// the check.
pub fn check_process(audit_token: &[u8], requirement: &SecRequirement) -> Result<()> {
    let mut attributes = GuestAttributes::new();
    let token = CFData::from_buffer(audit_token);
    attributes.set_audit_token(token.as_concrete_TypeRef());
    let code = SecCode::copy_guest_with_attribues(None, &attributes, Flags::NONE)
        .map_err(|e| anyhow!("CLIENT_CODE_UNAVAILABLE:{e}"))?;
    code.check_validity(Flags::NONE, requirement)
        .map_err(|e| anyhow!("CLIENT_SIGNATURE_REJECTED:{e}"))?;
    require_runtime(code.as_concrete_TypeRef() as *const std::ffi::c_void)
}

/// The same check against a file on disk.
pub fn check_file(path: &std::path::Path, requirement: &SecRequirement) -> Result<()> {
    let url = core_foundation::url::CFURL::from_path(path, false)
        .ok_or_else(|| anyhow!("CLIENT_PATH_INVALID"))?;
    use security_framework::os::macos::code_signing::SecStaticCode;

    let code = SecStaticCode::from_path(&url, Flags::NONE)
        .map_err(|e| anyhow!("CLIENT_CODE_UNAVAILABLE:{e}"))?;
    code.check_validity(Flags::NONE, requirement)
        .map_err(|e| anyhow!("CLIENT_SIGNATURE_REJECTED:{e}"))?;
    require_runtime(code.as_concrete_TypeRef() as *const std::ffi::c_void)
}

/// `code` is a SecCodeRef or SecStaticCodeRef, both accepted by
/// SecCodeCopySigningInformation.
fn require_runtime(code: *const std::ffi::c_void) -> Result<()> {
    let mut information: CFDictionaryRef = std::ptr::null();
    let status =
        unsafe { SecCodeCopySigningInformation(code, SIGNING_INFORMATION, &mut information) };
    if status != 0 || information.is_null() {
        return Err(anyhow!("CLIENT_SIGNING_INFORMATION_UNAVAILABLE:{status}"));
    }
    let information: CFDictionary<CFString, CFType> =
        unsafe { CFDictionary::wrap_under_create_rule(information) };
    let key = unsafe { CFString::wrap_under_get_rule(kSecCodeInfoFlags) };
    let flags = information
        .find(&key)
        .and_then(|value| value.downcast::<CFNumber>())
        .and_then(|number| number.to_i64())
        .unwrap_or(0) as u32;
    if flags & SIGNATURE_RUNTIME == 0 {
        return Err(anyhow!("CLIENT_NOT_HARDENED_RUNTIME"));
    }
    Ok(())
}
