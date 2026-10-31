//! The saved sign-in: the client's one opaque blob, kept where and as the
//! native apps keep it, so that either app finds the other's:
//! - macOS: the 0600 file `credentials` in the data directory, out of Time
//!   Machine (apps/macos FileCredentialStore; its Debug builds use
//!   `credentials.debug`, this app always the release name);
//! - Windows: one generic credential in the Credential Manager, target
//!   `PPVPN/desktop.credentials`, user `PPVPN`, persisted per machine
//!   (apps/windows CredentialStore);
//! - Linux: the Secret Service item libsecret stores for the native app
//!   (schema `com.peakpassvpn.ppvpn.desktop`), else the 0600 file
//!   `credentials` in the data directory when the session has no Secret
//!   Service (apps/linux CredentialStores);
//! - iOS: one generic password in the app's Keychain (#473), out of the
//!   App Group container the data directory is: the packet tunnel does not
//!   need the sign-in.
//! - Android: the 0600 file `credentials.sealed` in the app's own files,
//!   the blob sealed with an AES-GCM key that never leaves the Android
//!   Keystore (#473, runner/android Credentials.kt, over JNI: android.rs);
//!   the plain file `credentials` earlier builds wrote is moved over.
//!
//! With `PPVPN_FLUTTER_HOME` (the end-to-end tests) it is always the file,
//! in that home, so a test never touches a keyring.

use std::fs;
use std::io::ErrorKind;
use std::path::{Path, PathBuf};

use ppvpn_client::PlatformError;

/// The native apps' file name (macOS release builds, Linux).
const FILE: &str = "credentials";
/// Android's sealed blob, beside it.
#[cfg(target_os = "android")]
const SEALED_FILE: &str = "credentials.sealed";

fn failure(message: impl Into<String>) -> PlatformError {
    PlatformError::Failed {
        message: message.into(),
    }
}

fn failed(error: std::io::Error) -> PlatformError {
    failure(error.to_string())
}

pub(crate) enum Store {
    File(FileStore),
    #[cfg(windows)]
    CredentialManager,
    #[cfg(target_os = "linux")]
    Keyring(secret_service::Fallback),
    #[cfg(target_os = "ios")]
    Keychain,
    #[cfg(target_os = "android")]
    Keystore(keystore::Sealed),
}

impl Store {
    pub(crate) fn new(data_dir: &str) -> Self {
        let file = || FileStore(Path::new(data_dir).join(FILE));
        if crate::dirs::home_override().is_some() {
            Self::File(file())
        } else {
            Self::native(file)
        }
    }

    #[cfg(windows)]
    fn native(_file: impl FnOnce() -> FileStore) -> Self {
        Self::CredentialManager
    }

    #[cfg(target_os = "linux")]
    fn native(file: impl FnOnce() -> FileStore) -> Self {
        Self::Keyring(secret_service::Fallback::new(file()))
    }

    #[cfg(target_os = "ios")]
    fn native(_file: impl FnOnce() -> FileStore) -> Self {
        Self::Keychain
    }

    #[cfg(target_os = "android")]
    fn native(file: impl FnOnce() -> FileStore) -> Self {
        let plain = file();
        let sealed = FileStore(plain.0.with_file_name(SEALED_FILE));
        Self::Keystore(keystore::Sealed { sealed, plain })
    }

    #[cfg(not(any(windows, target_os = "linux", target_os = "ios", target_os = "android")))]
    fn native(file: impl FnOnce() -> FileStore) -> Self {
        Self::File(file())
    }

    pub(crate) fn load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        match self {
            Self::File(file) => file.load(),
            #[cfg(windows)]
            Self::CredentialManager => credential_manager::load(),
            #[cfg(target_os = "linux")]
            Self::Keyring(keyring) => keyring.load(),
            #[cfg(target_os = "ios")]
            Self::Keychain => keychain::load(),
            #[cfg(target_os = "android")]
            Self::Keystore(sealed) => sealed.load(),
        }
    }

    pub(crate) fn save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
        match self {
            Self::File(file) => file.save(&blob),
            #[cfg(windows)]
            Self::CredentialManager => credential_manager::save(blob),
            #[cfg(target_os = "linux")]
            Self::Keyring(keyring) => keyring.save(blob),
            #[cfg(target_os = "ios")]
            Self::Keychain => keychain::save(&blob),
            #[cfg(target_os = "android")]
            Self::Keystore(sealed) => sealed.save(&blob),
        }
    }

    pub(crate) fn delete(&self) -> Result<(), PlatformError> {
        match self {
            Self::File(file) => file.delete(),
            #[cfg(windows)]
            Self::CredentialManager => credential_manager::delete(),
            #[cfg(target_os = "linux")]
            Self::Keyring(keyring) => keyring.delete(),
            #[cfg(target_os = "ios")]
            Self::Keychain => keychain::delete(),
            #[cfg(target_os = "android")]
            Self::Keystore(sealed) => sealed.delete(),
        }
    }
}

/// The blob as a file, readable only by the owner on macOS and Linux.
pub(crate) struct FileStore(PathBuf);

impl FileStore {
    fn load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
        match fs::read(&self.0) {
            Ok(blob) => Ok(Some(blob)),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(None),
            Err(error) => Err(failed(error)),
        }
    }

    /// Written to a 0600 temporary file and renamed over the old one, so the
    /// blob is never readable by others, not even briefly.
    fn save(&self, blob: &[u8]) -> Result<(), PlatformError> {
        if let Some(directory) = self.0.parent() {
            fs::create_dir_all(directory).map_err(failed)?;
        }
        let partial = self.0.with_extension("new");
        let mut options = fs::OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        std::os::unix::fs::OpenOptionsExt::mode(&mut options, 0o600);
        let mut file = options.open(&partial).map_err(failed)?;
        std::io::Write::write_all(&mut file, blob).map_err(failed)?;
        file.sync_all().map_err(failed)?;
        fs::rename(&partial, &self.0).map_err(failed)?;
        // Keep the sign-in out of Time Machine backups.
        #[cfg(target_os = "macos")]
        crate::cf::exclude_from_backup(&self.0);
        Ok(())
    }

    fn delete(&self) -> Result<(), PlatformError> {
        match fs::remove_file(&self.0) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
            Err(error) => Err(failed(error)),
        }
    }
}

/// Windows: the native app's generic credential (per user, protected by
/// Windows with DPAPI).
#[cfg(windows)]
mod credential_manager {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    use ppvpn_client::PlatformError;
    use windows_sys::Win32::Foundation::{GetLastError, ERROR_NOT_FOUND, WIN32_ERROR};
    use windows_sys::Win32::Security::Credentials::{
        CredDeleteW, CredFree, CredReadW, CredWriteW, CREDENTIALW, CRED_MAX_CREDENTIAL_BLOB_SIZE,
        CRED_PERSIST_LOCAL_MACHINE, CRED_TYPE_GENERIC,
    };

    use super::failure;

    /// CredentialStore.Target in apps/windows (the fake backend's
    /// `.fake` target is the native app's screenshot mode, not used here).
    const TARGET: &str = "PPVPN/desktop.credentials";
    const USER_NAME: &str = "PPVPN";

    fn wide(text: &str) -> Vec<u16> {
        OsStr::new(text).encode_wide().chain(Some(0)).collect()
    }

    fn os_failure(code: WIN32_ERROR) -> PlatformError {
        failure(std::io::Error::from_raw_os_error(code as i32).to_string())
    }

    pub(super) fn load() -> Result<Option<Vec<u8>>, PlatformError> {
        let target = wide(TARGET);
        let mut credential: *mut CREDENTIALW = std::ptr::null_mut();
        // SAFETY: a NUL-terminated target and an out pointer for the result.
        if unsafe { CredReadW(target.as_ptr(), CRED_TYPE_GENERIC, 0, &mut credential) } == 0 {
            // SAFETY: no other call in between.
            let error = unsafe { GetLastError() };
            return match error {
                ERROR_NOT_FOUND => Ok(None),
                error => Err(os_failure(error)),
            };
        }
        // SAFETY: CredReadW succeeded, so `credential` is its result (freed
        // once here) and the blob is CredentialBlobSize bytes.
        let blob = unsafe {
            let read = &*credential;
            let blob = if read.CredentialBlob.is_null() || read.CredentialBlobSize == 0 {
                Vec::new()
            } else {
                std::slice::from_raw_parts(read.CredentialBlob, read.CredentialBlobSize as usize)
                    .to_vec()
            };
            CredFree(credential.cast_const().cast());
            blob
        };
        Ok(Some(blob))
    }

    pub(super) fn save(mut blob: Vec<u8>) -> Result<(), PlatformError> {
        if blob.len() > CRED_MAX_CREDENTIAL_BLOB_SIZE as usize {
            return Err(failure(format!(
                "credential blob is {} bytes; the limit is {CRED_MAX_CREDENTIAL_BLOB_SIZE}",
                blob.len()
            )));
        }
        let (mut target, mut user) = (wide(TARGET), wide(USER_NAME));
        let credential = CREDENTIALW {
            Type: CRED_TYPE_GENERIC,
            TargetName: target.as_mut_ptr(),
            CredentialBlobSize: blob.len() as u32,
            CredentialBlob: blob.as_mut_ptr(),
            Persist: CRED_PERSIST_LOCAL_MACHINE,
            UserName: user.as_mut_ptr(),
            ..Default::default()
        };
        // SAFETY: `credential` and the buffers it points to outlive the call.
        let written = unsafe { CredWriteW(&credential, 0) } != 0;
        // SAFETY: no other call in between.
        let error = (!written).then(|| unsafe { GetLastError() });
        // Wipe the plaintext copy before releasing it.
        blob.fill(0);
        match error {
            None => Ok(()),
            Some(error) => Err(os_failure(error)),
        }
    }

    /// Deleting a missing credential is not an error.
    pub(super) fn delete() -> Result<(), PlatformError> {
        let target = wide(TARGET);
        // SAFETY: a NUL-terminated target.
        if unsafe { CredDeleteW(target.as_ptr(), CRED_TYPE_GENERIC, 0) } != 0 {
            return Ok(());
        }
        // SAFETY: no other call in between.
        match unsafe { GetLastError() } {
            ERROR_NOT_FOUND => Ok(()),
            error => Err(os_failure(error)),
        }
    }
}

/// iOS: a generic password of the app alone (no access group: the app's
/// own), readable after the first unlock since the device started, never
/// restored to another device (#473).
#[cfg(target_os = "ios")]
mod keychain {
    use ppvpn_client::PlatformError;
    use security_framework::access_control::{ProtectionMode, SecAccessControl};
    use security_framework::base::Error;
    use security_framework::passwords::{
        delete_generic_password_options, generic_password, set_generic_password_options,
        PasswordOptions,
    };

    use super::failure;

    const SERVICE: &str = "com.peakpassvpn.ppvpn";
    const ACCOUNT: &str = "credentials";
    /// errSecItemNotFound.
    const NOT_FOUND: i32 = -25300;

    fn options() -> PasswordOptions {
        PasswordOptions::new_generic_password(SERVICE, ACCOUNT)
    }

    fn keychain_failure(error: Error) -> PlatformError {
        failure(format!("keychain: {error} ({})", error.code()))
    }

    pub(super) fn load() -> Result<Option<Vec<u8>>, PlatformError> {
        match generic_password(options()) {
            Ok(blob) => Ok(Some(blob)),
            Err(error) if error.code() == NOT_FOUND => Ok(None),
            Err(error) => Err(keychain_failure(error)),
        }
    }

    /// A new item each time: an update would keep the old item's
    /// accessibility, and a query with access control matches nothing.
    pub(super) fn save(blob: &[u8]) -> Result<(), PlatformError> {
        delete()?;
        let access = SecAccessControl::create_with_protection(
            Some(ProtectionMode::AccessibleAfterFirstUnlockThisDeviceOnly),
            0,
        )
        .map_err(keychain_failure)?;
        let mut options = options();
        options.set_access_control(access);
        set_generic_password_options(blob, options).map_err(keychain_failure)
    }

    /// Deleting a missing item is not an error.
    pub(super) fn delete() -> Result<(), PlatformError> {
        match delete_generic_password_options(options()) {
            Err(error) if error.code() != NOT_FOUND => Err(keychain_failure(error)),
            _ => Ok(()),
        }
    }
}

/// Android: the blob sealed by the app's Keystore key (android.rs), in a
/// file only the app reads. A sealed blob whose key is gone (the app's data
/// restored elsewhere, the key invalidated) is no sign-in: it is dropped and
/// the user signs in again.
#[cfg(target_os = "android")]
mod keystore {
    use ppvpn_client::PlatformError;

    use super::FileStore;

    pub(crate) struct Sealed {
        pub(super) sealed: FileStore,
        /// The plain file of earlier builds, moved over on the first load.
        pub(super) plain: FileStore,
    }

    impl Sealed {
        pub(super) fn load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
            if let Some(sealed) = self.sealed.load()? {
                if let Some(blob) = crate::android::open(&sealed)? {
                    return Ok(Some(blob));
                }
                tracing::warn!("the saved sign-in's key is gone: dropped");
                self.sealed.delete()?;
                return Ok(None);
            }
            let Some(blob) = self.plain.load()? else {
                return Ok(None);
            };
            self.save(&blob)?;
            Ok(Some(blob))
        }

        /// The plain file goes once the sealed one is written.
        pub(super) fn save(&self, blob: &[u8]) -> Result<(), PlatformError> {
            self.sealed.save(&crate::android::seal(blob)?)?;
            self.plain.delete()
        }

        pub(super) fn delete(&self) -> Result<(), PlatformError> {
            self.sealed.delete()?;
            self.plain.delete()
        }
    }
}

/// Linux: the Secret Service (GNOME Keyring, KWallet, KeePassXC …) over
/// D-Bus, with the item the native app stores through libsecret, and its
/// fallback to the file (apps/linux FallbackCredentialStore and
/// SecretServiceCredentialStore).
#[cfg(target_os = "linux")]
mod secret_service {
    use std::collections::HashMap;
    use std::future::Future;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::time::Duration;

    use ppvpn_client::PlatformError;
    use zbus::export::futures_util::StreamExt;
    use zbus::message::Type as MessageType;
    use zbus::zvariant::{DynamicType, OwnedObjectPath, OwnedValue, Value};
    use zbus::{Connection, MatchRule, Message, MessageStream};

    use super::{failure, FileStore};

    const SERVICE: &str = "org.freedesktop.secrets";
    const SERVICE_PATH: &str = "/org/freedesktop/secrets";
    const SERVICE_INTERFACE: &str = "org.freedesktop.Secret.Service";
    const ITEM_INTERFACE: &str = "org.freedesktop.Secret.Item";
    const PROMPT_INTERFACE: &str = "org.freedesktop.Secret.Prompt";
    const COLLECTION_INTERFACE: &str = "org.freedesktop.Secret.Collection";
    /// Where libsecret stores an item when no collection is given.
    const DEFAULT_COLLECTION: &str = "/org/freedesktop/secrets/aliases/default";
    /// The native app's libsecret schema. libsecret adds the schema's name
    /// as `xdg:schema` to the attributes it stores and searches with.
    const SCHEMA: &str = "com.peakpassvpn.ppvpn.desktop";
    const ACCOUNT: &str = "credentials";
    const LABEL: &str = "PPVPN";
    const CONTENT_TYPE: &str = "application/octet-stream";
    /// GDBus's default call timeout, which libsecret's calls have. Prompts
    /// wait for the user instead.
    const CALL_TIMEOUT: Duration = Duration::from_secs(25);

    /// What went wrong, as the native store tells the cases apart.
    enum Failure {
        /// No Secret Service in this session: use the file.
        Unavailable(String),
        /// The keyring holding the saved login is locked.
        Locked,
        /// No such object: the default collection does not exist yet.
        Missing(String),
        Failed(String),
    }

    impl From<zbus::Error> for Failure {
        fn from(error: zbus::Error) -> Self {
            match error_name(&error) {
                Some(
                    "org.freedesktop.DBus.Error.ServiceUnknown"
                    | "org.freedesktop.DBus.Error.NameHasNoOwner",
                ) => Self::Unavailable(error.to_string()),
                Some("org.freedesktop.Secret.Error.IsLocked") => Self::Locked,
                _ if missing_object(&error) => Self::Missing(error.to_string()),
                _ => Self::Failed(error.to_string()),
            }
        }
    }

    impl From<Failure> for PlatformError {
        fn from(error: Failure) -> Self {
            match error {
                Failure::Unavailable(message) => {
                    failure(format!("LINUX_SECRET_SERVICE_UNAVAILABLE:{message}"))
                }
                Failure::Locked => PlatformError::Locked {
                    message: "LINUX_KEYRING_LOCKED: the keyring holding the saved login is locked"
                        .into(),
                },
                Failure::Missing(message) | Failure::Failed(message) => {
                    failure(format!("LINUX_SECRET_SERVICE_FAILED:{message}"))
                }
            }
        }
    }

    fn error_name(error: &zbus::Error) -> Option<&str> {
        match error {
            zbus::Error::MethodError(name, _, _) => Some(name.as_str()),
            _ => None,
        }
    }

    fn missing_object(error: &zbus::Error) -> bool {
        matches!(
            error_name(error),
            Some(
                "org.freedesktop.Secret.Error.NoSuchObject"
                    | "org.freedesktop.DBus.Error.UnknownObject"
                    | "org.freedesktop.DBus.Error.UnknownMethod"
            )
        )
    }

    /// Secret Service first; the 0600 file only when the session has no
    /// Secret Service. Any other keyring failure, such as a dismissed unlock
    /// prompt, is reported instead of silently writing the sign-in to disk.
    pub(crate) struct Fallback {
        file: FileStore,
        /// Set once no Secret Service answered: the file from then on.
        unavailable: AtomicBool,
        /// Set once the unlock prompt was dismissed: later loads (the client
        /// retries in the background) report the lock without prompting
        /// again, until the keyring is unlocked.
        unlock_dismissed: AtomicBool,
    }

    impl Fallback {
        pub(super) fn new(file: FileStore) -> Self {
            Self {
                file,
                unavailable: AtomicBool::new(false),
                unlock_dismissed: AtomicBool::new(false),
            }
        }

        /// `outcome` of a keyring call, or None to go on with the file
        /// (no Secret Service).
        fn keyring<T>(&self, outcome: Result<T, Failure>) -> Result<Option<T>, PlatformError> {
            match outcome {
                Ok(value) => Ok(Some(value)),
                Err(Failure::Unavailable(message)) => {
                    tracing::info!("no Secret Service ({message}); the sign-in is kept in a file");
                    self.unavailable.store(true, Ordering::Relaxed);
                    Ok(None)
                }
                Err(error) => Err(error.into()),
            }
        }

        fn keyring_available(&self) -> bool {
            !self.unavailable.load(Ordering::Relaxed)
        }

        pub(super) fn load(&self) -> Result<Option<Vec<u8>>, PlatformError> {
            if self.keyring_available() {
                let dismissed = &self.unlock_dismissed;
                if let Some(Some(blob)) = self.keyring(on_own_thread(move || load(dismissed)))? {
                    return Ok(Some(blob));
                }
            }
            self.file.load()
        }

        pub(super) fn save(&self, blob: Vec<u8>) -> Result<(), PlatformError> {
            if self.keyring_available() {
                let saved = self.keyring(on_own_thread(|| save(&blob)))?;
                if saved.is_some() {
                    return self.file.delete();
                }
            }
            self.file.save(&blob)
        }

        pub(super) fn delete(&self) -> Result<(), PlatformError> {
            if self.keyring_available() {
                self.keyring(on_own_thread(delete))?;
            }
            self.file.delete()
        }
    }

    /// Runs a keyring call to the end on a thread and runtime of its own:
    /// the client calls the hooks from its own threads, which may be in a
    /// runtime already.
    fn on_own_thread<T, F>(call: impl FnOnce() -> F + Send) -> Result<T, Failure>
    where
        T: Send,
        F: Future<Output = Result<T, Failure>>,
    {
        std::thread::scope(|scope| {
            scope
                .spawn(move || {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(|error| Failure::Failed(error.to_string()))?
                        .block_on(call())
                })
                .join()
                .unwrap_or_else(|_| Err(Failure::Failed("the keyring call panicked".into())))
        })
    }

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([
            ("service", SCHEMA),
            ("account", ACCOUNT),
            ("xdg:schema", SCHEMA),
        ])
    }

    /// A connection to the session's Secret Service with a `plain` session
    /// (the secret travels unencrypted on the user's own session bus).
    struct Secrets {
        connection: Connection,
        session: OwnedObjectPath,
    }

    impl Secrets {
        async fn open() -> Result<Self, Failure> {
            // No session bus to talk to.
            let connection = Connection::session()
                .await
                .map_err(|error| Failure::Unavailable(error.to_string()))?;
            let reply = call(
                &connection,
                SERVICE_PATH,
                SERVICE_INTERFACE,
                "OpenSession",
                &("plain", Value::from("")),
            )
            .await?;
            let (_, session): (OwnedValue, OwnedObjectPath) = reply.body().deserialize()?;
            Ok(Self {
                connection,
                session,
            })
        }

        async fn call<B>(
            &self,
            path: &str,
            interface: &str,
            method: &str,
            body: &B,
        ) -> zbus::Result<Message>
        where
            B: serde::Serialize + DynamicType,
        {
            call(&self.connection, path, interface, method, body).await
        }

        /// The matching items: (unlocked, locked).
        async fn search(&self) -> Result<(Vec<OwnedObjectPath>, Vec<OwnedObjectPath>), Failure> {
            let reply = self
                .call(
                    SERVICE_PATH,
                    SERVICE_INTERFACE,
                    "SearchItems",
                    &(attributes(),),
                )
                .await?;
            Ok(reply.body().deserialize()?)
        }

        async fn secret(&self, item: &OwnedObjectPath) -> Result<Vec<u8>, Failure> {
            let reply = self
                .call(
                    item.as_str(),
                    ITEM_INTERFACE,
                    "GetSecret",
                    &(&self.session,),
                )
                .await?;
            let (_, _, value, _): (OwnedObjectPath, Vec<u8>, Vec<u8>, String) =
                reply.body().deserialize()?;
            Ok(value)
        }

        /// Unlocks `objects`, prompting when the service asks to; false when
        /// the prompt was dismissed.
        async fn unlock(&self, objects: Vec<OwnedObjectPath>) -> Result<bool, Failure> {
            let reply = self
                .call(SERVICE_PATH, SERVICE_INTERFACE, "Unlock", &(objects,))
                .await?;
            let (_, prompt): (Vec<OwnedObjectPath>, OwnedObjectPath) =
                reply.body().deserialize()?;
            self.prompt(&prompt).await
        }

        /// Runs `prompt` ("/" is none) and waits for it; false when it was
        /// dismissed.
        async fn prompt(&self, prompt: &OwnedObjectPath) -> Result<bool, Failure> {
            if prompt.as_str() == "/" {
                return Ok(true);
            }
            Ok(self.prompt_result(prompt).await?.is_some())
        }

        /// Runs `prompt` (not "/") and waits for it: its result, or None when
        /// it was dismissed.
        async fn prompt_result(
            &self,
            prompt: &OwnedObjectPath,
        ) -> Result<Option<OwnedValue>, Failure> {
            let rule = MatchRule::builder()
                .msg_type(MessageType::Signal)
                .path(prompt.as_str())?
                .interface(PROMPT_INTERFACE)?
                .member("Completed")?
                .build();
            let mut completed = MessageStream::for_match_rule(rule, &self.connection, None).await?;
            // No parent window to attach the prompt to.
            self.call(prompt.as_str(), PROMPT_INTERFACE, "Prompt", &("",))
                .await?;
            let signal = completed
                .next()
                .await
                .ok_or_else(|| Failure::Failed("the prompt ended without an answer".into()))??;
            let (dismissed, result): (bool, OwnedValue) = signal.body().deserialize()?;
            Ok((!dismissed).then_some(result))
        }

        /// CreateItem in `collection`, replacing the matching item.
        async fn create_item(&self, collection: &str, blob: &[u8]) -> zbus::Result<Message> {
            let properties = HashMap::from([
                ("org.freedesktop.Secret.Item.Label", Value::from(LABEL)),
                (
                    "org.freedesktop.Secret.Item.Attributes",
                    Value::from(attributes()),
                ),
            ]);
            let secret = (&self.session, Vec::<u8>::new(), blob, CONTENT_TYPE);
            self.call(
                collection,
                COLLECTION_INTERFACE,
                "CreateItem",
                &(properties, secret, true),
            )
            .await
        }
    }

    async fn call<B>(
        connection: &Connection,
        path: &str,
        interface: &str,
        method: &str,
        body: &B,
    ) -> zbus::Result<Message>
    where
        B: serde::Serialize + DynamicType,
    {
        let reply = connection.call_method(Some(SERVICE), path, Some(interface), method, body);
        tokio::time::timeout(CALL_TIMEOUT, reply)
            .await
            .unwrap_or_else(|_| {
                Err(zbus::Error::Failure(format!(
                    "{interface}.{method} timed out"
                )))
            })
    }

    async fn load(dismissed: &AtomicBool) -> Result<Option<Vec<u8>>, Failure> {
        let secrets = Secrets::open().await?;
        let (unlocked, locked) = secrets.search().await?;
        if let Some(item) = unlocked.first() {
            dismissed.store(false, Ordering::Relaxed);
            return secrets.secret(item).await.map(Some);
        }
        // Nothing unlocked: nothing is saved, or the saved login sits in a
        // locked keyring. Only the first means signed out; the second is an
        // unavailable store, so the client keeps the login and retries.
        let Some(item) = locked.first().cloned() else {
            return Ok(None);
        };
        if dismissed.load(Ordering::Relaxed) || !secrets.unlock(locked).await? {
            dismissed.store(true, Ordering::Relaxed);
            return Err(Failure::Locked);
        }
        dismissed.store(false, Ordering::Relaxed);
        secrets.secret(&item).await.map(Some)
    }

    /// Stores the blob in the default collection, as libsecret does without
    /// a collection: unlocked first, and created when there is none yet.
    async fn save(blob: &[u8]) -> Result<(), Failure> {
        let secrets = Secrets::open().await?;
        let default = OwnedObjectPath::try_from(DEFAULT_COLLECTION).map_err(zbus::Error::from)?;
        match secrets.unlock(vec![default]).await {
            Ok(true) | Err(Failure::Missing(_)) => {}
            Ok(false) => return Err(Failure::Locked),
            Err(error) => return Err(error),
        }
        let reply = match secrets.create_item(DEFAULT_COLLECTION, blob).await {
            Err(error) if missing_object(&error) => {
                let collection = create_default_collection(&secrets).await?;
                secrets.create_item(collection.as_str(), blob).await?
            }
            reply => reply?,
        };
        let (_, prompt): (OwnedObjectPath, OwnedObjectPath) = reply.body().deserialize()?;
        if !secrets.prompt(&prompt).await? {
            return Err(Failure::Locked);
        }
        Ok(())
    }

    /// The default collection, created as libsecret does when there is none.
    async fn create_default_collection(secrets: &Secrets) -> Result<OwnedObjectPath, Failure> {
        let properties = HashMap::from([(
            "org.freedesktop.Secret.Collection.Label",
            Value::from("Default keyring"),
        )]);
        let reply = secrets
            .call(
                SERVICE_PATH,
                SERVICE_INTERFACE,
                "CreateCollection",
                &(properties, "default"),
            )
            .await?;
        let (collection, prompt): (OwnedObjectPath, OwnedObjectPath) =
            reply.body().deserialize()?;
        if prompt.as_str() == "/" {
            return Ok(collection);
        }
        // Created behind a prompt: its result is the collection's path.
        let created = secrets
            .prompt_result(&prompt)
            .await?
            .ok_or(Failure::Locked)?;
        OwnedObjectPath::try_from(created).map_err(|error| zbus::Error::from(error).into())
    }

    /// Deletes the matching unlocked items, as libsecret's clear does.
    async fn delete() -> Result<(), Failure> {
        let secrets = Secrets::open().await?;
        let (unlocked, _) = secrets.search().await?;
        for item in unlocked {
            let reply = secrets
                .call(item.as_str(), ITEM_INTERFACE, "Delete", &())
                .await?;
            let prompt: OwnedObjectPath = reply.body().deserialize()?;
            if !secrets.prompt(&prompt).await? {
                return Err(Failure::Locked);
            }
        }
        Ok(())
    }
}
