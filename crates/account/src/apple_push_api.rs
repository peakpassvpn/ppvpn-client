//! Narrow Apple transport, pinned to the backend candidate snapshot in
//! docs/mobile/apple-push-contract.json. No arbitrary URL or bearer export.
use super::*;
use reqwest::Method;
use serde_json::json;

fn invalid() -> ApiError {
    ApiError::Config("invalid Apple push request")
}
fn uuid(body: &Value, key: &str) -> Result<String, ApiError> {
    uuid::Uuid::parse_str(body[key].as_str().ok_or_else(invalid)?)
        .map(|v| v.to_string())
        .map_err(|_| invalid())
}
fn fields(body: &Value, required: &[&str], optional: &[&str]) -> Result<(), ApiError> {
    let map = body.as_object().ok_or_else(invalid)?;
    if required.iter().any(|k| !map.contains_key(*k))
        || map
            .keys()
            .any(|k| !required.contains(&k.as_str()) && !optional.contains(&k.as_str()))
    {
        return Err(invalid());
    }
    Ok(())
}

impl Api {
    /// Returns an internal status/body envelope, not a wire protocol wrapper.
    /// Failure bodies are reduced to an allowlisted code; no server detail is
    /// logged, since a rejecting server might echo a proof or device token.
    pub async fn apple_push(
        &self,
        bearer: &str,
        operation: &str,
        mut body: Value,
    ) -> Result<Value, ApiError> {
        let root = "/api/v1/devices/apple";
        let (method, path) = match operation {
            "config" => {
                fields(&body, &[], &[])?;
                (Method::GET, format!("{root}/config"))
            }
            "resolve" => {
                fields(
                    &body,
                    &[
                        "installation_id",
                        "installation_secret",
                        "platform",
                        "bundle_id",
                        "environment",
                    ],
                    &[],
                )?;
                uuid(&body, "installation_id")?;
                (Method::POST, format!("{root}/installations"))
            }
            "bind" | "register" | "unbind" => {
                let id = uuid(&body, "installation_id")?;
                let (method, path) = match operation {
                    "bind" => {
                        fields(
                            &body,
                            &[
                                "installation_id",
                                "installation_secret",
                                "expected_binding_id",
                                "request_id",
                            ],
                            &[],
                        )?;
                        uuid(&body, "request_id")?;
                        (Method::POST, format!("{root}/installations/{id}/bindings"))
                    }
                    "register" => {
                        fields(
                            &body,
                            &[
                                "installation_id",
                                "installation_secret",
                                "binding_id",
                                "expected_registration_id",
                                "request_id",
                                "token",
                            ],
                            &["app_version", "locale", "timezone"],
                        )?;
                        uuid(&body, "request_id")?;
                        uuid(&body, "binding_id")?;
                        (
                            Method::POST,
                            format!("{root}/installations/{id}/registrations"),
                        )
                    }
                    _ => {
                        fields(
                            &body,
                            &["installation_id", "installation_secret", "binding_id"],
                            &[],
                        )?;
                        let binding = uuid(&body, "binding_id")?;
                        body.as_object_mut().unwrap().remove("binding_id");
                        (
                            Method::DELETE,
                            format!("{root}/installations/{id}/bindings/{binding}"),
                        )
                    }
                };
                body.as_object_mut().unwrap().remove("installation_id");
                (method, path)
            }
            "delivery" => {
                fields(&body, &["delivery_id"], &[])?;
                let id = body["delivery_id"].as_str().ok_or_else(invalid)?;
                let suffix = id.strip_prefix("push:").ok_or_else(invalid)?;
                if id.len() > 32
                    || suffix.starts_with('0')
                    || suffix.is_empty()
                    || !suffix.bytes().all(|b| b.is_ascii_digit())
                {
                    return Err(invalid());
                }
                (Method::GET, format!("/api/v1/messages/deliveries/{id}"))
            }
            "message" => {
                fields(&body, &["message_id"], &[])?;
                let id = body["message_id"]
                    .as_u64()
                    .filter(|id| *id > 0)
                    .ok_or_else(invalid)?;
                (Method::GET, format!("/api/v1/messages/{id}"))
            }
            "updates" => {
                fields(&body, &["after_id"], &[])?;
                let after = body["after_id"].as_u64().ok_or_else(invalid)?;
                let mut url = self.endpoint("/api/v1/messages/updates")?;
                url.query_pairs_mut()
                    .append_pair("after_id", &after.to_string())
                    .append_pair("limit", "100");
                return self
                    .apple_send(self.http.get(url).bearer_auth(bearer))
                    .await;
            }
            _ => return Err(invalid()),
        };
        let mut request = self
            .http
            .request(method.clone(), self.endpoint(&path)?)
            .bearer_auth(bearer);
        if method != Method::GET {
            request = request.json(&body);
        }
        self.apple_send(request).await
    }

    async fn apple_send(&self, request: reqwest::RequestBuilder) -> Result<Value, ApiError> {
        let response = self.send(request).await.map_err(ApiError::Transport)?;
        let status = response.status();
        let value = if status == StatusCode::NO_CONTENT {
            Value::Null
        } else {
            response
                .json::<Value>()
                .await
                .map_err(|_| ApiError::Decode("invalid Apple push response"))?
        };
        let body = if status.is_success() {
            value
        } else {
            let code = value["code"].as_str().filter(|v| {
                matches!(
                    *v,
                    "UNAUTHORIZED"
                        | "DEVICE_INSTALLATION_PROOF_INVALID"
                        | "DEVICE_SESSION_REQUIRED"
                        | "DEVICE_BINDING_CLOSED"
                        | "DEVICE_BINDING_CONFLICT"
                        | "DEVICE_REGISTRATION_CONFLICT"
                        | "DEVICE_TOKEN_OWNERSHIP_CONFLICT"
                        | "DEVICE_REQUEST_CONFLICT"
                        | "DEVICE_INSTALLATION_SCOPE_CONFLICT"
                        | "DEVICE_APPLE_INPUT_INVALID"
                        | "APPLE_PUSH_NOT_CONFIGURED"
                )
            });
            json!({"code": code})
        };
        Ok(json!({"status": status.as_u16(), "body": body}))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Read, Write};
    use std::net::TcpListener;
    use std::sync::{Arc, Mutex};
    const ID: &str = "00000000-0000-4000-8000-000000000001";
    const BINDING: &str = "00000000-0000-4000-8000-000000000002";
    fn mock(
        replies: Vec<(u16, Value)>,
    ) -> (Api, Arc<Mutex<Vec<String>>>, std::thread::JoinHandle<()>) {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let api = Api::new(ApiConfig::desktop(
            &format!("http://{}", server.local_addr().unwrap()),
            None,
            true,
        ));
        let requests = Arc::new(Mutex::new(Vec::new()));
        let captured = requests.clone();
        let thread = std::thread::spawn(move || {
            for (status, body) in replies {
                let (mut stream, _) = server.accept().unwrap();
                stream
                    .set_read_timeout(Some(Duration::from_secs(5)))
                    .unwrap();
                let mut raw = Vec::new();
                loop {
                    let mut chunk = [0; 4096];
                    let len = stream.read(&mut chunk).unwrap();
                    assert!(len > 0);
                    raw.extend_from_slice(&chunk[..len]);
                    if let Some(end) = raw.windows(4).position(|v| v == b"\r\n\r\n") {
                        let header = String::from_utf8_lossy(&raw[..end]).to_lowercase();
                        let length = header
                            .lines()
                            .find_map(|line| line.strip_prefix("content-length: "))
                            .and_then(|v| v.parse::<usize>().ok())
                            .unwrap_or(0);
                        if raw.len() >= end + 4 + length {
                            break;
                        }
                    }
                }
                captured
                    .lock()
                    .unwrap()
                    .push(String::from_utf8(raw).unwrap());
                let bytes = if status == 204 {
                    String::new()
                } else {
                    body.to_string()
                };
                write!(stream, "HTTP/1.1 {status} mock\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{bytes}", bytes.len()).unwrap();
            }
        });
        (api, requests, thread)
    }
    #[tokio::test]
    async fn apple_push_http_routes_bearer_cas_and_bare_json() {
        let (api, requests, server) = mock(vec![
            (200, json!({"configured":true})),
            (200, json!({"binding_id":BINDING})),
            (200, json!({"registration_id":ID})),
            (204, Value::Null),
            (200, json!({"id":7})),
            (200, json!({"items":[],"has_more":false})),
        ]);
        api.apple_push("fixture-bearer", "config", json!({}))
            .await
            .unwrap();
        let proof = "test-proof-not-a-live-secret";
        api.apple_push("fixture-bearer", "bind", json!({"installation_id":ID,"installation_secret":proof,"expected_binding_id":null,"request_id":ID})).await.unwrap();
        api.apple_push("fixture-bearer", "register", json!({"installation_id":ID,"installation_secret":proof,"binding_id":BINDING,"expected_registration_id":null,"request_id":ID,"token":"aabb"})).await.unwrap();
        assert_eq!(
            api.apple_push(
                "fixture-bearer",
                "unbind",
                json!({"installation_id":ID,"installation_secret":proof,"binding_id":BINDING})
            )
            .await
            .unwrap(),
            json!({"status":204,"body":null})
        );
        api.apple_push(
            "fixture-bearer",
            "delivery",
            json!({"delivery_id":"push:7"}),
        )
        .await
        .unwrap();
        api.apple_push("fixture-bearer", "updates", json!({"after_id":7}))
            .await
            .unwrap();
        server.join().unwrap();
        let raw = requests.lock().unwrap();
        assert!(raw.iter().all(|r| r
            .to_lowercase()
            .contains("authorization: bearer fixture-bearer")));
        assert!(raw[1].starts_with(&format!(
            "POST /api/v1/devices/apple/installations/{ID}/bindings "
        )));
        let body: Value = serde_json::from_str(raw[1].split("\r\n\r\n").nth(1).unwrap()).unwrap();
        assert!(body.get("data").is_none() && body.get("installation_id").is_none());
        assert_eq!(body["expected_binding_id"], Value::Null);
        assert!(raw[2].starts_with(&format!(
            "POST /api/v1/devices/apple/installations/{ID}/registrations "
        )));
        assert!(raw[3].starts_with(&format!(
            "DELETE /api/v1/devices/apple/installations/{ID}/bindings/{BINDING} "
        )));
        assert!(raw[4].starts_with("GET /api/v1/messages/deliveries/push:7 "));
        assert!(raw[5].starts_with("GET /api/v1/messages/updates?after_id=7&limit=100 "));
    }
    #[tokio::test]
    async fn apple_push_failure_redacts_problem_details_and_unknown_codes() {
        let (api, _, server) = mock(vec![
            (
                409,
                json!({"code":"DEVICE_BINDING_CONFLICT","message":"unsafe echoed proof","detail":"unsafe echoed token"}),
            ),
            (
                503,
                json!({"code":"arbitrary-sensitive-error","detail":"unsafe"}),
            ),
        ]);
        assert_eq!(
            api.apple_push("fixture", "config", json!({}))
                .await
                .unwrap(),
            json!({"status":409,"body":{"code":"DEVICE_BINDING_CONFLICT"}})
        );
        assert_eq!(
            api.apple_push("fixture", "config", json!({}))
                .await
                .unwrap(),
            json!({"status":503,"body":{"code":null}})
        );
        server.join().unwrap();
    }
    #[tokio::test]
    async fn apple_push_rejects_urls_identity_fields_and_payload_path_injection() {
        let api = Api::new(ApiConfig::desktop("https://example.invalid", None, false));
        for (operation, body) in [
            ("https://foreign.invalid", json!({})),
            ("config", json!({"account_id":"someone"})),
            ("delivery", json!({"delivery_id":"push:1/../../users/me"})),
            ("message", json!({"message_id":0})),
            ("unbind", json!({"installation_id":"../x"})),
        ] {
            assert!(matches!(
                api.apple_push("fixture", operation, body).await,
                Err(ApiError::Config(_))
            ));
        }
    }
}
