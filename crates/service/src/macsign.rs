//! Shared production identity checks; signature fixtures remain service-only.
#[path = "mac_identity.rs"]
mod implementation;
pub use implementation::*;

#[cfg(test)]
mod tests {
    use super::{
        check_file, client_requirement, requirement, team_id, CLIENT_IDENTIFIER, SERVICE_IDENTIFIER,
    };
    use security_framework::os::macos::code_signing::SecRequirement;
    use std::path::{Path, PathBuf};
    use std::process::Command;
    use std::str::FromStr;

    #[test]
    #[ignore = "requires PPVPN_CODESIGN_FIXTURE: actual hardened Developer ID helper"]
    fn actual_standalone_image_preserves_full_static_validation_after_relocation() {
        let source = std::env::var_os("PPVPN_CODESIGN_FIXTURE").expect("signed fixture path");
        let directory =
            std::env::temp_dir().join(format!("ppvpn-static-sign-{}", uuid::Uuid::new_v4()));
        let contents = directory.join("helper.bundle/Contents");
        let image = contents.join("MacOS/ppvpn-service");
        std::fs::create_dir_all(image.parent().unwrap()).unwrap();
        std::fs::copy(source, &image).unwrap();
        let info = contents.join("Info.plist");
        std::fs::write(
            &info,
            r#"<?xml version="1.0"?><plist version="1.0"><dict>
            <key>CFBundleIdentifier</key><string>com.peakpassvpn.ppvpn.service</string>
            <key>CFBundleExecutable</key><string>ppvpn-service</string>
            <key>CFBundlePackageType</key><string>BNDL</string>
            </dict></plist>"#,
        )
        .unwrap();
        let policy = requirement(SERVICE_IDENTIFIER, team_id()).unwrap();
        assert!(
            check_file(&image, &policy).is_err(),
            "unsigned metadata must not be accepted"
        );
        std::fs::remove_file(info).unwrap();
        check_file(&image, &policy)
            .expect("full Security.framework validation and hardened runtime");
        // Prove this remains executable validation, not only signer matching.
        use std::io::{Read, Seek, SeekFrom, Write};
        let mut file = std::fs::OpenOptions::new()
            .read(true)
            .write(true)
            .open(&image)
            .unwrap();
        file.seek(SeekFrom::Start(4096)).unwrap();
        let mut byte = [0];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 1;
        file.seek(SeekFrom::Start(4096)).unwrap();
        file.write_all(&byte).unwrap();
        file.sync_all().unwrap();
        assert!(
            check_file(&image, &policy).is_err(),
            "modified code must still be rejected"
        );
        drop(file);
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn the_team_id_is_one() {
        assert_eq!(team_id(), "HSVMRQBQJ2");
        assert!(client_requirement(team_id()).is_ok());
        assert!(client_requirement("PPVPN Code Signing").is_err());
        assert!(client_requirement("hsvmrqbqj2").is_err());
        assert!(client_requirement("HSVMRQBQJ").is_err());
        assert!(client_requirement("HSVMRQBQJ2\" or true").is_err());
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
    /// that names itself our Developer ID: the release's common name and our
    /// Team ID as its OU.
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
                 [dn]\nCN=Developer ID Application: PeakPass Labs LLC (HSVMRQBQJ2)\n\
                 OU=HSVMRQBQJ2\n\
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

    struct SearchList(Vec<String>);

    impl SearchList {
        fn with(keychains: &[&Path]) -> Self {
            let original: Vec<String> =
                run(Command::new("security").args(["list-keychains", "-d", "user"]))
                    .lines()
                    .map(|line| line.trim().trim_matches('"').to_string())
                    .filter(|line| !line.is_empty())
                    .collect();
            run(Command::new("security")
                .args(["list-keychains", "-d", "user", "-s"])
                .args(keychains)
                .args(&original));
            SearchList(original)
        }
    }

    impl Drop for SearchList {
        fn drop(&mut self) {
            let _ = Command::new("security")
                .args(["list-keychains", "-d", "user", "-s"])
                .args(&self.0)
                .status();
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

    /// A Developer ID-signed app on the runner (its own identifier and
    /// team), to check the requirement's form against a real signature.
    fn developer_id_app() -> (PathBuf, String, String) {
        for app in [
            "/Applications/Google Chrome.app",
            "/Applications/Firefox.app",
            "/Applications/Microsoft Edge.app",
        ] {
            let path = PathBuf::from(app);
            if !path.exists() {
                continue;
            }
            let output = Command::new("codesign")
                .args(["-dv", "--verbose=2"])
                .arg(&path)
                .output()
                .expect("spawn");
            let text = String::from_utf8_lossy(&output.stderr).into_owned();
            let field = |name: &str| {
                text.lines()
                    .find_map(|line| line.strip_prefix(name))
                    .map(str::to_string)
            };
            if let (Some(identifier), Some(team)) = (field("Identifier="), field("TeamIdentifier="))
            {
                return (path, identifier, team);
            }
        }
        panic!("no Developer ID-signed app on this machine");
    }

    /// Only a Developer ID signature of our team passes, with our
    /// identifier and the hardened runtime; a self-signed certificate that
    /// names itself ours does not.
    /// Touches keychains, so it runs where PPVPN_TEST_CODESIGN=1 (CI).
    #[test]
    fn only_a_developer_id_signature_of_the_team_passes() {
        if std::env::var_os("PPVPN_TEST_CODESIGN").is_none() {
            eprintln!("skipped: set PPVPN_TEST_CODESIGN=1");
            return;
        }
        // The form, against a real Developer ID signature.
        let (app, identifier, team) = developer_id_app();
        check_file(&app, &requirement(&identifier, &team).unwrap())
            .expect("its identifier and team");
        let error = check_file(&app, &requirement(&identifier, "ABCDE12345").unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");
        let error = check_file(&app, &requirement(CLIENT_IDENTIFIER, &team).unwrap())
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");

        let dir = std::env::temp_dir().join(format!("ppvpn-macsign-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let source = dir.join("main.c");
        std::fs::write(&source, "int main(void) { return 0; }\n").unwrap();
        let binary = dir.join("plain");
        run(Command::new("cc").arg(&source).arg("-o").arg(&binary));

        let ours = Signer::new(&dir, "ours");
        let other = Signer::new(&dir, "other");
        // codesign finds an identity only in the search list (as the
        // release build does it); the runner's list comes back afterwards.
        let _search_list = SearchList::with(&[&ours.keychain, &other.keychain]);
        assert_ne!(ours.sha1, other.sha1);

        // Not anchored at Apple: refused, whatever it calls itself.
        let client = client_requirement(team_id()).unwrap();
        let service = requirement(SERVICE_IDENTIFIER, team_id()).unwrap();
        let forged = dir.join("forged");
        ours.sign(&binary, &forged, CLIENT_IDENTIFIER, true);
        let error = check_file(&forged, &client).unwrap_err().to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");
        let forged_service = dir.join("forged-service");
        ours.sign(&binary, &forged_service, SERVICE_IDENTIFIER, true);
        let error = check_file(&forged_service, &service)
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");
        let error = check_file(&binary, &client).unwrap_err().to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");

        // The hardened runtime is required on top of the requirement (here
        // one pinning the throwaway certificate).
        let pinned = |sha1: &str| {
            SecRequirement::from_str(&format!(
                "identifier \"{CLIENT_IDENTIFIER}\" and certificate leaf = H\"{sha1}\""
            ))
            .unwrap()
        };
        let good = dir.join("good");
        ours.sign(&binary, &good, CLIENT_IDENTIFIER, true);
        check_file(&good, &pinned(&ours.sha1)).expect("the certificate, the runtime");
        let error = check_file(&good, &pinned(&other.sha1))
            .unwrap_err()
            .to_string();
        assert!(error.starts_with("CLIENT_SIGNATURE_REJECTED"), "{error}");
        let no_runtime = dir.join("no-runtime");
        ours.sign(&binary, &no_runtime, CLIENT_IDENTIFIER, false);
        let error = check_file(&no_runtime, &pinned(&ours.sha1))
            .unwrap_err()
            .to_string();
        assert_eq!(error, "CLIENT_NOT_HARDENED_RUNTIME");

        drop((_search_list, ours, other));
        let _ = std::fs::remove_dir_all(&dir);
    }
}
