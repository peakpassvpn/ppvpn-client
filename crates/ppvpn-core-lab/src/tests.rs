use super::*;

#[test]
fn session_secret_is_private_and_rotated() {
    let dir = std::env::temp_dir().join(format!("ppvpn-core-lab-secret-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("run/secret");
    let first = rotate_session_secret(&path).unwrap();
    let second = rotate_session_secret(&path).unwrap();
    assert_eq!(first.len(), 43, "32 bytes, base64url without padding");
    assert_ne!(first, second);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), second);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(std::fs::metadata(&path).unwrap().permissions().mode() & 0o777, 0o600);
        assert_eq!(std::fs::metadata(dir.join("run")).unwrap().permissions().mode() & 0o777, 0o700);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn lists_and_names() {
    assert_eq!(split_list(" 10.0.0.1, ,[fe80::1%en0]:53,"), ["10.0.0.1", "[fe80::1%en0]:53"]);
    assert!(["darwin", "linux", "windows"].contains(&go_os()));
    assert!(["amd64", "arm64"].contains(&go_arch()));
}
