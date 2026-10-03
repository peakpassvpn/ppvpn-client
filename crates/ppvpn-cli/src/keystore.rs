//! The device credential's home: the platform secret store, and nothing
//! else. There is no plain-file fallback; where no secret store is
//! available (a server without a Secret Service, a locked keyring) the CLI
//! says so and refuses to log in.
//!
//! - macOS: a generic password in the login Keychain.
//! - Linux: an item in the Secret Service default collection, over an
//!   encrypted (Diffie-Hellman) session.

use std::sync::Arc;

use ppvpn_account::auth::{CredentialStore, StoreFailure};

/// The Keychain service / Secret Service `service` attribute. A CLI login
/// is its own device session, separate from the desktop app's.
pub const SERVICE: &str = "com.peakpassvpn.ppvpn.cli";
pub const ACCOUNT: &str = "device-credential";

pub fn platform_store() -> Arc<dyn CredentialStore> {
    Arc::new(PlatformStore)
}

struct PlatformStore;

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    use super::{StoreFailure, ACCOUNT, SERVICE};

    // Security.framework status codes.
    const ITEM_NOT_FOUND: i32 = -25300;
    const INTERACTION_NOT_ALLOWED: i32 = -25308;
    const USER_CANCELED: i32 = -128;

    fn failure(action: &str, err: security_framework::base::Error) -> StoreFailure {
        StoreFailure {
            locked: matches!(err.code(), INTERACTION_NOT_ALLOWED | USER_CANCELED),
            message: format!("Keychain {action} failed: {err}"),
        }
    }

    pub fn load() -> Result<Option<Vec<u8>>, StoreFailure> {
        match get_generic_password(SERVICE, ACCOUNT) {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(failure("read", err)),
        }
    }

    pub fn save(blob: &[u8]) -> Result<(), StoreFailure> {
        set_generic_password(SERVICE, ACCOUNT, blob).map_err(|e| failure("write", e))
    }

    pub fn delete() -> Result<(), StoreFailure> {
        match delete_generic_password(SERVICE, ACCOUNT) {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(()),
            Err(err) => Err(failure("delete", err)),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;

    use secret_service::{EncryptionType, SecretService};

    use super::{StoreFailure, ACCOUNT, SERVICE};

    fn unavailable(err: impl std::fmt::Display) -> StoreFailure {
        StoreFailure {
            locked: false,
            message: format!("no Secret Service is available to store the login (a desktop keyring such as GNOME Keyring or KWallet is required): {err}"),
        }
    }

    fn attributes() -> HashMap<&'static str, &'static str> {
        HashMap::from([("service", SERVICE), ("username", ACCOUNT)])
    }

    /// The trait is synchronous and may be called from inside a tokio
    /// runtime, so each call runs on its own thread with its own
    /// current-thread runtime.
    fn block_on<T: Send>(
        future: impl std::future::Future<Output = Result<T, StoreFailure>> + Send,
    ) -> Result<T, StoreFailure> {
        std::thread::scope(|scope| {
            scope
                .spawn(|| {
                    tokio::runtime::Builder::new_current_thread()
                        .enable_all()
                        .build()
                        .map_err(unavailable)?
                        .block_on(future)
                })
                .join()
                .unwrap_or_else(|_| Err(unavailable("the secret store call panicked")))
        })
    }

    async fn collection<'a>(
        service: &'a SecretService<'a>,
    ) -> Result<secret_service::Collection<'a>, StoreFailure> {
        let collection = service
            .get_default_collection()
            .await
            .map_err(unavailable)?;
        if collection.is_locked().await.map_err(unavailable)? {
            // Asks the keyring to prompt; a headless session cannot.
            let _ = collection.unlock().await;
            if collection.is_locked().await.map_err(unavailable)? {
                return Err(StoreFailure {
                    locked: true,
                    message: "the keyring is locked; unlock it and try again".into(),
                });
            }
        }
        Ok(collection)
    }

    pub fn load() -> Result<Option<Vec<u8>>, StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            let items = collection
                .search_items(attributes())
                .await
                .map_err(unavailable)?;
            match items.first() {
                Some(item) => item.get_secret().await.map(Some).map_err(unavailable),
                None => Ok(None),
            }
        })
    }

    pub fn save(blob: &[u8]) -> Result<(), StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            collection
                .create_item(
                    "PPVPN CLI device login",
                    attributes(),
                    blob,
                    true,
                    "application/json",
                )
                .await
                .map(|_| ())
                .map_err(unavailable)
        })
    }

    pub fn delete() -> Result<(), StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            for item in collection
                .search_items(attributes())
                .await
                .map_err(unavailable)?
            {
                item.delete().await.map_err(unavailable)?;
            }
            Ok(())
        })
    }
}

impl CredentialStore for PlatformStore {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreFailure> {
        imp::load()
    }

    fn save(&self, blob: Vec<u8>) -> Result<(), StoreFailure> {
        imp::save(&blob)
    }

    fn delete(&self) -> Result<(), StoreFailure> {
        imp::delete()
    }
}

/// An in-memory store for tests and embedding.
#[derive(Default)]
pub struct MemoryStore(std::sync::Mutex<Option<Vec<u8>>>);

impl MemoryStore {
    pub fn is_empty(&self) -> bool {
        self.0.lock().expect("memory store lock").is_none()
    }
}

impl CredentialStore for MemoryStore {
    fn load(&self) -> Result<Option<Vec<u8>>, StoreFailure> {
        Ok(self.0.lock().expect("memory store lock").clone())
    }

    fn save(&self, blob: Vec<u8>) -> Result<(), StoreFailure> {
        *self.0.lock().expect("memory store lock") = Some(blob);
        Ok(())
    }

    fn delete(&self) -> Result<(), StoreFailure> {
        *self.0.lock().expect("memory store lock") = None;
        Ok(())
    }
}
