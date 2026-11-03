use super::*;

fn fixture(name: &str) -> Vec<u8> {
    std::fs::read(
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("src/rulesets/testdata")
            .join(name),
    )
    .unwrap()
}

#[test]
fn a_growing_reader_is_bounded_and_the_exact_limit_is_accepted() {
    let mut reader = std::io::Cursor::new(vec![1; 64]);
    assert_eq!(
        read_limited(&mut reader, 8).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
    assert_eq!(reader.position(), 9, "never read past the bound plus one");
    assert_eq!(read_limited(&[1; 8][..], 8).unwrap(), vec![1; 8]);
}

#[test]
fn an_oversized_sparse_cache_file_is_rejected() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("large.srs");
    let file = std::fs::File::create(&path).unwrap();
    file.set_len(MAX_SIZE as u64 + 1).unwrap();
    assert_eq!(
        read_bounded(&path).unwrap_err().kind(),
        io::ErrorKind::InvalidData
    );
}

#[test]
fn object_names_are_digests_and_corrupt_objects_are_not_reused() {
    let dir = tempfile::tempdir().unwrap();
    let mut cache = Cache::new(Some(dir.path().into()));
    for digest in ["../escape", "a/b", "", &"g".repeat(64)] {
        assert!(cache.object_path(digest).is_none());
    }
    let data = fixture("domains.srs");
    let digest = sha256_hex(&data);
    let object = cache.install(&data, &digest).unwrap();
    std::fs::write(&object.path, fixture("domains-other.srs")).unwrap();
    assert!(cache.load_object(&digest).is_none());
    cache.install(&data, &digest).unwrap();
    assert_eq!(std::fs::read(&object.path).unwrap(), data);
}

#[cfg(unix)]
#[test]
fn the_cache_repairs_old_directory_permissions_and_keeps_files_private() {
    use std::os::unix::fs::PermissionsExt;
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("rule-sets");
    std::fs::create_dir(&root).unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o755)).unwrap();
    let mut cache = Cache::new(Some(root.clone()));
    let data = fixture("domains.srs");
    let object = cache.install(&data, &sha256_hex(&data)).unwrap();
    for path in [&root, &root.join("objects")] {
        assert_eq!(
            std::fs::metadata(path).unwrap().permissions().mode() & 0o777,
            0o700
        );
    }
    assert_eq!(
        std::fs::metadata(&object.path)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
