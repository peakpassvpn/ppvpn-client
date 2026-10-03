use std::net::TcpListener;
use std::path::PathBuf;
use std::time::Duration;

use serde_json::{json, Value};

use super::*;

const NODE_1: &str = "3f2c9a1e-0000-4000-8000-000000000001-128";
const NODE_2: &str = "3f2c9a1e-0000-4000-8000-000000000002-129";

/// The contract golden's profile: two nodes.
fn profile() -> Profile {
    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../testdata/golden/contract/profiles/base.json");
    let data = fs::read(&path).unwrap_or_else(|e| panic!("{}: {e}", path.display()));
    crate::profile::parse(&data).unwrap()
}

/// No preferred port: tests must not race for 7890.
fn any_port() -> LocalProxyConfig {
    LocalProxyConfig::new().with_preferred_port(0)
}

/// A loopback port that was free a moment ago.
fn free_port() -> u16 {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port()
}

/// Binds 127.0.0.1:port for the test. Another test may probe the same
/// well-known port for an instant, so retry briefly; if it stays taken,
/// another process owns it and it is busy either way.
fn hold_port(port: u16) -> Option<TcpListener> {
    for _ in 0..20 {
        if let Ok(listener) = TcpListener::bind(("127.0.0.1", port)) {
            return Some(listener);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    None
}

fn read_state(dir: &Path) -> DiskState {
    serde_json::from_slice(&fs::read(dir.join(STATE_FILE)).unwrap()).unwrap()
}

fn write_state(dir: &Path, content: &str) {
    write_private(&dir.join(STATE_FILE), content.as_bytes()).unwrap();
}

/// A fresh prefix and password: tests never hard-code a secret.
fn secret() -> (String, String) {
    (random_prefix().unwrap(), random_password().unwrap())
}

/// Credentials compared without printing them on failure.
fn same(a: &LocalProxyCredential, b: &LocalProxyCredential) -> bool {
    a == b
}

#[cfg(unix)]
fn mode(path: &Path) -> u32 {
    use std::os::unix::fs::PermissionsExt;
    fs::metadata(path).unwrap().permissions().mode() & 0o777
}

/// Go: TestSharedEndpointsAreStableAndPrivate.
#[test]
fn shared_endpoints_are_stable_and_private() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("state");
    let profile = profile();
    let state = LocalProxyState::open(&dir, &any_port()).unwrap();
    assert_eq!(state.credentials_reset(), None, "first credentials");
    let first = [
        state.credential(Some(&profile), NODE_1).unwrap(),
        state.credential(Some(&profile), NODE_2).unwrap(),
    ];
    let (prefix, node_id) = parse_username(&first[0].username).expect("username parses");
    assert!(node_id == NODE_1, "username does not name its node");
    assert!(prefix.len() == 5, "prefix length");
    for (credential, id) in first.iter().zip([NODE_1, NODE_2]) {
        assert_eq!(credential.kind, LocalProxyKind::Node);
        assert_eq!(credential.node_id, id);
        assert_eq!(credential.listen, "127.0.0.1");
        assert_ne!(credential.port, 0);
        assert_eq!(credential.port, first[0].port);
        assert!(
            credential.password == first[0].password,
            "password not shared"
        );
        assert!(credential.password.len() == 43, "password length");
        assert!(
            credential.username == format_username(prefix, id),
            "username format"
        );
    }
    let translated = state.translate_options();
    assert_eq!(translated.port, first[0].port);
    assert!(translated.prefix == prefix, "translated prefix");
    assert!(
        translated.password == first[0].password,
        "translated password"
    );
    assert!(
        translated.username(NODE_1) == first[0].username,
        "translated username"
    );
    drop(state);

    // A restart keeps prefix, password and port.
    let again = LocalProxyState::open(&dir, &any_port()).unwrap();
    assert_eq!(again.credentials_reset(), None, "kept credentials");
    assert!(
        same(
            &again.credential(Some(&profile), NODE_1).unwrap(),
            &first[0]
        ),
        "credential changed on restart"
    );
    assert!(
        same(
            &again.credential(Some(&profile), NODE_2).unwrap(),
            &first[1]
        ),
        "credential changed on restart"
    );
    let disk = read_state(&dir);
    assert_eq!(disk.version, STATE_VERSION);
    assert!(disk.prefix == prefix, "persisted prefix");
    assert!(disk.password == first[0].password, "persisted password");
    assert_eq!(disk.port, first[0].port);
    // Only the device state: no profile, no node.
    let raw = fs::read_to_string(dir.join(STATE_FILE)).unwrap();
    assert!(!raw.contains(NODE_1), "state file names a node");
    #[cfg(unix)]
    {
        assert_eq!(mode(&dir.join(STATE_FILE)), 0o600);
        assert_eq!(mode(&dir), 0o700);
    }
}

/// Go: TestPrefixesAreRandomLowercaseAlphanumerics.
#[test]
fn prefixes_are_random_lowercase_alphanumerics() {
    let mut seen = std::collections::HashSet::new();
    for _ in 0..32 {
        let prefix = random_prefix().unwrap();
        assert!(valid_prefix(&prefix), "prefix outside the alphabet");
        seen.insert(prefix);
    }
    assert!(
        seen.len() >= 30,
        "prefixes repeat too often: {}",
        seen.len()
    );
    let password = random_password().unwrap();
    assert!(password.len() == 43, "password length");
    assert!(
        password
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "password is not base64url"
    );
    assert!(password != random_password().unwrap(), "password repeats");
}

/// Go: TestParseUsername.
#[test]
fn parse_username_splits_prefix_and_node() {
    for (case, (username, want)) in [
        ("u8f2k-hk-001", Some(("u8f2k", "hk-001"))),
        (
            "u8f2k-3f2c9a1e-0000-4000-8000-000000000001-128",
            Some(("u8f2k", "3f2c9a1e-0000-4000-8000-000000000001-128")),
        ),
        ("u8f2k--leading", Some(("u8f2k", "-leading"))),
        ("u8f2k-a.b_c", Some(("u8f2k", "a.b_c"))),
        ("u8f2k-", None),
        ("u8f2k", Some(("u8f2k", ""))), // the routed user
        ("U8F2K", None),
        ("u8f2", None),
        ("U8F2K-node", None),
        ("u8f2-node", None),
        ("u8f2kk-node", None),
        ("-node", None),
        ("", None),
    ]
    .into_iter()
    .enumerate()
    {
        assert!(parse_username(username) == want, "case {case}");
        if let Some((prefix, node_id)) = want {
            assert!(
                format_username(prefix, node_id) == username,
                "case {case}: round trip"
            );
        }
    }
}

/// Go: TestStartupPrefers7890AndFallsBackWhenBusy.
#[test]
fn startup_prefers_7890_and_falls_back_when_busy() {
    let _held = hold_port(PREFERRED_PORT);
    let tmp = tempfile::tempdir().unwrap();
    let first = LocalProxyState::open(tmp.path(), &LocalProxyConfig::new()).unwrap();
    assert_ne!(first.port(), PREFERRED_PORT);
    assert_ne!(first.port(), 0);
    assert_eq!(read_state(tmp.path()).port, first.port(), "not persisted");
    let port = first.port();
    drop(first);
    // The persisted fallback port is tried first next time.
    let second = LocalProxyState::open(tmp.path(), &LocalProxyConfig::new()).unwrap();
    assert_eq!(second.port(), port);
}

/// Go: TestStartupUsesPreferredPortWhenFree (a port known to be free
/// stands in for 7890, which a developer machine may use).
#[test]
fn startup_uses_preferred_port_when_free() {
    let tmp = tempfile::tempdir().unwrap();
    let preferred = free_port();
    let config = LocalProxyConfig::new().with_preferred_port(preferred);
    let state = LocalProxyState::open(tmp.path(), &config).unwrap();
    assert_eq!(state.port(), preferred);
    assert_eq!(state.status(false).port, preferred);
}

/// Go: TestStartupReplacesOccupiedPersistedPortOnly. A running listener
/// holds the port and nothing moves it; `reconcile_port` before the next
/// start replaces it, keeps the credentials and returns the event.
#[test]
fn startup_replaces_occupied_persisted_port_only() {
    let tmp = tempfile::tempdir().unwrap();
    let mut state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    let first = state.routed_credential();
    // Free: kept, no event.
    assert_eq!(state.reconcile_port().unwrap(), None);
    let listener = TcpListener::bind(("127.0.0.1", first.port)).unwrap();
    // While it runs, the credential stays as it is.
    assert!(
        same(&state.routed_credential(), &first),
        "credential changed while running"
    );
    let event = state.reconcile_port().unwrap().expect("port changed");
    let moved = state.routed_credential();
    assert_ne!(moved.port, first.port);
    assert!(moved.username == first.username, "username changed");
    assert!(moved.password == first.password, "password changed");
    match event {
        Event::LocalProxyEndpointChanged { listen, port, .. } => {
            assert_eq!(listen, "127.0.0.1");
            assert_eq!(port, moved.port);
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(read_state(tmp.path()).port, moved.port);
    drop(listener);
}

/// Go: TestMigratesVersion1StateInPlace.
#[test]
fn migrates_version_1_state_in_place() {
    let tmp = tempfile::tempdir().unwrap();
    let (old_user, old_password) = secret();
    write_state(
        tmp.path(),
        &serde_json::json!({
            "version": 1,
            "endpoints": { "hk-001": {
                "node_id": "hk-001", "listen": "127.0.0.1", "port": free_port(),
                "username": old_user, "password": old_password,
            }},
        })
        .to_string(),
    );
    let state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    assert_eq!(state.credentials_reset(), None, "an upgrade is not a reset");
    let got = state.routed_credential();
    assert!(got.username != old_user, "legacy username survived");
    assert!(got.password != old_password, "legacy password survived");
    assert_ne!(got.port, 0);
    let raw: Value =
        serde_json::from_slice(&fs::read(tmp.path().join(STATE_FILE)).unwrap()).unwrap();
    assert!(raw["version"] == STATE_VERSION, "version not upgraded");
    assert!(raw.get("endpoints").is_none(), "endpoints kept");
    let disk = read_state(tmp.path());
    assert!(valid_prefix(&disk.prefix), "invalid prefix");
    assert!(disk.password == got.password, "persisted password");
    assert_eq!(disk.port, got.port);
    #[cfg(unix)]
    assert_eq!(mode(&tmp.path().join(STATE_FILE)), 0o600);
    drop(state);
    // The upgrade happens once: the next open keeps the generated values.
    let again = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    assert!(
        same(&again.routed_credential(), &got),
        "upgraded state not stable"
    );
}

/// Go: TestRejectsUnsupportedOrCorruptState. Rust rebuilds such a file
/// instead of failing `Engine::new` until it is deleted by hand.
#[test]
fn unsupported_or_corrupt_state_is_rebuilt() {
    let (prefix, password) = secret();
    let upper = prefix.to_uppercase();
    let cases = [
        ("future", json!({ "version": 3 })),
        (
            "no-version",
            json!({ "prefix": prefix, "password": password }),
        ),
        ("legacy-no-map", json!({ "version": 1 })),
        (
            "bad-prefix",
            json!({ "version": 2, "prefix": upper, "password": password, "port": 7890 }),
        ),
        (
            "prefix-no-pass",
            json!({ "version": 2, "prefix": prefix, "password": "", "port": 7890 }),
        ),
        (
            "pass-no-prefix",
            json!({ "version": 2, "prefix": "", "password": password, "port": 7890 }),
        ),
        (
            "bad-port",
            json!({ "version": 2, "prefix": prefix, "password": password, "port": 70000 }),
        ),
    ];
    let contents = cases
        .into_iter()
        .map(|(name, value)| (name, value.to_string()))
        .chain([("not-json", "{".to_owned())]);
    for (name, content) in contents {
        let tmp = tempfile::tempdir().unwrap();
        write_state(tmp.path(), &content);
        // Readable by others too: the rebuilt file is private all the same,
        // and the reason is the corruption.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let path = tmp.path().join(STATE_FILE);
            fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
        }
        assert!(
            matches!(decode(content.as_bytes()), Loaded::Corrupt(_)),
            "{name}"
        );
        let Ok(state) = LocalProxyState::open(tmp.path(), &any_port()) else {
            panic!("{name}: not rebuilt");
        };
        assert_eq!(
            state.credentials_reset(),
            Some(CredentialsResetReason::Corrupt),
            "{name}"
        );
        assert_eq!(
            state.status(false).credentials_reset,
            Some(CredentialsResetReason::Corrupt),
            "{name}"
        );
        #[cfg(unix)]
        assert_eq!(mode(&tmp.path().join(STATE_FILE)), 0o600, "{name}");
        let routed = state.routed_credential();
        assert!(valid_prefix(&routed.username), "{name}: invalid prefix");
        assert!(routed.password != password, "{name}: password kept");
        let disk = read_state(tmp.path());
        assert_eq!(disk.version, STATE_VERSION, "{name}");
        assert!(
            disk.prefix == routed.username,
            "{name}: prefix not persisted"
        );
    }
}

/// A state file that exists but cannot be read is an error, not a rebuild.
#[test]
fn unreadable_state_fails() {
    let tmp = tempfile::tempdir().unwrap();
    fs::create_dir(tmp.path().join(STATE_FILE)).unwrap();
    assert!(LocalProxyState::open(tmp.path(), &any_port()).is_err());
}

/// Go: TestRejectsWeakStatePermissions. Rust keeps the ports and replaces
/// the secret, written 0600.
#[cfg(unix)]
#[test]
fn weak_state_permissions_renew_the_secret() {
    use std::os::unix::fs::PermissionsExt;
    let tmp = tempfile::tempdir().unwrap();
    let port = free_port();
    let (prefix, password) = secret();
    let path = tmp.path().join(STATE_FILE);
    fs::write(
        &path,
        json!({ "version": 2, "prefix": prefix, "password": password, "port": port }).to_string(),
    )
    .unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    let state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    assert_eq!(
        state.credentials_reset(),
        Some(CredentialsResetReason::InsecurePermissions)
    );
    let routed = state.routed_credential();
    assert!(routed.username != prefix, "prefix kept");
    assert!(routed.password != password, "password kept");
    assert_eq!(routed.port, port);
    assert_eq!(mode(&path), 0o600);
    assert!(
        read_state(tmp.path()).password == routed.password,
        "new password not persisted"
    );
}

/// Go: TestRemovedNodeHasNoEndpoint.
#[test]
fn removed_node_has_no_endpoint() {
    let tmp = tempfile::tempdir().unwrap();
    let state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    let both = profile();
    let mut one = both.clone();
    one.nodes.retain(|n| n.id == NODE_1);
    let ids = |profile: &Profile| -> Vec<String> {
        state
            .metadata(Some(profile))
            .into_iter()
            .map(|m| m.node_id)
            .collect()
    };
    assert_eq!(ids(&both), [NODE_1, NODE_2, ""]);
    assert_eq!(ids(&one), [NODE_1, ""]);
    let unknown =
        |profile: Option<&Profile>, node_id: &str| match state.credential(profile, node_id) {
            Ok(_) => panic!("unknown node has a credential"),
            Err(e) => e,
        };
    let err = unknown(Some(&one), NODE_2);
    assert_eq!(err.code, codes::NODE_NOT_FOUND);
    assert_eq!(err.field.as_deref(), Some("node_id"));
    // Before the first apply there is no node at all.
    assert_eq!(unknown(None, NODE_1).code, codes::NODE_NOT_FOUND);
    assert_eq!(unknown(Some(&both), "").code, codes::NODE_NOT_FOUND);
}

/// Go: TestSystemProxyPortPrefers7891FallsBackAndPersists.
#[test]
fn system_proxy_port_prefers_7891_falls_back_and_persists() {
    let _held = hold_port(SYSTEM_PROXY_PREFERRED_PORT);
    let tmp = tempfile::tempdir().unwrap();
    let mut state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    let first = state.system_proxy_port(true).unwrap();
    assert_ne!(first, 0);
    assert_ne!(first, SYSTEM_PROXY_PREFERRED_PORT);
    assert_ne!(first, state.port());
    let disk = read_state(tmp.path());
    assert_eq!(disk.system_proxy_port, first);
    assert_eq!(disk.port, state.port());
    assert!(!disk.prefix.is_empty(), "prefix not persisted");
    drop(state);

    let mut again = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    assert_eq!(again.system_proxy_port(true).unwrap(), first);
    // A running listener keeps its port unchecked.
    let listener = TcpListener::bind(("127.0.0.1", first)).unwrap();
    assert_eq!(again.system_proxy_port(false).unwrap(), first);
    drop(listener);

    // The system proxy never takes the shared port...
    let local = again.port();
    again.state.system_proxy_port = local;
    again.system_preferred_port = local;
    assert_ne!(again.system_proxy_port(true).unwrap(), local);
    // ...and the shared port never takes the system proxy's.
    let taken = again.state.system_proxy_port;
    let (prefix, password) = secret();
    write_state(
        tmp.path(),
        &json!({
            "version": 2, "prefix": prefix, "password": password,
            "port": taken, "system_proxy_port": taken,
        })
        .to_string(),
    );
    drop(again);
    let moved = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    assert_ne!(moved.port(), taken);
    assert!(
        moved.routed_credential().username == prefix,
        "credentials not kept"
    );
}

/// Go: TestSystemProxyPortUsesPreferredWhenFree.
#[test]
fn system_proxy_port_uses_preferred_when_free() {
    let tmp = tempfile::tempdir().unwrap();
    let preferred = free_port();
    let mut state = LocalProxyState::open_with(tmp.path(), &any_port(), preferred).unwrap();
    if state.port() == preferred {
        // The kernel handed the same port out again; nothing to test.
        return;
    }
    assert_eq!(state.system_proxy_port(true).unwrap(), preferred);
}

/// Go: TestRoutedEndpoint. The routed user is the bare prefix with the
/// shared listener and password, listed last. Rust: it exists before any
/// profile (contract, section 3).
#[test]
fn routed_user_is_the_bare_prefix() {
    let tmp = tempfile::tempdir().unwrap();
    let state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    let node = state.credential(Some(&profile()), NODE_1).unwrap();
    let routed = state.routed_credential();
    let (prefix, _) = parse_username(&node.username).expect("username parses");
    assert_eq!(routed.kind, LocalProxyKind::Routed);
    assert_eq!(routed.node_id, "");
    assert!(
        routed.username == prefix,
        "routed username is not the prefix"
    );
    assert_eq!(routed.listen, node.listen);
    assert_eq!(routed.port, node.port);
    assert!(routed.password == node.password, "password not shared");

    let metadata = state.metadata(Some(&profile()));
    let last = metadata.last().unwrap();
    assert_eq!(last.kind, LocalProxyKind::Routed);
    assert_eq!(last.node_id, "");
    for entry in &metadata {
        assert_eq!(entry.listen, "127.0.0.1");
        assert_eq!(entry.port, node.port);
        assert_eq!(entry.protocols, ["http", "socks5"]);
        assert!(entry.auth_required);
    }
    let alone = state.metadata(None);
    assert_eq!(alone.len(), 1);
    assert_eq!(alone[0].kind, LocalProxyKind::Routed);
    // Metadata never carries the secret.
    let json = serde_json::to_string(&metadata).unwrap();
    assert!(
        !json.contains(&node.password),
        "metadata carries the password"
    );
}

#[test]
fn listen_must_be_an_ip_address() {
    let tmp = tempfile::tempdir().unwrap();
    let err = match LocalProxyState::open(tmp.path(), &any_port().with_listen("localhost")) {
        Ok(_) => panic!("a host name accepted"),
        Err(e) => e,
    };
    assert_eq!(err.field.as_deref(), Some("local_proxy.listen"));
    assert!(!tmp.path().join(STATE_FILE).exists());
}

#[test]
fn debug_output_has_no_secret() {
    let tmp = tempfile::tempdir().unwrap();
    let state = LocalProxyState::open(tmp.path(), &any_port()).unwrap();
    let password = state.routed_credential().password;
    assert!(
        !format!("{state:?}").contains(&password),
        "Debug shows the password"
    );
}
