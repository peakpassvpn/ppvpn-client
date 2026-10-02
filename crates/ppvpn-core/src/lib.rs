//! The Rust ppvpn-core: Profile validation and translation, failover and pin,
//! the local proxy, DNS policy and events, running sail in the host's
//! process (#45). An empty shell for now: the workspace, its CI and the sail
//! dependency come first, the modules follow one PR each.

pub mod localdns;

/// Whether a sail runtime with this id runs in this process. Here so that
/// the shell links sail (and CI builds and caches it, BoringSSL included).
pub fn sail_runtime_running(id: sail::RuntimeId) -> bool {
    sail::is_running(id)
}

#[cfg(test)]
mod tests {
    #[test]
    fn no_sail_runtime_runs_by_itself() {
        assert!(!super::sail_runtime_running(0));
    }
}
