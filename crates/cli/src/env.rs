//! The process environment, behind a type so tests can supply their own
//! values without mutating process-wide state.

use std::collections::HashMap;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Os {
    Linux,
    Macos,
}

impl Os {
    pub fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone)]
pub struct Env {
    pub os: Os,
    vars: HashMap<String, String>,
}

impl Env {
    pub fn from_process() -> Env {
        Env {
            os: Os::current(),
            vars: std::env::vars().collect(),
        }
    }

    /// An environment with exactly these variables (tests, embedding).
    pub fn with_vars(os: Os, vars: &[(&str, &str)]) -> Env {
        Env {
            os,
            vars: vars
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    /// A variable's value, or `None` when it is unset or empty.
    pub fn var(&self, name: &str) -> Option<&str> {
        self.vars
            .get(name)
            .map(String::as_str)
            .filter(|v| !v.is_empty())
    }
}
