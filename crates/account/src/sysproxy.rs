//! The OS proxy for API calls (#476), as the native apps' HTTP stacks used
//! it: the environment (`HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY`,
//! `NO_PROXY`), else on Windows the WinINet values (`ProxyEnable`,
//! `ProxyServer`, `ProxyOverride`) and on macOS the SystemConfiguration
//! proxies. Linux reads the environment only.
//!
//! Read at each request, so that turning a proxy on or off (Clash, our own
//! compatible mode) takes effect without a restart. A loopback destination
//! never goes through a proxy.

use hyper_util::client::proxy::matcher::Matcher;
use reqwest::Url;

/// The proxy the OS settings name for `url`, if any.
pub(crate) fn for_url(url: &Url) -> Option<Url> {
    if is_loopback(url) {
        return None;
    }
    let uri: http::Uri = url.as_str().parse().ok()?;
    let intercept = matcher().intercept(&uri)?;
    Url::parse(&intercept.uri().to_string()).ok()
}

fn is_loopback(url: &Url) -> bool {
    let Some(host) = url.host_str() else {
        return true;
    };
    let host = host.trim_start_matches('[').trim_end_matches(']');
    host.eq_ignore_ascii_case("localhost")
        || host
            .parse::<std::net::IpAddr>()
            .is_ok_and(|ip| ip.is_loopback())
}

#[cfg(not(windows))]
fn matcher() -> Matcher {
    Matcher::from_system()
}

/// hyper-util takes `ProxyServer` whole, but WinINet also allows one proxy
/// per scheme (`http=host:port;https=host:port;socks=host:port`, as Surge
/// writes it), and `ProxyOverride` holds `<local>` and `*.` wildcards.
#[cfg(windows)]
fn matcher() -> Matcher {
    const ENV: &[&str] = &[
        "HTTPS_PROXY", "https_proxy", "HTTP_PROXY", "http_proxy", "ALL_PROXY", "all_proxy",
    ];
    if ENV.iter().any(|name| std::env::var_os(name).is_some_and(|v| !v.is_empty())) {
        return Matcher::from_env();
    }
    let Ok(settings) = windows_registry::CURRENT_USER
        .open(r"Software\Microsoft\Windows\CurrentVersion\Internet Settings")
    else {
        return Matcher::builder().build();
    };
    if settings.get_u32("ProxyEnable").unwrap_or(0) == 0 {
        return Matcher::builder().build();
    }
    let server = settings.get_string("ProxyServer").unwrap_or_default();
    let Some(proxy) = wininet_https_proxy(&server) else {
        return Matcher::builder().build();
    };
    let no = settings
        .get_string("ProxyOverride")
        .map(|value| wininet_no_proxy(&value))
        .unwrap_or_default();
    Matcher::builder().http(proxy.clone()).https(proxy).no(no).build()
}

/// The proxy for HTTPS in a WinINet `ProxyServer` value, as a URL: the
/// `https=` entry, else `http=`, else the value itself when it names one
/// proxy for every scheme. A SOCKS-only value has none (WinINet would speak
/// SOCKS4 to it).
#[cfg_attr(not(windows), allow(dead_code))]
fn wininet_https_proxy(server: &str) -> Option<String> {
    let server = server.trim();
    let proxy = if server.contains('=') {
        let entry = |scheme: &str| {
            server.split(';').find_map(|part| {
                let (key, value) = part.split_once('=')?;
                (key.trim().eq_ignore_ascii_case(scheme) && !value.trim().is_empty())
                    .then(|| value.trim())
            })
        };
        entry("https").or_else(|| entry("http"))?
    } else {
        server
    };
    if proxy.is_empty() {
        return None;
    }
    Some(if proxy.contains("://") {
        proxy.to_string()
    } else {
        format!("http://{proxy}")
    })
}

/// A WinINet `ProxyOverride` value as a `NO_PROXY` list: `<local>` (hosts
/// without a dot) has no `NO_PROXY` form and is dropped; `*.example.com`
/// becomes `.example.com`.
#[cfg_attr(not(windows), allow(dead_code))]
fn wininet_no_proxy(value: &str) -> String {
    value
        .split(';')
        .map(str::trim)
        .filter(|entry| !entry.is_empty() && !entry.eq_ignore_ascii_case("<local>"))
        .map(|entry| entry.strip_prefix('*').unwrap_or(entry))
        .collect::<Vec<_>>()
        .join(",")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wininet_server_forms() {
        assert_eq!(
            wininet_https_proxy("127.0.0.1:7897").as_deref(),
            Some("http://127.0.0.1:7897")
        );
        assert_eq!(
            wininet_https_proxy("http=127.0.0.1:6152;https=127.0.0.1:6153;socks=127.0.0.1:6154")
                .as_deref(),
            Some("http://127.0.0.1:6153")
        );
        assert_eq!(
            wininet_https_proxy("http=10.0.0.5:3128").as_deref(),
            Some("http://10.0.0.5:3128")
        );
        assert_eq!(wininet_https_proxy("socks=127.0.0.1:1080"), None);
        assert_eq!(wininet_https_proxy("  "), None);
        assert_eq!(
            wininet_https_proxy("http://proxy.example:8080").as_deref(),
            Some("http://proxy.example:8080")
        );
    }

    #[test]
    fn wininet_override() {
        assert_eq!(
            wininet_no_proxy("localhost;127.*;*.local;<local>; *.example.com "),
            "localhost,127.*,.local,.example.com"
        );
        assert_eq!(wininet_no_proxy(""), "");
    }

    #[test]
    fn loopback_never_proxied() {
        for base in [
            "http://127.0.0.1:8080/api",
            "https://localhost/x",
            "http://[::1]:9/",
        ] {
            assert_eq!(for_url(&Url::parse(base).unwrap()), None, "{base}");
        }
    }
}
