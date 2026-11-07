//! Apple-only authenticated requests. Local owner checks fence stale bridge
//! work; the backend independently validates the active JWT sid and CAS.
use crate::api::ApiErrorExt;
use crate::{Client, ClientError};

impl Client {
    pub async fn apple_push_http(
        &self,
        owner: String,
        operation: String,
        body: String,
    ) -> Result<String, ClientError> {
        let session = self.current_session()?;
        if self.snapshot().account.as_ref().map(|a| a.id.as_str()) != Some(owner.as_str()) {
            return Err(ClientError::Cancelled);
        }
        let body =
            serde_json::from_str::<serde_json::Value>(&body).map_err(|_| ClientError::Cancelled)?;
        let response = self
            .bearer(
                session,
                |token| {
                    let body = body.clone();
                    let operation = operation.clone();
                    async move { self.auth.api().apple_push(&token, &operation, body).await }
                },
                |e| e.into_client_error(),
            )
            .await?;
        if !self.session_is(session)
            || self.snapshot().account.as_ref().map(|a| a.id.as_str()) != Some(owner.as_str())
        {
            return Err(ClientError::Cancelled);
        }
        Ok(response.to_string())
    }
}

impl Client {
    pub async fn apple_push_message(
        &self,
        owner: String,
        delivery_id: String,
        message_id: Option<u64>,
    ) -> Result<crate::InboxMessage, ClientError> {
        let (operation, body) = if delivery_id.starts_with("push:") {
            ("delivery", serde_json::json!({"delivery_id": delivery_id}))
        } else if let Some(id) = message_id.filter(|id| *id > 0) {
            ("message", serde_json::json!({"message_id": id}))
        } else {
            return Err(ClientError::Cancelled);
        };
        let response = self
            .apple_push_http(owner, operation.to_string(), body.to_string())
            .await?;
        let response: serde_json::Value =
            serde_json::from_str(&response).map_err(|_| ClientError::Cancelled)?;
        if response["status"] != 200 {
            return Err(ClientError::Cancelled);
        }
        let message: crate::api::Message =
            serde_json::from_value(response["body"].clone()).map_err(|_| ClientError::Cancelled)?;
        Ok(crate::notifications::to_notification(
            message,
            &self.config.api_base,
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::auth::test_support::MemoryPlatform;
    use crate::cores::test_support::{FakeLauncher, NoService};
    use crate::test_backend::{serve, temp_dir, wait_until, AppleConfigGate, Backend};
    use crate::{ClientConfig, ClientListener, ClientSnapshot, ProbeResult, TrafficSample};
    use std::sync::Arc;
    struct Listener;
    impl ClientListener for Listener {
        fn on_snapshot(&self, _: ClientSnapshot) {}
        fn on_probe_result(&self, _: ProbeResult) {}
        fn on_traffic(&self, _: TrafficSample) {}
    }
    fn client(backend: Arc<Backend>) -> (Arc<Client>, String) {
        let dir = temp_dir("ppvpn-apple-session-test");
        let client = Client::with_parts(
            ClientConfig {
                api_base: serve(backend),
                data_dir: dir.clone(),
                log_dir: dir.clone(),
                platform: "ios".into(),
                app_version: "0.0.0-test".into(),
            },
            MemoryPlatform::with_blob(r#"{"version":1,"active_refresh":"jyr_old"}"#),
            Arc::new(Listener),
            Arc::new(NoService),
            FakeLauncher::new(),
            Arc::new(crate::sysproxy::tests::FakeWriter::default()),
        );
        wait_until(|| client.snapshot().account.is_some());
        (client, dir)
    }
    fn run<T>(future: impl std::future::Future<Output = T>) -> T {
        tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap()
            .block_on(future)
    }
    #[test]
    fn apple_push_uses_current_owner_and_no_request_after_logout() {
        let backend = Backend::new();
        let (client, dir) = client(backend.clone());
        assert!(
            run(client.apple_push_http("foreign".into(), "config".into(), "{}".into())).is_err()
        );
        assert!(backend.requests_matching("/devices/apple").is_empty());
        let response =
            run(client.apple_push_http("u7".into(), "config".into(), "{}".into())).unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&response).unwrap()["status"],
            200
        );
        run(client.logout()).unwrap();
        assert!(run(client.apple_push_http("u7".into(), "config".into(), "{}".into())).is_err());
        assert_eq!(backend.requests_matching("/devices/apple").len(), 1);
        client.shutdown();
        drop(client);
        std::fs::remove_dir_all(dir).unwrap();
    }
    #[test]
    fn apple_push_discards_actual_http_response_after_session_revocation() {
        let backend = Backend::new();
        let (client, dir) = client(backend.clone());
        let (entered, started) = std::sync::mpsc::channel();
        let (release, released) = std::sync::mpsc::channel();
        *backend.apple_config_gate.lock().unwrap() = Some(AppleConfigGate {
            entered,
            release: released,
        });
        let pending = client.clone();
        let task = std::thread::spawn(move || {
            run(pending.apple_push_http("u7".into(), "config".into(), "{}".into()))
        });
        started
            .recv_timeout(std::time::Duration::from_secs(5))
            .unwrap();
        run(client.logout()).unwrap();
        release.send(()).unwrap();
        assert!(matches!(task.join().unwrap(), Err(ClientError::Cancelled)));
        client.shutdown();
        drop(client);
        std::fs::remove_dir_all(dir).unwrap();
    }
}
