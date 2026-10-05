//! End-to-end test switch: `PPVPN_TEST_TRUST_LOCAL_BACKEND=1` lets a release
//! build talk to a local mock backend. It applies only when the configured
//! API base is a loopback host (`127.0.0.1`, `::1`, `localhost`); then, and
//! only then:
//!
//! - plain `http` is allowed (and a self-signed certificate on `https`);
//! - the backend's own loopback verification URL is trusted;
//! - the device-code minimum length is relaxed.
//!
//! Non-loopback hosts are never relaxed, whatever the variable says.

/// The opt-in environment variable.
pub(crate) const ENV: &str = "PPVPN_TEST_TRUST_LOCAL_BACKEND";

/// The variable is set to `1`.
pub(crate) fn env_enabled() -> bool {
    std::env::var(ENV).is_ok_and(|value| value.trim() == "1")
}

/// Logs once per client when the switch is set.
pub(crate) fn log_startup(base: &str) {
    if !env_enabled() {
        return;
    }
    if ppvpn_account::api::is_loopback_base(base) {
        tracing::warn!(
            "{ENV}=1: TEST MODE - trusting the local backend {base} (http, self-signed TLS, loopback verification URL, short device codes)"
        );
    } else {
        tracing::warn!("{ENV}=1 ignored: the API base is not a loopback host");
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn only_loopback_hosts_with_the_switch_are_trusted() {
        let trusted = |enabled: bool, base: &str| {
            crate::api::client_api_with_test_mode(base, enabled).trusts_local_backend()
        };
        for base in [
            "http://127.0.0.1:8080",
            "https://127.0.0.1",
            "http://[::1]:9000/api",
            "http://localhost:3000",
            "http://LOCALHOST",
        ] {
            assert!(trusted(true, base), "{base}");
            assert!(!trusted(false, base), "unset: {base}");
        }
        for base in [
            "https://www.example.com",
            "http://staging.example.com",
            "http://192.0.2.10:8080",
            "http://127.0.0.1.nip.io",
            "http://localhost.example.com",
            "not a url",
        ] {
            assert!(!trusted(true, base), "{base}");
        }
    }
}
