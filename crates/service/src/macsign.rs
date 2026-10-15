//! macOS client identity: the code signature of the process at the other end
//! of the socket (docs/security.md, "桌面 IPC").
//!
//! Our certificate is self-signed and has no Team ID, so the requirement pins
//! the certificate itself (`certificate leaf = H"<SHA-1>"`), never its subject:
//! anyone can make a certificate with the same common name. The hash lives in
//! macos_client_cert.sha1 only; the release build refuses a signing
//! certificate that does not match it (tools/desktop/ci/build-macos-native.sh).

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

/// Bundle identifier of PPVPN.app (apps/macos/project.yml).
pub const CLIENT_IDENTIFIER: &str = "com.peakpassvpn.ppvpn.desktop";

const PINNED_CERT_SHA1: &str = include_str!("macos_client_cert.sha1");

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

pub fn pinned_cert_sha1() -> &'static str {
    PINNED_CERT_SHA1.trim()
}

/// The requirement a client must meet: our identifier, signed by the pinned
/// certificate.
pub fn client_requirement(cert_sha1: &str) -> Result<SecRequirement> {
    if cert_sha1.len() != 40 || !cert_sha1.bytes().all(|b| b.is_ascii_hexdigit()) {
        return Err(anyhow!("CLIENT_CERT_PIN_INVALID"));
    }
    let text =
        format!("identifier \"{CLIENT_IDENTIFIER}\" and certificate leaf = H\"{cert_sha1}\"");
    SecRequirement::from_str(&text).map_err(|e| anyhow!("CLIENT_REQUIREMENT_INVALID:{e}"))
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

/// The same check against a file, for tests and diagnostics.
#[cfg(test)]
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

#[cfg(test)]
mod tests {
    use super::{check_file, client_requirement, pinned_cert_sha1, CLIENT_IDENTIFIER};
    use std::path::{Path, PathBuf};
    use std::process::Command;

    #[test]
    fn pinned_hash_is_a_sha1() {
        assert!(client_requirement(pinned_cert_sha1()).is_ok());
        assert!(client_requirement("PPVPN Code Signing").is_err());
        assert!(client_requirement("e677633cfd8e675a02f901c811d7d8df7a6ecd6").is_err());
    }

    fn run(command: &mut Command) -> String {
        let output = command.output().expect("spawn");
        assert!(
            output.status.success(),
            "{command:?}: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8_lossy(&output.stdout).into_owned()
    }

    /// A throwaway keychain holding one self-signed code-signing certificate
    /// whose common name is the release one.
    struct Signer {
        keychain: PathBuf,
        sha1: String,
    }

    impl Signer {
        fn new(dir: &Path, name: &str) -> Self {
            let config = dir.join(format!("{name}.cnf"));
            std::fs::write(
                &config,
                "[req]\ndistinguished_name=dn\nprompt=no\nx509_extensions=v3\n\
                 [dn]\nCN=PPVPN Code Signing\n\
                 [v3]\nbasicConstraints=critical,CA:false\nkeyUsage=critical,digitalSignature\n\
                 extendedKeyUsage=critical,codeSigning\n",
            )
            .unwrap();
            let key = dir.join(format!("{name}.key"));
            let cert = dir.join(format!("{name}.pem"));
            let p12 = dir.join(format!("{name}.p12"));
            run(Command::new("/usr/bin/openssl")
                .args([
                    "req", "-x509", "-newkey", "rsa:2048", "-nodes", "-days", "1",
                ])
                .arg("-config")
                .arg(&config)
                .arg("-keyout")
                .arg(&key)
                .arg("-out")
                .arg(&cert));
            run(Command::new("/usr/bin/openssl")
                .args(["pkcs12", "-export", "-passout", "pass:test"])
                .arg("-inkey")
                .arg(&key)
                .arg("-in")
                .arg(&cert)
                .arg("-out")
                .arg(&p12));
            let fingerprint = run(Command::new("/usr/bin/openssl")
                .args(["x509", "-noout", "-fingerprint", "-sha1", "-in"])
                .arg(&cert));
            let sha1 = fingerprint
                .trim()
                .rsplit('=')
                .next()
                .unwrap()
                .replace(':', "")
                .to_ascii_lowercase();

            let keychain = dir.join(format!("{name}.keychain-db"));
            run(Command::new("security")
                .args(["create-keychain", "-p", "test"])
                .arg(&keychain));
            run(Command::new("security")
                .args(["unlock-keychain", "-p", "test"])
                .arg(&keychain));
            run(Command::new("security")
                .arg("import")
                .arg(&p12)
                .arg("-k")
                .arg(&keychain)
                .args(["-P", "test", "-T", "/usr/bin/codesign"]));
            run(Command::new("security")
                .args([
                    "set-key-partition-list",
                    "-S",
                    "apple-tool:,apple:,codesign:",
                    "-s",
                    "-k",
                    "test",
                ])
                .arg(&keychain));
            Signer { keychain, sha1 }
        }

        fn sign(&self, source: &Path, target: &Path, identifier: &str, runtime: bool) {
            std::fs::copy(source, target).unwrap();
            let mut command = Command::new("codesign");
            command
                .args(["--force", "--timestamp=none", "--sign", &self.sha1])
                .arg("--keychain")
                .arg(&self.keychain)
                .args(["--identifier", identifier]);
            if runtime {
                command.args(["--options", "runtime"]);
            }
            run(command.arg(target));
        }
    }

    impl Drop for Signer {
        fn drop(&mut self) {
            let _ = Command::new("security")
                .arg("delete-keychain")
                .arg(&self.keychain)
                .status();
        }
    }

    /// Two certificates with the same common name: only the pinned one
    /// passes, and only with our identifier and the hardened runtime.
    /// Touches keychains, so it runs where PPVPN_TEST_CODESIGN=1 (CI).
    #[test]
    fn only_the_pinned_certificate_passes() {
        if std::env::var_os("PPVPN_TEST_CODESIGN").is_none() {
            eprintln!("skipped: set PPVPN_TEST_CODESIGN=1");
            return;
        }
        let dir = std::env::temp_dir().join(format!("ppvpn-macsign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("main.c");
        std::fs::write(&source, "int main(void) { return 0; }\n").unwrap();
        let binary = dir.join("plain");
        run(Command::new("cc").arg(&source).arg("-o").arg(&binary));

        let ours = Signer::new(&dir, "ours");
        let other = Signer::new(&dir, "other");
        assert_ne!(ours.sha1, other.sha1);
        let requirement = client_requirement(&ours.sha1).unwrap();

        let good = dir.join("good");
        ours.sign(&binary, &good, CLIENT_IDENTIFIER, true);
        check_file(&good, &requirement).expect("pinned certificate, our identifier, runtime");

        let same_name = dir.join("same-name");
        other.sign(&binary, &same_name, CLIENT_IDENTIFIER, true);
        let error = check_file(&same_name, &requirement)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");

        let wrong_id = dir.join("wrong-id");
        ours.sign(&binary, &wrong_id, "com.example.other", true);
        let error = check_file(&wrong_id, &requirement).unwrap_err().to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");

        let no_runtime = dir.join("no-runtime");
        ours.sign(&binary, &no_runtime, CLIENT_IDENTIFIER, false);
        let error = check_file(&no_runtime, &requirement)
            .unwrap_err()
            .to_string();
        assert_eq!(error, "CLIENT_NOT_HARDENED_RUNTIME");

        let error = check_file(&binary, &requirement).unwrap_err().to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");

        drop((ours, other));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
