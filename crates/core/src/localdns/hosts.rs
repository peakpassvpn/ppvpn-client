//! The system's hosts file: names in it are answered without a query (#214
//! dns-local case C6), as the Go core's transport did.

use std::collections::HashMap;
use std::net::IpAddr;

/// Addresses by lower-case name, without the trailing dot.
#[derive(Debug, Default)]
pub struct Hosts {
    names: HashMap<String, Vec<IpAddr>>,
}

impl Hosts {
    /// /etc/hosts, or %SystemRoot%\System32\drivers\etc\hosts on Windows;
    /// empty when it cannot be read.
    pub fn system() -> Hosts {
        let path = if cfg!(windows) {
            let root = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".into());
            format!(r"{root}\System32\drivers\etc\hosts")
        } else {
            "/etc/hosts".into()
        };
        std::fs::read_to_string(path)
            .map(|text| Hosts::parse(&text))
            .unwrap_or_default()
    }

    pub fn parse(text: &str) -> Hosts {
        let mut names: HashMap<String, Vec<IpAddr>> = HashMap::new();
        for line in text.lines() {
            let line = line.split('#').next().unwrap_or("");
            let mut fields = line.split_whitespace();
            let Some(Ok(ip)) = fields
                .next()
                .map(|f| f.split('%').next().unwrap_or(f).parse::<IpAddr>())
            else {
                continue;
            };
            for name in fields {
                let list = names
                    .entry(name.trim_end_matches('.').to_ascii_lowercase())
                    .or_default();
                if !list.contains(&ip) {
                    list.push(ip);
                }
            }
        }
        Hosts { names }
    }

    /// The addresses of `name` (any case, with or without the trailing dot).
    pub fn lookup(&self, name: &str) -> &[IpAddr] {
        self.names
            .get(&name.trim_end_matches('.').to_ascii_lowercase())
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_names_aliases_and_comments() {
        let hosts = Hosts::parse("127.0.0.1 localhost\n# a comment\n192.0.2.7  www.lab.test WWW2.lab.test # trailing\nfe80::1%en0 ll.lab.test\nbogus line\n");
        assert_eq!(
            hosts.lookup("www.lab.test."),
            &["192.0.2.7".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            hosts.lookup("www2.LAB.test"),
            &["192.0.2.7".parse::<IpAddr>().unwrap()]
        );
        assert_eq!(
            hosts.lookup("ll.lab.test"),
            &["fe80::1".parse::<IpAddr>().unwrap()]
        );
        assert!(hosts.lookup("missing.lab.test").is_empty());
    }
}
