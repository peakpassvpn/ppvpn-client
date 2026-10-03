//! The error every public call returns (docs/host-integration.md, section
//! 7): a stable string code, the field it is about and whether retrying can
//! help. The message is for developers and not part of the contract.

use std::fmt;

/// Error codes. The strings are the contract (Core API v1's, kept); new ones
/// are only ever added.
pub mod codes {
    pub const PROFILE_REQUIRED: &str = "PROFILE_REQUIRED";
    pub const PROFILE_MALFORMED: &str = "PROFILE_MALFORMED";
    pub const SCHEMA_UNSUPPORTED: &str = "SCHEMA_UNSUPPORTED";
    pub const FIELD_REQUIRED: &str = "FIELD_REQUIRED";
    pub const TIME_RANGE_INVALID: &str = "TIME_RANGE_INVALID";
    pub const PROFILE_EXPIRED: &str = "PROFILE_EXPIRED";
    pub const NODE_ID_INVALID: &str = "NODE_ID_INVALID";
    pub const NODE_ID_DUPLICATE: &str = "NODE_ID_DUPLICATE";
    pub const ENTRY_KEY_INVALID: &str = "ENTRY_KEY_INVALID";
    pub const EXIT_IP_INVALID: &str = "EXIT_IP_INVALID";
    pub const CAPABILITIES_INVALID: &str = "CAPABILITIES_INVALID";
    pub const DEFAULT_NODE_NOT_FOUND: &str = "DEFAULT_NODE_NOT_FOUND";
    pub const SELECTION_MODE_UNSUPPORTED: &str = "SELECTION_MODE_UNSUPPORTED";
    pub const INGRESS_COUNT_INVALID: &str = "INGRESS_COUNT_INVALID";
    pub const INGRESS_ROLE_INVALID: &str = "INGRESS_ROLE_INVALID";
    pub const ENDPOINT_KEY_INVALID: &str = "ENDPOINT_KEY_INVALID";
    pub const ENDPOINT_KEY_DUPLICATE: &str = "ENDPOINT_KEY_DUPLICATE";
    pub const INGRESS_LABEL_INVALID: &str = "INGRESS_LABEL_INVALID";
    pub const REPLICA_ORDINAL_INVALID: &str = "REPLICA_ORDINAL_INVALID";
    pub const PORT_INVALID: &str = "PORT_INVALID";
    pub const ENTRY_IP_NOT_PUBLIC: &str = "ENTRY_IP_NOT_PUBLIC";
    pub const TRANSPORT_UNSUPPORTED: &str = "TRANSPORT_UNSUPPORTED";
    pub const CREDENTIALS_INVALID: &str = "CREDENTIALS_INVALID";
    pub const SHADOWSOCKS_METHOD_UNSUPPORTED: &str = "SHADOWSOCKS_METHOD_UNSUPPORTED";
    pub const SHADOWSOCKS_KEY_INVALID: &str = "SHADOWSOCKS_KEY_INVALID";
    pub const SHADOWSOCKS_SERVER_KEY_REMOVED: &str = "SHADOWSOCKS_SERVER_KEY_REMOVED";
    pub const REALITY_REQUIRED: &str = "REALITY_REQUIRED";
    pub const TLS_SERVER_NAME_INVALID: &str = "TLS_SERVER_NAME_INVALID";
    pub const REALITY_PUBLIC_KEY_INVALID: &str = "REALITY_PUBLIC_KEY_INVALID";
    pub const REALITY_SHORT_ID_INVALID: &str = "REALITY_SHORT_ID_INVALID";
    pub const TLS_REQUIRED: &str = "TLS_REQUIRED";
    pub const TLS_SERVER_NAME_MISMATCH: &str = "TLS_SERVER_NAME_MISMATCH";
    pub const PROTOCOL_UNSUPPORTED: &str = "PROTOCOL_UNSUPPORTED";
    pub const RULE_ID_INVALID: &str = "RULE_ID_INVALID";
    pub const RULE_ID_DUPLICATE: &str = "RULE_ID_DUPLICATE";
    pub const RULE_MATCH_EMPTY: &str = "RULE_MATCH_EMPTY";
    pub const DOMAIN_WILDCARD_UNSUPPORTED: &str = "DOMAIN_WILDCARD_UNSUPPORTED";
    pub const DOMAIN_INVALID: &str = "DOMAIN_INVALID";
    pub const DOMAIN_DUPLICATE: &str = "DOMAIN_DUPLICATE";
    pub const CIDR_INVALID: &str = "CIDR_INVALID";
    pub const CIDR_DUPLICATE: &str = "CIDR_DUPLICATE";
    pub const NETWORK_UNSUPPORTED: &str = "NETWORK_UNSUPPORTED";
    pub const NETWORK_DUPLICATE: &str = "NETWORK_DUPLICATE";
    pub const PORT_DUPLICATE: &str = "PORT_DUPLICATE";
    pub const PORT_RANGE_INVALID: &str = "PORT_RANGE_INVALID";
    pub const PORT_RANGE_DUPLICATE: &str = "PORT_RANGE_DUPLICATE";
    pub const ROUTING_ACTION_INVALID: &str = "ROUTING_ACTION_INVALID";
    pub const ROUTING_NODE_NOT_FOUND: &str = "ROUTING_NODE_NOT_FOUND";
    pub const ROUTING_TARGET_UNSUPPORTED: &str = "ROUTING_TARGET_UNSUPPORTED";
    pub const ROUTING_ACTION_UNSUPPORTED: &str = "ROUTING_ACTION_UNSUPPORTED";
    pub const RULE_SET_COUNT_INVALID: &str = "RULE_SET_COUNT_INVALID";
    pub const RULE_SET_ID_INVALID: &str = "RULE_SET_ID_INVALID";
    pub const RULE_SET_ID_DUPLICATE: &str = "RULE_SET_ID_DUPLICATE";
    pub const RULE_SET_URL_INVALID: &str = "RULE_SET_URL_INVALID";
    pub const RULE_SET_SHA256_INVALID: &str = "RULE_SET_SHA256_INVALID";
    pub const RULE_SET_INTERVAL_INVALID: &str = "RULE_SET_INTERVAL_INVALID";
    pub const RULE_SET_NOT_FOUND: &str = "RULE_SET_NOT_FOUND";
    pub const RULE_SET_REF_DUPLICATE: &str = "RULE_SET_REF_DUPLICATE";
    pub const RULE_SET_HOSTS_INVALID: &str = "RULE_SET_HOSTS_INVALID";
    pub const RULE_SET_HOST_NOT_ALLOWED: &str = "RULE_SET_HOST_NOT_ALLOWED";
    pub const ROUTING_MODE_INVALID: &str = "ROUTING_MODE_INVALID";
    pub const PINS_INVALID: &str = "PINS_INVALID";

    /// Every code `validate` and `apply` reject a request with before
    /// anything changes: the profile, the routing mode, the allowed rule set
    /// hosts and the pins. Retrying the same request fails again; the host
    /// shows the error and keeps what is applied. The authoritative list:
    /// hosts use it (or [`super::Error::is_profile_validation`]) rather than
    /// keeping their own. Only grows.
    pub const PROFILE_VALIDATION: &[&str] = &[
        PROFILE_REQUIRED,
        PROFILE_MALFORMED,
        SCHEMA_UNSUPPORTED,
        FIELD_REQUIRED,
        TIME_RANGE_INVALID,
        PROFILE_EXPIRED,
        NODE_ID_INVALID,
        NODE_ID_DUPLICATE,
        ENTRY_KEY_INVALID,
        EXIT_IP_INVALID,
        CAPABILITIES_INVALID,
        DEFAULT_NODE_NOT_FOUND,
        SELECTION_MODE_UNSUPPORTED,
        INGRESS_COUNT_INVALID,
        INGRESS_ROLE_INVALID,
        ENDPOINT_KEY_INVALID,
        ENDPOINT_KEY_DUPLICATE,
        INGRESS_LABEL_INVALID,
        REPLICA_ORDINAL_INVALID,
        PORT_INVALID,
        ENTRY_IP_NOT_PUBLIC,
        TRANSPORT_UNSUPPORTED,
        CREDENTIALS_INVALID,
        SHADOWSOCKS_METHOD_UNSUPPORTED,
        SHADOWSOCKS_KEY_INVALID,
        SHADOWSOCKS_SERVER_KEY_REMOVED,
        REALITY_REQUIRED,
        TLS_SERVER_NAME_INVALID,
        REALITY_PUBLIC_KEY_INVALID,
        REALITY_SHORT_ID_INVALID,
        TLS_REQUIRED,
        TLS_SERVER_NAME_MISMATCH,
        PROTOCOL_UNSUPPORTED,
        RULE_ID_INVALID,
        RULE_ID_DUPLICATE,
        RULE_MATCH_EMPTY,
        DOMAIN_WILDCARD_UNSUPPORTED,
        DOMAIN_INVALID,
        DOMAIN_DUPLICATE,
        CIDR_INVALID,
        CIDR_DUPLICATE,
        NETWORK_UNSUPPORTED,
        NETWORK_DUPLICATE,
        PORT_DUPLICATE,
        PORT_RANGE_INVALID,
        PORT_RANGE_DUPLICATE,
        ROUTING_ACTION_INVALID,
        ROUTING_NODE_NOT_FOUND,
        ROUTING_TARGET_UNSUPPORTED,
        ROUTING_ACTION_UNSUPPORTED,
        RULE_SET_COUNT_INVALID,
        RULE_SET_ID_INVALID,
        RULE_SET_ID_DUPLICATE,
        RULE_SET_URL_INVALID,
        RULE_SET_SHA256_INVALID,
        RULE_SET_INTERVAL_INVALID,
        RULE_SET_NOT_FOUND,
        RULE_SET_REF_DUPLICATE,
        RULE_SET_HOSTS_INVALID,
        RULE_SET_HOST_NOT_ALLOWED,
        ROUTING_MODE_INVALID,
        PINS_INVALID,
    ];

    // Lifecycle, queries and probes (Core API v1's).
    pub const PROFILE_NOT_APPLIED: &str = "PROFILE_NOT_APPLIED";
    pub const CORE_NOT_RUNNING: &str = "CORE_NOT_RUNNING";
    pub const NODE_NOT_FOUND: &str = "NODE_NOT_FOUND";
    pub const INGRESS_NOT_FOUND: &str = "INGRESS_NOT_FOUND";
    pub const LOCAL_PROXY_DISABLED: &str = "LOCAL_PROXY_DISABLED";
    pub const SYSTEM_PROXY_UNAVAILABLE: &str = "SYSTEM_PROXY_UNAVAILABLE";
    pub const SYSTEM_PROXY_START_FAILED: &str = "SYSTEM_PROXY_START_FAILED";
    pub const NO_DEFAULT_INTERFACE: &str = "NO_DEFAULT_INTERFACE";
    pub const PROBE_METHOD_UNSUPPORTED: &str = "PROBE_METHOD_UNSUPPORTED";
    pub const RULE_SET_STORAGE_UNAVAILABLE: &str = "RULE_SET_STORAGE_UNAVAILABLE";
    /// An internal error; the log says why.
    pub const CORE_OPERATION_FAILED: &str = "CORE_OPERATION_FAILED";

    // New with the library (docs/host-integration.md, section 7).
    pub const CORE_PANICKED: &str = "CORE_PANICKED";
    pub const ENGINE_FATAL: &str = "ENGINE_FATAL";
    pub const ENGINE_SHUT_DOWN: &str = "ENGINE_SHUT_DOWN";
    pub const TUN_INSTANCE_EXISTS: &str = "TUN_INSTANCE_EXISTS";
    pub const STATE_DIR_IN_USE: &str = "STATE_DIR_IN_USE";
    pub const PERMISSION_DENIED: &str = "PERMISSION_DENIED";
    pub const WINTUN_UNAVAILABLE: &str = "WINTUN_UNAVAILABLE";
}

/// The error of a public call.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[non_exhaustive]
pub struct Error {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub field: Option<String>,
    pub retryable: bool,
    pub message: String,
}

impl Error {
    /// The request itself was rejected by validation
    /// ([`codes::PROFILE_VALIDATION`]): not retryable, nothing changed.
    pub fn is_profile_validation(&self) -> bool {
        codes::PROFILE_VALIDATION.contains(&self.code)
    }

    /// An error with a code that is not about one field.
    pub(crate) fn new(code: &'static str, retryable: bool, message: impl Into<String>) -> Self {
        Self {
            code,
            field: None,
            retryable,
            message: message.into(),
        }
    }

    /// What the skeleton's unimplemented calls return.
    #[allow(dead_code)] // every public call is wired at the moment
    pub(crate) fn not_implemented(call: &str) -> Self {
        Self::new(
            codes::CORE_OPERATION_FAILED,
            false,
            format!("{call}: not implemented"),
        )
    }

    /// A validation error: never retryable (the same input fails again).
    pub(crate) fn invalid(
        code: &'static str,
        field: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        let field = field.into();
        Self {
            code,
            field: (!field.is_empty()).then_some(field),
            retryable: false,
            message: message.into(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match &self.field {
            Some(field) => write!(f, "{}: {} ({field})", self.code, self.message),
            None => write!(f, "{}: {}", self.code, self.message),
        }
    }
}

impl std::error::Error for Error {}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn profile_validation_codes_are_the_validation_ones() {
        let all: HashSet<_> = codes::PROFILE_VALIDATION.iter().collect();
        assert_eq!(all.len(), codes::PROFILE_VALIDATION.len(), "a code twice");
        for code in [
            codes::PROFILE_REQUIRED,
            codes::PROFILE_MALFORMED,
            codes::PROFILE_EXPIRED,
            codes::INGRESS_LABEL_INVALID,
            codes::ROUTING_MODE_INVALID,
            codes::RULE_SET_HOST_NOT_ALLOWED,
            codes::PINS_INVALID,
        ] {
            assert!(
                Error::invalid(code, "", "").is_profile_validation(),
                "{code}"
            );
        }
        // Not about the request: retrying or another call may help.
        for code in [
            codes::PROFILE_NOT_APPLIED,
            codes::CORE_OPERATION_FAILED,
            codes::NODE_NOT_FOUND,
            codes::RULE_SET_STORAGE_UNAVAILABLE,
            codes::ENGINE_SHUT_DOWN,
            codes::STATE_DIR_IN_USE,
        ] {
            assert!(
                !Error::new(code, false, "").is_profile_validation(),
                "{code}"
            );
        }
    }
}
