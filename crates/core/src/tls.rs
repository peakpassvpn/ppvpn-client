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

/// For tests: a certificate authority made for the run and a TLS server
/// (sail's BoringSSL) holding a certificate it issued for 127.0.0.1.
#[cfg(test)]
pub(crate) mod testing {
    use btls::asn1::Asn1Time;
    use btls::bn::BigNum;
    use btls::ec::{EcGroup, EcKey};
    use btls::hash::MessageDigest;
    use btls::nid::Nid;
    use btls::pkey::{PKey, Private};
    use btls::ssl::{SslAcceptor, SslMethod};
    use btls::x509::extension::{BasicConstraints, SubjectAlternativeName};
    use btls::x509::{X509Name, X509NameBuilder, X509};

    /// The authority named `ca_name` (PEM) and an acceptor whose
    /// certificate, for the IP address 127.0.0.1 alone, it issued.
    pub(crate) fn authority(ca_name: &str) -> (String, SslAcceptor) {
        fn key() -> PKey<Private> {
            let group = EcGroup::from_curve_name(Nid::X9_62_PRIME256V1).unwrap();
            PKey::from_ec_key(EcKey::generate(&group).unwrap()).unwrap()
        }
        fn name(cn: &str) -> X509Name {
            let mut name = X509NameBuilder::new().unwrap();
            name.append_entry_by_text("CN", cn).unwrap();
            name.build()
        }
        fn cert(
            serial: u32,
            subject: &str,
            key: &PKey<Private>,
            issuer: Option<(&X509, &PKey<Private>)>,
        ) -> X509 {
            let mut builder = X509::builder().unwrap();
            builder.set_version(2).unwrap();
            let serial = BigNum::from_u32(serial).unwrap().to_asn1_integer().unwrap();
            builder.set_serial_number(&serial).unwrap();
            builder.set_subject_name(&name(subject)).unwrap();
            builder.set_pubkey(key).unwrap();
            builder
                .set_not_before(&Asn1Time::days_from_now(0).unwrap())
                .unwrap();
            builder
                .set_not_after(&Asn1Time::days_from_now(2).unwrap())
                .unwrap();
            let signer = match issuer {
                Some((ca, ca_key)) => {
                    builder.set_issuer_name(ca.subject_name()).unwrap();
                    let san = SubjectAlternativeName::new()
                        .ip("127.0.0.1")
                        .build(&builder.x509v3_context(Some(&**ca), None))
                        .unwrap();
                    builder.append_extension(&san).unwrap();
                    ca_key
                }
                None => {
                    builder.set_issuer_name(&name(subject)).unwrap();
                    let ca = BasicConstraints::new().critical().ca().build().unwrap();
                    builder.append_extension(&ca).unwrap();
                    key
                }
            };
            builder.sign(signer, MessageDigest::sha256()).unwrap();
            builder.build()
        }
        let ca_key = key();
        let ca = cert(1, ca_name, &ca_key, None);
        let leaf_key = key();
        let leaf = cert(2, "127.0.0.1", &leaf_key, Some((&ca, &ca_key)));
        let mut acceptor = SslAcceptor::mozilla_intermediate_v5(SslMethod::tls()).unwrap();
        acceptor.set_private_key(&leaf_key).unwrap();
        acceptor.set_certificate(&leaf).unwrap();
        let pem = String::from_utf8(ca.to_pem().unwrap()).unwrap();
        (pem, acceptor.build())
    }
}
