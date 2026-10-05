// Service build id: SHA-256 over the service's source tree. Included by
// service/build.rs; crates/ppvpn-client/build.rs carries an identical copy
// (it must build without this directory) and its tests check the two agree.
//
// Inputs, in sorted path order with `/` separators: Cargo.toml, the
// workspace's Cargo.lock (as "Cargo.lock", when present), every file under
// src/, and the Cargo.toml and src/ files of the engine it links (as
// "engine-host/..." and "ppvpn-core/..."). Each file contributes its
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
    // The service's manifest, and the workspace's lock it builds with.
    for (name, path) in [
        ("Cargo.toml", service_dir.join("Cargo.toml")),
        ("Cargo.lock", service_dir.join("../../Cargo.lock")),
    ] {
        if path.is_file() {
            inputs.push((name.to_string(), path));
        }
    }
    walk(service_dir, &service_dir.join("src"), &mut inputs)?;
    // The engine the service links: a change to it must reinstall the
    // service as well (the lock above covers sail and the other crates).
    for (prefix, dir) in [
        ("engine-host", service_dir.join("../crates/engine-host")),
        ("ppvpn-core", service_dir.join("../../crates/ppvpn-core")),
    ] {
        if dir.join("Cargo.toml").is_file() {
            inputs.push((format!("{prefix}/Cargo.toml"), dir.join("Cargo.toml")));
            let mut sources = Vec::new();
            walk(&dir, &dir.join("src"), &mut sources)?;
            inputs.extend(
                sources
                    .into_iter()
                    .map(|(name, path)| (format!("{prefix}/{name}"), path)),
            );
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
