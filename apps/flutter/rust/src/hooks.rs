//! The platform hooks, in Rust for the prototype: the client calls them from
//! its own threads, where a call into Dart would have to wait for the UI
//! isolate. The native apps keep the credential in the keychain or keyring;
//! the prototype keeps it in a file of its own data directory (owner only on
//! macOS and Linux), apart from the native apps'. Enhanced mode is out of the
//! prototype's scope: no service is installed.

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;

use ppvpn_client::{PlatformError, PlatformHooks};

pub(crate) struct Hooks {
    credential: PathBuf,
}

impl Hooks {
    pub(crate) fn new(data_dir: &str) -> Self {
        Self {
            credential: PathBuf::from(data_dir).join("credential.json"),
        }
    }
}

fn failed(error: std::io::Error) -> PlatformError {
    PlatformError::Failed {
        message: error.to_string(),
    }
}

impl PlatformHooks for Hooks {
    fn credential_load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        match fs::read(&self.credential) {
            Ok(blob) => Ok(Some(blob)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(failed(error)),
        }
    }

    fn credential_save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
        let partial = self.credential.with_extension("partial");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial).map_err(failed)?;
        std::io::Write::write_all(&mut file, &blob).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        fs::rename(&partial, &self.credential).map_err(failed)
    }

    fn credential_delete(&self) -> Result<(), PlatformError> {
        match fs::remove_file(&self.credential) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }

    fn open_url(&self, url: String) -> bool {
        open_url(&url)
    }

    fn privileged_service_installed(&self) -> bool {
        false
    }

    fn install_privileged_service(&self) -> Result<(), PlatformError> {
        Err(PlatformError::Failed {
            message: "enhanced mode is not part of the Flutter prototype".into(),
        })
    }

    fn uninstall_privileged_service(&self) -> Result<(), PlatformError> {
        Ok(())
    }
}

pub(crate) fn open_url(url: &str) -> bool {
    let mut command = if cfg!(target_os = "macos") {
        Command::new("open")
    } else if cfg!(windows) {
        let mut command = Command::new("rundll32");
        command.arg("url.dll,FileProtocolHandler");
        command
    } else {
        Command::new("xdg-open")
    };
    command.arg(url).spawn().is_ok()
}
