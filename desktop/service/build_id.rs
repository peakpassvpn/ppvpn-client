// Service build id: SHA-256 over the service's source tree. Included by
// service/build.rs; crates/ppvpn-client/build.rs carries an identical copy
// (it must build without this directory) and its tests check the two agree.
//
// Inputs, in sorted path order with `/` separators: Cargo.toml, Cargo.lock
// (when present), every file under src/ and the current vendored core's
// manifest.json (as "vendor/ppvpn-core/manifest.json"). Each file contributes its
// relative path, a NUL, its length (u64 LE) and its bytes.

/// The files that make up the service build id, relative to `service_dir`.
fn service_build_inputs(
    service_dir: &std::path::Path,
) -> std::io::Result<Vec<(String, std::path::PathBuf)>> {
    fn walk(
        root: &std::path::Path,
        dir: &std::path::Path,
        out: &mut Vec<(String, std::path::PathBuf)>,
    ) -> std::io::Result<()> {
        for entry in std::fs::read_dir(dir)? {
            let path = entry?.path();
            if path.is_dir() {
                walk(root, &path, out)?;
            } else if path.is_file() {
                out.push((relative_name(root, &path), path));
            }
        }
        Ok(())
    }
    fn relative_name(root: &std::path::Path, path: &std::path::Path) -> String {
        let relative = path.strip_prefix(root).unwrap_or(path);
        relative
            .components()
            .map(|part| part.as_os_str().to_string_lossy().into_owned())
            .collect::<Vec<_>>()
            .join("/")
    }

    let mut inputs = Vec::new();
    for name in ["Cargo.toml", "Cargo.lock"] {
        let path = service_dir.join(name);
        if path.is_file() {
            inputs.push((name.to_string(), path));
        }
    }
    walk(service_dir, &service_dir.join("src"), &mut inputs)?;
    // The privileged core ships with the service: a new vendored core must
    // reinstall it as well. Its manifest names every binary's SHA-256.
    let vendor = service_dir.join("../vendor/ppvpn-core");
    if let Ok(current) = std::fs::read_to_string(vendor.join("CURRENT")) {
        let manifest = vendor.join(current.trim()).join("manifest.json");
        if manifest.is_file() {
            inputs.push(("vendor/ppvpn-core/manifest.json".to_string(), manifest));
        }
    }
    inputs.sort_by(|a, b| a.0.cmp(&b.0));
    Ok(inputs)
}

/// Lowercase hex SHA-256 of [`service_build_inputs`].
fn service_build_id(service_dir: &std::path::Path) -> std::io::Result<String> {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    for (name, path) in service_build_inputs(service_dir)? {
        let bytes = std::fs::read(&path)?;
        hasher.update(name.as_bytes());
        hasher.update([0u8]);
        hasher.update((bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect())
}
