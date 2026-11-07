//! Peer-bound authentication using the EXISTING service Request/Response HMAC
//! envelope. Capture DTOs come exclusively from ppvpn-core-wire. There is no
//! second canonical form or new enabled wire protocol in this module.
use crate::protocol::{Request, MESSAGE_EXPIRY_SECONDS};
use std::collections::HashMap;
use std::time::{Duration, Instant};
const MAX_REPLAYS: usize = 4096;

pub use crate::peer_identity::Peer;
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity,
    Expired,
    WrongPeer,
    WrongGeneration,
    InvalidSignature,
    StaleRequest,
    Replay,
    Capacity,
}
/// No Debug/Serialize: this object contains an authentication key. Peer fields
/// must originate in a verified OS audit token, never the request payload.
pub struct Session {
    peer: Peer,
    key: [u8; 32],
    token: String,
    generation: u64,
    expires: Instant,
    seen: HashMap<String, u64>,
}
impl Session {
    pub fn new(
        peer: Peer,
        key: [u8; 32],
        token: String,
        generation: u64,
        now: Instant,
        lifetime: Duration,
    ) -> Result<Self, Error> {
        if peer.uid == 0
            || peer.pid == 0
            || peer.pid_version == 0
            || generation == 0
            || lifetime.is_zero()
            || token.len() < 16
            || token.len() > 128
        {
            return Err(Error::InvalidIdentity);
        }
        let expires = now.checked_add(lifetime).ok_or(Error::InvalidIdentity)?;
        Ok(Self {
            peer,
            key,
            token,
            generation,
            expires,
            seen: HashMap::new(),
        })
    }
    /// generation is the acceptor's current service generation, not a wire field.
    /// Keys/tokens are freshly generated per handshake and never persisted.
    pub fn verify(
        &mut self,
        peer: Peer,
        request: &Request,
        generation: u64,
        now: Instant,
        wall: u64,
    ) -> Result<(), Error> {
        if now >= self.expires {
            return Err(Error::Expired);
        }
        if peer != self.peer {
            return Err(Error::WrongPeer);
        }
        if generation != self.generation {
            return Err(Error::WrongGeneration);
        }
        if request.id.is_empty()
            || request.id.len() > 128
            || request.timestamp > wall
            || wall - request.timestamp > MESSAGE_EXPIRY_SECONDS
        {
            return Err(Error::StaleRequest);
        }
        if request.auth_token.as_deref() != Some(&self.token)
            || !request.verify_with(&self.key).unwrap_or(false)
        {
            return Err(Error::InvalidSignature);
        }
        self.seen
            .retain(|_, recorded| wall.saturating_sub(*recorded) <= MESSAGE_EXPIRY_SECONDS);
        if self.seen.contains_key(&request.id) {
            return Err(Error::Replay);
        }
        if self.seen.len() >= MAX_REPLAYS {
            return Err(Error::Capacity);
        }
        self.seen.insert(request.id.clone(), wall);
        Ok(())
    }
    pub fn sign_response(
        &self,
        id: String,
        success: bool,
        data: Option<serde_json::Value>,
        error: Option<String>,
    ) -> anyhow::Result<crate::protocol::Response> {
        crate::protocol::Response::signed_with(id, success, data, error, &self.key)
    }
}
impl Drop for Session {
    fn drop(&mut self) {
        for byte in &mut self.key {
            unsafe { std::ptr::write_volatile(byte, 0) };
        }
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    use crate::protocol::{now_epoch, Command};
    fn peer() -> Peer {
        Peer {
            uid: 501,
            pid: 25,
            pid_version: 4,
            audit_session: 11,
        }
    }
    fn session(now: Instant) -> Session {
        Session::new(
            peer(),
            [7; 32],
            "session-token-1234".into(),
            1,
            now,
            Duration::from_secs(60),
        )
        .unwrap()
    }
    fn request(id: &str) -> Request {
        Request::signed(
            id.into(),
            Command::CoreApi,
            serde_json::json!({"path":"/v1/get-status"}),
            Some("session-token-1234".into()),
            &[7; 32],
        )
        .unwrap()
    }
    #[test]
    fn binds_peer_and_service_generation_without_consuming_bad_requests() {
        let now = Instant::now();
        let mut session = session(now);
        let req = request("a");
        for field in 0..4 {
            let mut wrong = peer();
            match field {
                0 => wrong.uid += 1,
                1 => wrong.pid += 1,
                2 => wrong.pid_version += 1,
                _ => wrong.audit_session += 1,
            };
            assert_eq!(
                session.verify(wrong, &req, 1, now, now_epoch()),
                Err(Error::WrongPeer)
            );
        }
        assert_eq!(
            session.verify(peer(), &req, 2, now, now_epoch()),
            Err(Error::WrongGeneration)
        );
        session.verify(peer(), &req, 1, now, now_epoch()).unwrap();
        assert_eq!(
            session.verify(
                peer(),
                &request("b"),
                1,
                now + Duration::from_secs(60),
                now_epoch()
            ),
            Err(Error::Expired)
        );
    }
    #[test]
    fn shared_envelope_binds_all_fields_and_rejects_replay() {
        let now = Instant::now();
        let mut session = session(now);
        let req = request("a");
        for field in 0..4 {
            let mut changed = req.clone();
            match field {
                0 => changed.id = "b".into(),
                1 => changed.command = Command::Disconnect,
                2 => changed.payload = serde_json::json!({"changed":true}),
                _ => changed.auth_token = Some("different-session".into()),
            };
            assert_eq!(
                session.verify(peer(), &changed, 1, now, now_epoch()),
                Err(Error::InvalidSignature)
            );
        }
        session.verify(peer(), &req, 1, now, now_epoch()).unwrap();
        assert_eq!(
            session.verify(peer(), &req, 1, now, now_epoch()),
            Err(Error::Replay)
        );
        let response = session
            .sign_response("a".into(), true, Some(serde_json::json!({})), None)
            .unwrap();
        assert!(response.verify_with(&[7; 32]).unwrap());
    }
    #[test]
    fn stale_future_and_capacity_fail_closed() {
        let now = Instant::now();
        let mut session = session(now);
        let wall = now_epoch();
        for timestamp in [wall + 1, wall - 31] {
            let mut req = request("a");
            req.timestamp = timestamp;
            assert_eq!(
                session.verify(peer(), &req, 1, now, wall),
                Err(Error::StaleRequest)
            );
        }
        for i in 0..MAX_REPLAYS {
            session
                .verify(peer(), &request(&format!("id-{i}")), 1, now, now_epoch())
                .unwrap();
        }
        assert_eq!(
            session.verify(peer(), &request("extra"), 1, now, now_epoch()),
            Err(Error::Capacity)
        );
        assert_eq!(
            session.verify(peer(), &request("id-0"), 1, now, now_epoch()),
            Err(Error::Replay)
        );
    }
}
