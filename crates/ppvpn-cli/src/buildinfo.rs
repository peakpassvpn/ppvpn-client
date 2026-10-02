//! Build profile and API origin.
//!
//! Release builds talk to the production API and ignore any override. Only a
//! `dev` build accepts `PPVPN_API_BASE`, and only an HTTPS origin (plain HTTP
//! is allowed for loopback development). The profile and a non-production
//! default origin are set at compile time through `PPVPN_BUILD_PROFILE` and
//! `PPVPN_DEFAULT_API_BASE`, so no environment-specific origin lives in the
//! source.

use std::net::IpAddr;

use crate::env::Env;
use crate::error::{CliError, Result};

pub const PRODUCTION_API_BASE: &str = "https://www.peakpassvpn.com";

#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Profile {
    Prod,
    Dev,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildConfig {
    pub profile: Profile,
    /// An origin: scheme, host and optional port, without a trailing slash.
    pub api_base: String,
}

pub fn version() -> &'static str {
    option_env!("PPVPN_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

/// The compiled-in build values.
pub fn compiled() -> (Option<&'static str>, Option<&'static str>) {
    (
        option_env!("PPVPN_BUILD_PROFILE"),
        option_env!("PPVPN_DEFAULT_API_BASE"),
    )
}

pub fn resolve(env: &Env) -> Result<BuildConfig> {
    let (profile, default_base) = compiled();
    resolve_with(profile, default_base, env)
}

pub fn resolve_with(
    profile: Option<&str>,
    default_base: Option<&str>,
    env: &Env,
) -> Result<BuildConfig> {
    let invalid =
        |message: &str| CliError::argument(message.to_string()).with_code("BUILD_CONFIG_INVALID");
    let profile = match profile.unwrap_or("prod") {
        "prod" => Profile::Prod,
        "dev" => Profile::Dev,
        _ => return Err(invalid("unsupported build profile")),
    };
    let raw = match profile {
        Profile::Prod => default_base.unwrap_or(PRODUCTION_API_BASE),
        Profile::Dev => env
            .var("PPVPN_API_BASE")
            .or(default_base)
            .ok_or_else(|| invalid("a dev build needs PPVPN_API_BASE"))?,
    };
    let api_base = validate_origin(raw.trim(), profile == Profile::Dev)
        .map_err(|message| invalid(&message))?;
    Ok(BuildConfig { profile, api_base })
}

/// Checks that `raw` is an origin and returns it without a trailing slash.
fn validate_origin(raw: &str, allow_loopback_http: bool) -> std::result::Result<String, String> {
    let (scheme, rest) = raw
        .split_once("://")
        .ok_or("API base must be an origin such as https://example.com")?;
    let authority = rest.strip_suffix('/').unwrap_or(rest);
    if authority.is_empty() || authority.contains(['/', '?', '#', '@']) {
        return Err(
            "API base must be an origin without credentials, path, query, or fragment".into(),
        );
    }
    match scheme {
        "https" => {}
        "http" if allow_loopback_http && is_loopback(authority) => {}
        _ => {
            return Err(
                "API base must use HTTPS (HTTP is allowed only for loopback development)".into(),
            )
        }
    }
    Ok(format!("{scheme}://{authority}"))
}

fn is_loopback(authority: &str) -> bool {
    let host = if let Some(rest) = authority.strip_prefix('[') {
        rest.split_once(']').map(|(h, _)| h).unwrap_or(rest)
    } else {
        authority
            .rsplit_once(':')
            .map(|(h, _)| h)
            .unwrap_or(authority)
    };
    host.eq_ignore_ascii_case("localhost")
        || host.parse::<IpAddr>().is_ok_and(|ip| ip.is_loopback())
}

/// The rule-set download hosts core may use: the API origin's authority,
/// lowercase, with the port omitted only when it is 443 (as desktop does:
/// `http://localhost` is `localhost:80`).
pub fn rule_set_hosts(api_base: &str) -> Vec<String> {
    let Some((scheme, authority)) = api_base.split_once("://") else {
        return Vec::new();
    };
    let authority = authority.trim_end_matches('/').to_ascii_lowercase();
    let host_end = if authority.starts_with('[') {
        authority.find(']').map(|i| i + 1)
    } else {
        authority.rfind(':').or(Some(authority.len()))
    };
    let Some(host_end) = host_end else {
        return Vec::new();
    };
    let (host, port) = authority.split_at(host_end);
    if host.is_empty() || host.contains(['/', '?', '#', '@', ' ']) {
        return Vec::new();
    }
    let port = match port.strip_prefix(':') {
        Some(port) => port,
        None if scheme == "http" => "80",
        None => "443",
    };
    if port == "443" {
        vec![host.to_string()]
    } else {
        vec![format!("{host}:{port}")]
    }
}

impl CliError {
    fn with_code(mut self, code: &str) -> Self {
        self.code = code.to_string();
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{Env, Os};

    fn env(vars: &[(&str, &str)]) -> Env {
        Env::with_vars(Os::Linux, vars)
    }

    #[test]
    fn release_builds_ignore_the_override() {
        let config = resolve_with(
            None,
            None,
            &env(&[("PPVPN_API_BASE", "https://evil.example.test")]),
        )
        .unwrap();
        assert_eq!(config.profile, Profile::Prod);
        assert_eq!(config.api_base, PRODUCTION_API_BASE);
    }

    #[test]
    fn dev_builds_take_the_override_and_check_it() {
        let config = resolve_with(
            Some("dev"),
            Some("https://api.example.test"),
            &env(&[("PPVPN_API_BASE", "http://127.0.0.1:8080/")]),
        )
        .unwrap();
        assert_eq!(config.api_base, "http://127.0.0.1:8080");
        let config =
            resolve_with(Some("dev"), Some("https://api.example.test"), &env(&[])).unwrap();
        assert_eq!(config.api_base, "https://api.example.test");
        for bad in [
            "http://api.example.test",
            "https://api.example.test/v1",
            "https://u:p@api.example.test",
            "api.example.test",
        ] {
            let err =
                resolve_with(Some("dev"), None, &env(&[("PPVPN_API_BASE", bad)])).unwrap_err();
            assert_eq!(
                (err.exit_code(), err.code.as_str()),
                (2, "BUILD_CONFIG_INVALID"),
                "{bad}"
            );
        }
        assert!(resolve_with(Some("dev"), None, &env(&[])).is_err());
        assert!(resolve_with(Some("beta"), None, &env(&[])).is_err());
    }

    #[test]
    fn rule_set_hosts_are_the_api_authority() {
        assert_eq!(
            rule_set_hosts("https://API.Example.test"),
            ["api.example.test"]
        );
        assert_eq!(
            rule_set_hosts("https://api.example.test:443"),
            ["api.example.test"]
        );
        assert_eq!(
            rule_set_hosts("https://api.example.test:8443"),
            ["api.example.test:8443"]
        );
        assert_eq!(rule_set_hosts("http://localhost"), ["localhost:80"]);
        assert_eq!(rule_set_hosts("https://[2001:db8::1]"), ["[2001:db8::1]"]);
        assert_eq!(rule_set_hosts("http://127.0.0.1:8080"), ["127.0.0.1:8080"]);
        assert_eq!(
            rule_set_hosts("https://[2001:db8::1]:9443"),
            ["[2001:db8::1]:9443"]
        );
        assert!(rule_set_hosts("not a url").is_empty());
    }
}
