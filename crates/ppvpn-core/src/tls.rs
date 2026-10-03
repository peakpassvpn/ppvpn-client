//! The core's own TLS client: sail's (BoringSSL, the system's roots), for
//! what the core fetches over HTTPS itself (the availability probe, rule
//! set downloads). One place decides how it is made.

use sail::config::model::CertificateStore;
use sail::transport::tls::roots::Roots;
use sail::transport::tls::TlsClient;

/// A client offering `alpn`. `trust_pem` replaces the system's roots
/// (tests). The error is for the log.
pub(crate) fn client(alpn: &[&str], trust_pem: Option<&str>) -> Result<TlsClient, String> {
    let store = if trust_pem.is_some() {
        CertificateStore::None
    } else {
        CertificateStore::System
    };
    let alpn: Vec<String> = alpn.iter().map(|p| (*p).to_owned()).collect();
    Roots::of(store)
        .and_then(|roots| TlsClient::new(&alpn, trust_pem, false, None, &roots))
        .map_err(|e| format!("{e:#}"))
}
