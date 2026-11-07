//! Adapt the shared audit-token verifier to the App's signing checks.
//! Service-generation caching and acceptance tracing belong to the service;
//! the UI retains full static signature validation on every peer check.

mod image_verification {
    use anyhow::Result;
    use security_framework::os::macos::code_signing::SecRequirement;
    use std::path::Path;

    pub fn verify(image: &Path, _identifier: &str, requirement: &SecRequirement) -> Result<()> {
        crate::mac_identity::check_file(image, requirement)
    }
}

mod native {
    use anyhow::Result;

    pub fn timed<T>(_stage: &'static str, work: impl FnOnce() -> Result<T>) -> Result<T> {
        work()
    }
}

#[path = "../../service/src/single_runtime/mac_peer.rs"]
mod shared;
pub use shared::verify;
