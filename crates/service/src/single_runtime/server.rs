//! Bounded framed IPC over a private socket, using the existing service
//! envelope. The injected verifier MUST check OS peer, signature and installed
//! executable before handshake bytes are read. No plaintext-key debug bypass.
use super::{
    auth::Peer,
    endpoint::Endpoint,
    gateway::{Forwarder, Gateway},
};
use crate::protocol::{Request, Response};
use anyhow::{anyhow, Result};
use std::{
    future::Future,
    io::{Read, Write},
    os::unix::net::UnixStream,
    sync::Arc,
    time::Duration,
};
const MAX_FRAME: usize = 4 * 1024 * 1024;
const MAX_CONNECTIONS: usize = 16;
pub type VerifyPeer = Arc<dyn Fn(&UnixStream) -> Result<Peer> + Send + Sync>;
fn read(stream: &mut UnixStream) -> Result<Request> {
    let mut length = [0; 4];
    stream.read_exact(&mut length)?;
    let length = u32::from_be_bytes(length) as usize;
    if length == 0 || length > MAX_FRAME {
        return Err(anyhow!("IPC_FRAME_LENGTH_INVALID"));
    }
    let mut body = vec![0; length];
    stream.read_exact(&mut body)?;
    Ok(serde_json::from_slice(&body)?)
}
fn write(stream: &mut UnixStream, reply: &Response) -> Result<()> {
    let data = serde_json::to_vec(reply)?;
    if data.len() > MAX_FRAME {
        return Err(anyhow!("IPC_RESPONSE_TOO_LARGE"));
    }
    stream.write_all(&(data.len() as u32).to_be_bytes())?;
    stream.write_all(&data)?;
    Ok(())
}
pub async fn serve<B: Forwarder>(
    endpoint: Endpoint,
    gateway: Arc<Gateway<B>>,
    verify: VerifyPeer,
    stop: impl Future<Output = ()>,
) -> Result<()> {
    endpoint.listener.set_nonblocking(true)?;
    let listener = tokio::net::UnixListener::from_std(endpoint.listener.try_clone()?)?;
    let capacity = Arc::new(tokio::sync::Semaphore::new(MAX_CONNECTIONS));
    let mut workers = tokio::task::JoinSet::new();
    tokio::pin!(stop);
    loop {
        let socket = tokio::select! { _=&mut stop=>break, result=listener.accept()=>result?.0 };
        let Ok(permit) = capacity.clone().try_acquire_owned() else {
            drop(socket);
            continue;
        };
        let mut stream = socket.into_std()?;
        stream.set_nonblocking(false)?;
        stream.set_read_timeout(Some(Duration::from_secs(10)))?;
        stream.set_write_timeout(Some(Duration::from_secs(10)))?;
        let gateway = gateway.clone();
        let verify = verify.clone();
        let runtime = tokio::runtime::Handle::current();
        workers.spawn_blocking(move || {
            let _permit = permit;
            let mut execute = || -> Result<()> {
                let peer = verify(&stream)?;
                let request = read(&mut stream)?;
                let reply = runtime.block_on(async {
                    tokio::time::timeout(Duration::from_secs(45), gateway.handle(peer, request))
                        .await
                })??;
                write(&mut stream, &reply)
            };
            // Do not log request payloads, auth keys, credentials or profile data.
            let _ = execute();
        });
        while workers.try_join_next().is_some() {}
    }
    drop(listener);
    // No new requests after shutdown. Existing socket reads have a 10s timeout;
    // admitted backend calls are bounded at 45s. The owning agent must await
    // drain before shutting its engine down. Capture remains capability-gated.
    while workers.join_next().await.is_some() {}
    drop(endpoint);
    Ok(())
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        protocol::{self, Command, HandshakeResponse},
        single_runtime::gateway::Forwarder,
    };
    use async_trait::async_trait;
    use serde_json::{json, Value};
    struct Fake;
    #[async_trait]
    impl Forwarder for Fake {
        fn identity(&self) -> ppvpn_core_wire::ForwarderIdentity {
            ppvpn_core_wire::ForwarderIdentity {
                instance: "one-runtime".into(),
                generation: 1,
            }
        }
        async fn call(&self, _: &str, _: Value) -> Result<Value, ppvpn_engine_host::ApiError> {
            Ok(json!({"same_runtime":true}))
        }
    }
    fn peer() -> Peer {
        Peer {
            uid: unsafe { libc::geteuid() },
            pid: std::process::id(),
            pid_version: 1,
            audit_session: 1,
        }
    }
    fn exchange(path: &std::path::Path, request: &Request) -> Result<Response> {
        let mut stream = UnixStream::connect(path)?;
        stream.set_read_timeout(Some(Duration::from_secs(2)))?;
        let bytes = serde_json::to_vec(request)?;
        stream.write_all(&(bytes.len() as u32).to_be_bytes())?;
        stream.write_all(&bytes)?;
        let mut size = [0; 4];
        stream.read_exact(&mut size)?;
        let size = u32::from_be_bytes(size) as usize;
        if size > MAX_FRAME {
            return Err(anyhow!("bad frame"));
        }
        let mut body = vec![0; size];
        stream.read_exact(&mut body)?;
        Ok(serde_json::from_slice(&body)?)
    }
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn socket_reconnect_keeps_forwarder_and_enforces_hmac_and_roles() {
        let dir = std::env::temp_dir().canonicalize().unwrap().join(format!(
            "pv-{}",
            &uuid::Uuid::new_v4().simple().to_string()[..8]
        ));
        std::fs::create_dir(&dir).unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).unwrap();
        let endpoint = Endpoint::bind(&dir, peer().uid).unwrap();
        let path = endpoint.path().to_path_buf();
        let gateway = Gateway::new(Arc::new(Fake), peer().uid).unwrap();
        let identity = gateway.identity();
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let server = tokio::spawn(serve(
            endpoint,
            gateway.clone(),
            Arc::new(|_| Ok(peer())),
            async {
                let _ = stopped.await;
            },
        ));
        let test_path = path.clone();
        tokio::task::spawn_blocking(move || {
            let hello = Request {
                id: "hello".into(),
                timestamp: protocol::now_epoch(),
                command: Command::Handshake,
                payload: json!({"client_nonce":"01234567890123456789012345678901"}),
                auth_token: None,
                signature: String::new(),
            };
            let info: HandshakeResponse =
                serde_json::from_value(exchange(&test_path, &hello).unwrap().data.unwrap())
                    .unwrap();
            let key = hex::decode(info.auth_key).unwrap();
            for index in 0..3 {
                let request = Request::signed(
                    format!("status-{index}"),
                    Command::GetStatus,
                    json!({}),
                    Some(info.auth_token.clone()),
                    &key,
                )
                .unwrap();
                let reply = exchange(&test_path, &request).unwrap();
                assert!(reply.verify_with(&key).unwrap());
                assert_eq!(reply.data.unwrap()["session_id"], "one-runtime");
            }
            let req = Request::signed(
                "capture".into(),
                Command::Connect,
                json!({}),
                Some(info.auth_token.clone()),
                &key,
            )
            .unwrap();
            assert!(!exchange(&test_path, &req).unwrap().success);
            let mut bad = Request::signed(
                "bad".into(),
                Command::GetStatus,
                json!({}),
                Some(info.auth_token),
                &key,
            )
            .unwrap();
            bad.signature = "00".repeat(32);
            assert!(exchange(&test_path, &bad).is_err());
        })
        .await
        .unwrap();
        assert_eq!(gateway.identity(), identity);
        stop.send(()).unwrap();
        server.await.unwrap().unwrap();
        assert!(!path.exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
