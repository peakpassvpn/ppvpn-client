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
    store_for(SERVICE)
}

/// The platform store under another service name: the tests' own, so they
/// never read or replace a real login.
pub fn store_for(service: &str) -> Arc<dyn CredentialStore> {
    Arc::new(PlatformStore {
        service: service.to_string(),
    })
}

struct PlatformStore {
    service: String,
}

#[cfg(target_os = "macos")]
mod imp {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    use super::{StoreFailure, ACCOUNT};

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

    pub fn load(service: &str) -> Result<Option<Vec<u8>>, StoreFailure> {
        match get_generic_password(service, ACCOUNT) {
            Ok(data) => Ok(Some(data)),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(None),
            Err(err) => Err(failure("read", err)),
        }
    }

    pub fn save(service: &str, blob: &[u8]) -> Result<(), StoreFailure> {
        set_generic_password(service, ACCOUNT, blob).map_err(|e| failure("write", e))
    }

    pub fn delete(service: &str) -> Result<(), StoreFailure> {
        match delete_generic_password(service, ACCOUNT) {
            Ok(()) => Ok(()),
            Err(err) if err.code() == ITEM_NOT_FOUND => Ok(()),
            Err(err) => Err(failure("delete", err)),
        }
    }
}

#[cfg(target_os = "linux")]
mod imp {
    use std::collections::HashMap;
    use std::time::Duration;

    use secret_service::{EncryptionType, SecretService};

    use super::{StoreFailure, ACCOUNT};

    /// How long the keyring's unlock prompt may take. Without a prompter
    /// (a headless session) the request may never be answered.
    const UNLOCK_TIMEOUT: Duration = Duration::from_secs(60);

    fn unavailable(err: impl std::fmt::Display) -> StoreFailure {
        StoreFailure {
            locked: false,
            message: format!("no Secret Service is available to store the login (a desktop keyring such as GNOME Keyring or KWallet is required): {err}"),
        }
    }

    fn attributes(service: &str) -> HashMap<&str, &str> {
        HashMap::from([("service", service), ("username", ACCOUNT)])
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
            let _ = tokio::time::timeout(UNLOCK_TIMEOUT, collection.unlock()).await;
            if collection.is_locked().await.map_err(unavailable)? {
                return Err(StoreFailure {
                    locked: true,
                    message: "the keyring is locked; unlock it and try again".into(),
                });
            }
        }
        Ok(collection)
    }

    pub fn load(name: &str) -> Result<Option<Vec<u8>>, StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            let items = collection
                .search_items(attributes(name))
                .await
                .map_err(unavailable)?;
            match items.first() {
                Some(item) => item.get_secret().await.map(Some).map_err(unavailable),
                None => Ok(None),
            }
        })
    }

    pub fn save(name: &str, blob: &[u8]) -> Result<(), StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            collection
                .create_item(
                    "PPVPN CLI device login",
                    attributes(name),
                    blob,
                    true,
                    "application/json",
                )
                .await
                .map(|_| ())
                .map_err(unavailable)
        })
    }

    pub fn delete(name: &str) -> Result<(), StoreFailure> {
        block_on(async {
            let service = SecretService::connect(EncryptionType::Dh)
                .await
                .map_err(unavailable)?;
            let collection = collection(&service).await?;
            for item in collection
                .search_items(attributes(name))
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
        imp::load(&self.service)
    }

    fn save(&self, blob: Vec<u8>) -> Result<(), StoreFailure> {
        imp::save(&self.service, &blob)
    }

    fn delete(&self) -> Result<(), StoreFailure> {
        imp::delete(&self.service)
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
