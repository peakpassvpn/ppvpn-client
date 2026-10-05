//! Command-line flags as Go's flag package reads them, so that the lab
//! scripts start this binary exactly as they start `ppvpn-core serve`:
//! `-name value`, `--name value`, `-name=value`; a bool flag is `-name` or
//! `-name=false`; parsing stops at the first argument that is not a flag,
//! or after `--`.

use std::collections::HashMap;

#[derive(Clone, Copy, PartialEq)]
enum Kind {
    String,
    Bool,
}

pub struct FlagSet {
    name: &'static str,
    defs: Vec<(&'static str, Kind, String, &'static str)>,
    values: HashMap<&'static str, String>,
}

impl FlagSet {
    pub fn new(name: &'static str) -> Self {
        FlagSet {
            name,
            defs: Vec::new(),
            values: HashMap::new(),
        }
    }

    pub fn string(&mut self, name: &'static str, default: &str, usage: &'static str) {
        self.defs.push((name, Kind::String, default.into(), usage));
    }

    pub fn bool(&mut self, name: &'static str, default: bool, usage: &'static str) {
        self.defs
            .push((name, Kind::Bool, default.to_string(), usage));
    }

    /// Parses `args`; the error reads as Go's.
    pub fn parse(&mut self, args: &[String]) -> Result<Vec<String>, String> {
        let mut i = 0;
        while i < args.len() {
            let arg = &args[i];
            if arg == "--" {
                i += 1;
                break;
            }
            let Some(body) = arg.strip_prefix("--").or_else(|| arg.strip_prefix('-')) else {
                break;
            };
            if body.is_empty() || body.starts_with('-') || body.starts_with('=') {
                return Err(format!("bad flag syntax: {arg}"));
            }
            let (name, inline) = match body.split_once('=') {
                Some((n, v)) => (n, Some(v.to_string())),
                None => (body, None),
            };
            let Some(&(def_name, kind, _, _)) = self.defs.iter().find(|d| d.0 == name) else {
                return Err(format!("flag provided but not defined: -{name}"));
            };
            let value = match (kind, inline) {
                (Kind::Bool, Some(v)) => match parse_bool(&v) {
                    Some(b) => b.to_string(),
                    None => {
                        return Err(format!(
                            "invalid boolean value {v:?} for -{name}: parse error"
                        ))
                    }
                },
                (Kind::Bool, None) => "true".into(),
                (Kind::String, Some(v)) => v,
                (Kind::String, None) => {
                    i += 1;
                    match args.get(i) {
                        Some(v) => v.clone(),
                        None => return Err(format!("flag needs an argument: -{name}")),
                    }
                }
            };
            self.values.insert(def_name, value);
            i += 1;
        }
        Ok(args[i.min(args.len())..].to_vec())
    }

    pub fn get(&self, name: &str) -> String {
        if let Some(v) = self.values.get(name) {
            return v.clone();
        }
        self.defs
            .iter()
            .find(|d| d.0 == name)
            .map(|d| d.2.clone())
            .unwrap_or_default()
    }

    pub fn get_bool(&self, name: &str) -> bool {
        self.get(name) == "true"
    }

    pub fn usage(&self) -> String {
        let mut out = format!("Usage of {}:\n", self.name);
        for (name, kind, default, usage) in &self.defs {
            let arg = if *kind == Kind::String { " string" } else { "" };
            out.push_str(&format!("  -{name}{arg}\n    \t{usage}"));
            if !default.is_empty() && default != "false" {
                out.push_str(&format!(" (default {default:?})"));
            }
            out.push('\n');
        }
        out
    }
}

/// strconv.ParseBool.
fn parse_bool(s: &str) -> Option<bool> {
    match s {
        "1" | "t" | "T" | "TRUE" | "true" | "True" => Some(true),
        "0" | "f" | "F" | "FALSE" | "false" | "False" => Some(false),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn set() -> FlagSet {
        let mut f = FlagSet::new("serve");
        f.string("socket", "", "");
        f.bool("tun", false, "");
        f.bool("local-proxy", true, "");
        f
    }

    fn args(s: &[&str]) -> Vec<String> {
        s.iter().map(|a| a.to_string()).collect()
    }

    #[test]
    fn go_flag_forms() {
        let mut f = set();
        let rest = f
            .parse(&args(&[
                "--socket",
                "/a",
                "-tun",
                "--local-proxy=false",
                "x",
                "-socket=b",
            ]))
            .unwrap();
        assert_eq!(
            (
                f.get("socket").as_str(),
                f.get_bool("tun"),
                f.get_bool("local-proxy")
            ),
            ("/a", true, false)
        );
        assert_eq!(rest, ["x", "-socket=b"]);
        let mut f = set();
        f.parse(&args(&["-socket=/b"])).unwrap();
        assert_eq!(
            (
                f.get("socket").as_str(),
                f.get_bool("tun"),
                f.get_bool("local-proxy")
            ),
            ("/b", false, true)
        );
    }

    #[test]
    fn errors_read_as_gos() {
        assert_eq!(
            set().parse(&args(&["--nope"])).unwrap_err(),
            "flag provided but not defined: -nope"
        );
        assert_eq!(
            set().parse(&args(&["--socket"])).unwrap_err(),
            "flag needs an argument: -socket"
        );
        assert!(set()
            .parse(&args(&["--tun=maybe"]))
            .unwrap_err()
            .starts_with("invalid boolean value"));
    }
}
