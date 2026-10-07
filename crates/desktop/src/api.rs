//! The backend API as this client uses it: [`ppvpn_account::api`] plus the
//! mapping of its errors to [`ClientError`] codes, the conversions to the
//! UniFFI records, and the proxy-profile summary the UI shows.
//!
//! Errors never carry response bodies or credentials: only the HTTP status and
//! the backend problem `code` travel into [`ClientError`] details.

use reqwest::StatusCode;

pub(crate) use ppvpn_account::api::*;
use ppvpn_account::profile::{NodeView, ProfileView, ReplicaView};

use crate::errors::{http_error, transport_error, ClientError, ErrorCode};
use crate::{Account, Node, ProfileSummary, Replica, Team};

/// The API client for `base`, honouring the local-backend test switch
/// ([`crate::testmode`]) and sending the system language.
pub(crate) fn client_api(base: &str) -> Api {
    client_api_with_test_mode(base, crate::testmode::env_enabled())
}

/// [`client_api`] with the test switch given explicitly.
pub(crate) fn client_api_with_test_mode(base: &str, test_switch: bool) -> Api {
    Api::new(ApiConfig::desktop(
        base,
        Some(crate::device::locale()),
        test_switch,
    ))
}

impl From<UserInfo> for Account {
    fn from(user: UserInfo) -> Self {
        Self {
            id: user.id,
            name: user.name,
            avatar_url: user.avatar_url,
            email: user.email,
        }
    }
}

/// The UniFFI view of a `GET /me/teams` item.
pub(crate) trait ToTeam {
    fn to_team(&self) -> Team;
}

impl ToTeam for TeamEntry {
    fn to_team(&self) -> Team {
        Team {
            id: self.id.clone(),
            name: self.name.clone(),
            personal: self.is_personal,
            active: self.is_active(),
        }
    }
}

/// [`ApiError`] mapped to the client's error codes.
pub(crate) trait ApiErrorExt {
    fn into_client_error(self) -> ClientError;
    fn into_auth_error(self) -> ClientError;
    fn into_profile_error(self) -> ClientError;
}

impl ApiErrorExt for ApiError {
    /// Generic mapping for bearer endpoints, with subscription problem codes
    /// recognised on any status.
    fn into_client_error(self) -> ClientError {
        match self {
            Self::Transport(error) => transport_error(&error),
            Self::Status {
                request,
                status,
                code,
                expired_at,
            } => {
                if let Some(mapped) = subscription_code(status, code.as_deref()) {
                    let mut detail = status_detail(&request, status, code.as_deref());
                    if mapped == ErrorCode::SubscriptionExpired {
                        if let Some(at) = expired_at {
                            detail.push_str(&format!(" {EXPIRED_AT_TAG}{at}"));
                        }
                    }
                    return ClientError::failed(mapped, detail);
                }
                http_error(&request, status, code.as_deref())
            }
            Self::Decode(what) => ClientError::failed(ErrorCode::ServerUnavailable, what),
            Self::Config(what) => ClientError::failed(ErrorCode::Internal, what),
        }
    }

    /// Mapping for device-credential endpoints: 401/403 mean the saved
    /// session is gone.
    fn into_auth_error(self) -> ClientError {
        match self {
            Self::Status {
                request,
                status,
                code,
                ..
            } if status == StatusCode::FORBIDDEN
                && code.as_deref() != Some(PROBLEM_TEAM_DISABLED) =>
            {
                ClientError::failed(
                    ErrorCode::AuthSessionInvalid,
                    status_detail(&request, status, code.as_deref()),
                )
            }
            other => other.into_client_error(),
        }
    }

    /// Mapping for `GET /me/proxy-profile`: the backend answers 404 when the
    /// team has no active subscription or no usable node.
    fn into_profile_error(self) -> ClientError {
        match self {
            Self::Status {
                request,
                status,
                code,
                ..
            } if status == StatusCode::NOT_FOUND => ClientError::failed(
                ErrorCode::NoSubscription,
                status_detail(&request, status, code.as_deref()),
            ),
            other => match other.into_client_error() {
                // Any other rejection of this endpoint is a profile problem.
                ClientError::Failed {
                    code: ErrorCode::RequestRejected,
                    detail,
                } => ClientError::failed(ErrorCode::ProfileFetchFailed, detail),
                mapped => mapped,
            },
        }
    }
}

/// The client error code of a [`SubscriptionProblem`].
fn subscription_code(status: StatusCode, code: Option<&str>) -> Option<ErrorCode> {
    subscription_problem(status, code).map(|problem| match problem {
        SubscriptionProblem::TeamDisabled => ErrorCode::TeamDisabled,
        SubscriptionProblem::Expired => ErrorCode::SubscriptionExpired,
        SubscriptionProblem::NoSubscription => ErrorCode::NoSubscription,
    })
}

// ---------------------------------------------------------------------------
// Proxy profile (UI summary only)
// ---------------------------------------------------------------------------

/// What the UI needs from a proxy profile ([`ppvpn_account::profile`]), as
/// the UniFFI records. The raw bytes stay authoritative.
#[derive(Clone, Debug)]
pub(crate) struct ParsedProfile {
    pub(crate) summary: ProfileSummary,
    pub(crate) nodes: Vec<Node>,
    pub(crate) default_node_id: Option<String>,
}

impl From<ProfileView> for ParsedProfile {
    fn from(view: ProfileView) -> Self {
        let nodes: Vec<Node> = view.nodes.into_iter().map(Node::from).collect();
        Self {
            summary: ProfileSummary {
                revision: view.revision,
                expires_at: view.expires_at,
                node_count: u32::try_from(nodes.len()).unwrap_or(u32::MAX),
            },
            nodes,
            default_node_id: view.default_node_id,
        }
    }
}

impl From<NodeView> for Node {
    fn from(node: NodeView) -> Self {
        let (name, flag_country) = split_flag(node.name);
        Self {
            id: node.id,
            name,
            entry_key: node.entry_key,
            entry_label: node.entry_label,
            exit_region: node.exit_region,
            exit_country_code: node.exit_country_code.or(flag_country),
            udp: node.udp,
            replicas: node.replicas.into_iter().map(Replica::from).collect(),
        }
    }
}

impl From<ReplicaView> for Replica {
    fn from(replica: ReplicaView) -> Self {
        Self {
            endpoint_key: replica.endpoint_key,
            replica_ordinal: replica.replica_ordinal,
            protocol: replica.protocol,
            label: replica.label,
        }
    }
}

/// Parse the UI summary out of raw proxy-profile bytes. Fails with
/// `ProfileInvalid` when the document is not JSON or not the unified
/// schema-1 shape (detail `PROFILE_SHAPE: …`).
pub(crate) fn parse_profile(raw: &[u8]) -> Result<ParsedProfile, ClientError> {
    ppvpn_account::profile::parse_profile(raw)
        .map(ParsedProfile::from)
        .map_err(|error| ClientError::failed(ErrorCode::ProfileInvalid, error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn team_status_marks_disabled_teams_inactive() {
        let entries: Vec<TeamEntry> = serde_json::from_str(
            r#"[{"id":"a","name":"A","status":"active"},{"id":"b","name":"B","status":"disabled"},{"id":"c","name":"C"},{"id":"d","name":"D","status":"deleted"}]"#,
        )
        .unwrap();
        let active: Vec<bool> = entries.iter().map(|e| e.to_team().active).collect();
        assert_eq!(active, [true, false, true, false]);
    }

    const FIXTURE: &[u8] = include_bytes!("../../account/tests/fixtures/proxy-profile.json");

    #[test]
    fn fixture_profile_becomes_the_uniffi_summary() {
        let parsed = parse_profile(FIXTURE).expect("fixture parses");
        assert_eq!(
            parsed.summary.revision,
            "e4a97a896762108f6b8e05e95e2ce1581b8af4b35044c2898a567a32627d5876"
        );
        assert_eq!(parsed.summary.expires_at, "2099-01-01T00:00:00Z");
        assert_eq!(parsed.summary.node_count, 3);
        assert_eq!(
            parsed.default_node_id.as_deref(),
            Some("7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11-101")
        );
        let first = &parsed.nodes[0];
        assert_eq!(first.id, "7d7c34e4-7f38-4c0c-9a53-1f0c0c9e2b11-101");
        // The fixture's name starts with the flag ("🇺🇸洛杉矶家宽-…").
        assert_eq!(first.name, "洛杉矶家宽-203.0.113.10");
        assert_eq!(first.entry_key, "cn-optimized");
        assert_eq!(first.entry_label.as_deref(), Some("三网优化"));
        assert_eq!(first.exit_region.as_deref(), Some("洛杉矶"));
        assert_eq!(first.exit_country_code.as_deref(), Some("US"));
        assert!(first.udp);
        let replicas: Vec<_> = first
            .replicas
            .iter()
            .map(|r| {
                (
                    r.endpoint_key.as_str(),
                    r.replica_ordinal,
                    r.protocol.as_str(),
                )
            })
            .collect();
        assert_eq!(replicas, [("11", 0, "shadowsocks"), ("12", 1, "vless")]);

        assert_eq!(parsed.nodes[1].entry_key, "standard");
        assert_eq!(parsed.nodes[1].name, "洛杉矶家宽-203.0.113.10-standard");
        assert_eq!(parsed.nodes[2].name, "东京住宅-203.0.113.20");
        assert_eq!(parsed.nodes[2].exit_region.as_deref(), Some("东京"));
    }

    #[test]
    fn split_flag_takes_one_leading_flag_off_the_name() {
        let split = |name: &str| split_flag(name.to_string());
        let us = Some("US".to_string());
        assert_eq!(
            split("🇺🇸洛杉矶家宽-203.0.113.10"),
            ("洛杉矶家宽-203.0.113.10".to_string(), us.clone())
        );
        // The whitespace after the flag goes with it.
        assert_eq!(split("🇺🇸 LA 01"), ("LA 01".to_string(), us.clone()));
        // No flag, a flag later in the name, a lone indicator: unchanged.
        assert_eq!(
            split("东京-203.0.113.20"),
            ("东京-203.0.113.20".to_string(), None)
        );
        assert_eq!(split("LA 🇺🇸 01"), ("LA 🇺🇸 01".to_string(), None));
        assert_eq!(split("🇺 LA"), ("🇺 LA".to_string(), None));
        assert_eq!(split("🇺"), ("🇺".to_string(), None));
        assert_eq!(split(""), (String::new(), None));
        // Only the first pair goes.
        assert_eq!(split("🇺🇸🇯🇵 x"), ("🇯🇵 x".to_string(), us.clone()));
        // A name that is nothing but the flag keeps it.
        assert_eq!(split("🇺🇸"), ("🇺🇸".to_string(), us.clone()));
        assert_eq!(split("🇺🇸  "), ("🇺🇸  ".to_string(), us));
    }

    #[test]
    fn a_flag_in_the_name_fills_a_missing_country_code_only() {
        let profile = |name: &str, exit: &str| {
            format!(
                r#"{{"schema_version":1,"revision":"r","expires_at":"2099-01-01T00:00:00Z","nodes":[{{"id":"a-1","name":"{name}","entry_key":"standard","exit":{exit},"capabilities":{{"tcp":true,"udp":true}},"ingresses":[{{"endpoint_key":"a","replica_ordinal":0,"protocol":"vless"}}]}}]}}"#
            )
        };
        let node = |raw: String| parse_profile(raw.as_bytes()).unwrap().nodes.remove(0);

        let filled = node(profile("🇯🇵东京住宅-203.0.113.20", r#"{"region":"东京"}"#));
        assert_eq!(filled.name, "东京住宅-203.0.113.20");
        assert_eq!(filled.exit_country_code.as_deref(), Some("JP"));

        // The server's country code wins over the flag.
        let kept = node(profile(
            "🇯🇵东京",
            r#"{"region":"东京","country_code":"US"}"#,
        ));
        assert_eq!(kept.name, "东京");
        assert_eq!(kept.exit_country_code.as_deref(), Some("US"));
    }

    #[test]
    fn parses_optional_fields_and_sorts_replicas() {
        let raw = br#"{
            "schema_version": 1, "revision": "r1", "expires_at": "2030-01-01T00:00:00Z",
            "future_field": {"x": 1},
            "nodes": [{
                "id": "n1", "name": "Node", "entry_key": "standard",
                "entry_label": "Standard", "unknown": true,
                "exit": {"region": "Tokyo", "country_code": "JP"},
                "capabilities": {"tcp": true, "udp": false},
                "ingresses": [
                    {"endpoint_key": "b", "replica_ordinal": 2, "protocol": "anytls", "label": "Backup B"},
                    {"endpoint_key": "a", "replica_ordinal": 0, "protocol": "vless"}
                ]
            }]
        }"#;
        let parsed = parse_profile(raw).unwrap();
        let node = &parsed.nodes[0];
        assert_eq!(node.entry_label.as_deref(), Some("Standard"));
        assert_eq!(node.exit_country_code.as_deref(), Some("JP"));
        assert!(!node.udp);
        assert_eq!(node.replicas[0].endpoint_key, "a");
        assert_eq!(node.replicas[1].endpoint_key, "b");
        assert_eq!(node.replicas[1].label.as_deref(), Some("Backup B"));
        assert_eq!(node.replicas[0].label, None);
        assert_eq!(parsed.default_node_id, None);
    }

    #[test]
    fn rejects_the_old_profile_format() {
        let old = r#"{"schema_version":1,"revision":"r","expires_at":"2099-01-01T00:00:00Z","nodes":[{"id":"a-1","name":"x","exit":{"region":"东京"}}]}"#;
        match parse_profile(old.as_bytes()).unwrap_err() {
            ClientError::Failed {
                code: ErrorCode::ProfileInvalid,
                detail,
            } => assert_eq!(detail, "PROFILE_SHAPE: nodes[0].entry_key missing"),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn expired_problem_carries_the_end_date() {
        let error = ApiError::Status {
            request: "GET /api/v1/me/proxy-profile".into(),
            status: StatusCode::BAD_REQUEST,
            code: Some("SUBSCRIPTION_EXPIRED".into()),
            expired_at: Some("2026-09-01T00:00:00Z".into()),
        }
        .into_profile_error();
        let ClientError::Failed { code, detail } = error else {
            panic!("unexpected {error:?}")
        };
        assert_eq!(code, ErrorCode::SubscriptionExpired);
        assert_eq!(
            expired_at_from_detail(&detail).as_deref(),
            Some("2026-09-01T00:00:00Z")
        );
    }

    #[test]
    fn rejects_non_json_profile() {
        let error = parse_profile(b"<html>").unwrap_err();
        assert!(matches!(
            error,
            ClientError::Failed {
                code: ErrorCode::ProfileInvalid,
                ..
            }
        ));
    }

    #[test]
    fn maps_subscription_and_status_errors() {
        let code = |error: ClientError| match error {
            ClientError::Failed { code, .. } => code,
            other => panic!("unexpected {other:?}"),
        };
        let status = |status: u16, problem: Option<&str>| ApiError::Status {
            expired_at: None,
            request: "GET /api/v1/me/proxy-profile".to_string(),
            status: StatusCode::from_u16(status).unwrap(),
            code: problem.map(str::to_string),
        };
        assert_eq!(
            code(status(404, Some("NOT_FOUND")).into_profile_error()),
            ErrorCode::NoSubscription
        );
        assert_eq!(
            code(status(402, None).into_client_error()),
            ErrorCode::NoSubscription
        );
        assert_eq!(
            code(status(400, Some("SUBSCRIPTION_EXPIRED")).into_client_error()),
            ErrorCode::SubscriptionExpired
        );
        assert_eq!(
            code(status(401, None).into_client_error()),
            ErrorCode::AuthSessionInvalid
        );
        assert_eq!(
            code(status(403, None).into_auth_error()),
            ErrorCode::AuthSessionInvalid
        );
        assert_eq!(
            code(status(503, None).into_profile_error()),
            ErrorCode::ServerUnavailable
        );
        assert_eq!(
            code(status(400, Some("BAD")).into_profile_error()),
            ErrorCode::ProfileFetchFailed
        );
        assert_eq!(
            code(status(409, Some("CONFLICT")).into_client_error()),
            ErrorCode::RequestRejected
        );
        assert!(status(403, None).is_terminal_credential());
        assert!(!status(403, Some("403012")).is_terminal_credential());
        assert_eq!(
            code(status(404, Some(PROBLEM_TEAM_DISSOLVED)).into_client_error()),
            ErrorCode::TeamDisabled
        );
        assert_eq!(
            code(status(403, Some("403012")).into_client_error()),
            ErrorCode::TeamDisabled
        );
        assert_eq!(
            code(status(403, Some("403012")).into_auth_error()),
            ErrorCode::TeamDisabled
        );
        assert_eq!(
            code(status(403, Some("403012")).into_profile_error()),
            ErrorCode::TeamDisabled
        );
        match status(403, Some("403012")).into_client_error() {
            ClientError::Failed { detail, .. } => {
                assert_eq!(detail, "GET /api/v1/me/proxy-profile -> HTTP 403 403012")
            }
            other => panic!("unexpected {other:?}"),
        }
        assert!(!status(500, None).is_terminal_credential());
    }

    #[test]
    fn endpoint_keeps_base_prefix_and_requires_https() {
        let api = client_api_with_test_mode("https://api.example.com/prefix/", false);
        assert_eq!(
            api.endpoint(PATH_USERS_ME).unwrap().as_str(),
            "https://api.example.com/prefix/api/v1/users/me"
        );
        assert!(client_api_with_test_mode("http://api.example.com", false)
            .endpoint(PATH_USERS_ME)
            .is_err());
        assert!(
            client_api_with_test_mode("http://127.0.0.1:8080", cfg!(not(debug_assertions)))
                .endpoint(PATH_USERS_ME)
                .is_ok()
        );
    }
}
