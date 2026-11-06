//! The proxy profile: model, strict decoding and validation (Go's `profile`
//! package, ported; the golden contract in testdata/golden/contract is the
//! reference).

mod addr;
mod model;
mod parse;
mod ruleset;
mod validate;

pub use model::*;
pub use parse::parse;
pub use ruleset::validate_rule_set_hosts;
#[cfg(feature = "forwarder")]
pub use ruleset::{normalize_rule_set_host, rule_set_host};
pub use validate::validate;
#[cfg(feature = "forwarder")]
pub use validate::{normalize_domain, parse_port_range};
