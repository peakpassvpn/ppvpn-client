//! The real platform secret store: the macOS Keychain and the Linux Secret
//! Service. Opt-in with `PPVPN_TEST_KEYSTORE=1` (CI sets it, in a step with
//! its own timeout), so an ordinary `cargo test` never touches a
//! developer's keychain. The item lives under a service name of its own
//! and is removed again.

use std::time::{SystemTime, UNIX_EPOCH};

use ppvpn_cli::keystore::store_for;

/// Bytes unique to this run, shaped like the JSON the login saves.
fn blob(tag: &str) -> Vec<u8> {
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_nanos());
    format!(r#"{{"{tag}":"{}-{nanos}"}}"#, std::process::id()).into_bytes()
}

#[cfg(target_os = "linux")]
fn lock_the_default_collection() {
    use secret_service::{EncryptionType, SecretService};
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let service = SecretService::connect(EncryptionType::Dh).await.unwrap();
            let collection = service.get_default_collection().await.unwrap();
            collection.lock().await.unwrap();
            assert!(collection.is_locked().await.unwrap());
        });
}

#[test]
fn the_platform_store_saves_loads_and_deletes() {
    if std::env::var_os("PPVPN_TEST_KEYSTORE").is_none() {
        return;
    }
    let service = format!("com.peakpassvpn.ppvpn.cli.test-{}", std::process::id());
    let store = store_for(&service);
    // Whatever a failed assertion says, it must not be the stored bytes.
    let loaded = || store.load().expect("the store can be read");

    assert!(loaded().is_none(), "a fresh service name has no item");
    let first = blob("first");
    store.save(first.clone()).expect("the store can be written");
    assert!(loaded() == Some(first), "the saved item is read back");

    // Saving again replaces the item instead of adding a second one.
    let second = blob("second");
    store
        .save(second.clone())
        .expect("the item can be replaced");
    assert!(loaded() == Some(second), "the replaced item is read back");

    store.delete().expect("the item can be deleted");
    assert!(loaded().is_none(), "a deleted item is gone");
    store
        .delete()
        .expect("deleting a missing item is not an error");

    // A keyring that stays locked is reported as locked, not as empty and
    // not as a hang: nothing can answer the unlock prompt here.
    #[cfg(target_os = "linux")]
    {
        lock_the_default_collection();
        let failure = store.load().expect_err("a locked keyring cannot be read");
        assert!(failure.locked, "{}", failure.message);
    }
}
