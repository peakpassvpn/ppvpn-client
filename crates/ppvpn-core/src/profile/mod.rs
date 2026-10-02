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
pub use validate::validate;
