//! Candidate authentication at the user-runtime/broker boundary. Socket
//! acceptors must first verify code signature and installed image against an
//! OS audit token. This module does not replace the production IPC acceptor.
use hmac::{Hmac, Mac};
use sha2::Sha256;
use std::collections::HashMap;
use std::time::{Duration, Instant};

const MAX_REPLAYS: usize = 4096;
const REQUEST_WINDOW: u64 = 30;
const MAX_PAYLOAD: usize = 4 * 1024 * 1024;
const DOMAIN: &[u8] = b"ppvpn-capture-auth-v1/request";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Peer {
    pub uid: u32,
    pub pid: u32,
    pub pid_version: u32,
    pub audit_session: u32,
}
#[derive(Debug, PartialEq, Eq)]
pub enum Error {
    InvalidIdentity,
    Expired,
    WrongPeer,
    InvalidSignature,
    StaleRequest,
    Replay,
    Capacity,
}

/// Not a wire DTO: a borrowed view of the bounded request bytes. This candidate
/// canonical form must be reconciled with shared core-wire before integration.
pub struct Call<'a> {
    pub id: &'a str,
    pub timestamp: u64,
    pub command: &'a str,
    pub payload: &'a [u8],
    pub signature: &'a [u8],
}
/// No Debug/Serialize implementation: never print the session's secret key.
pub struct Session {
    peer: Peer,
    key: [u8; 32],
    generation: u64,
    expires: Instant,
    seen: HashMap<String, u64>,
}
fn field(mac: &mut Hmac<Sha256>, bytes: &[u8]) {
    mac.update(&(bytes.len() as u64).to_be_bytes());
    mac.update(bytes);
}
fn canonical(mac: &mut Hmac<Sha256>, generation: u64, call: &Call<'_>) {
    field(mac, DOMAIN);
    mac.update(&generation.to_be_bytes());
    field(mac, call.id.as_bytes());
    mac.update(&call.timestamp.to_be_bytes());
    field(mac, call.command.as_bytes());
    field(mac, call.payload);
}
impl Session {
    /// A fresh key must come from the OS CSPRNG for each authenticated handshake;
    /// never reuse it across services, peers or generations. Auth expiry is not
    /// a capture lease. Peer fields come from the verified OS audit token.
    pub fn new(
        peer: Peer,
        key: [u8; 32],
        generation: u64,
        now: Instant,
        lifetime: Duration,
    ) -> Result<Self, Error> {
        if peer.uid == 0
            || peer.pid == 0
            || peer.pid_version == 0
            || generation == 0
            || lifetime.is_zero()
        {
            return Err(Error::InvalidIdentity);
        }
        let expires = now.checked_add(lifetime).ok_or(Error::InvalidIdentity)?;
        Ok(Self {
            peer,
            key,
            generation,
            expires,
            seen: HashMap::new(),
        })
    }
    pub fn verify(
        &mut self,
        peer: Peer,
        call: Call<'_>,
        now: Instant,
        wall: u64,
    ) -> Result<(), Error> {
        if now >= self.expires {
            return Err(Error::Expired);
        }
        if peer != self.peer {
            return Err(Error::WrongPeer);
        }
        if call.id.is_empty()
            || call.id.len() > 128
            || call.command.is_empty()
            || call.command.len() > 128
            || call.payload.len() > MAX_PAYLOAD
            || call.timestamp > wall
            || wall - call.timestamp > REQUEST_WINDOW
        {
            return Err(Error::StaleRequest);
        }
        let mut mac = Hmac::<Sha256>::new_from_slice(&self.key).expect("fixed HMAC key size");
        canonical(&mut mac, self.generation, &call);
        mac.verify_slice(call.signature)
            .map_err(|_| Error::InvalidSignature)?;
        self.seen
            .retain(|_, recorded| wall.saturating_sub(*recorded) <= REQUEST_WINDOW);
        if self.seen.contains_key(call.id) {
            return Err(Error::Replay);
        }
        if self.seen.len() >= MAX_REPLAYS {
            return Err(Error::Capacity);
        }
        self.seen.insert(call.id.to_string(), wall);
        Ok(())
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
    fn peer() -> Peer {
        Peer {
            uid: 501,
            pid: 25,
            pid_version: 4,
            audit_session: 11,
        }
    }
    fn call(id: &str, timestamp: u64) -> Call<'_> {
        Call {
            id,
            timestamp,
            command: "attach",
            payload: b"settings",
            signature: &[],
        }
    }
    fn sign(generation: u64, call: &Call<'_>) -> Vec<u8> {
        let mut mac = Hmac::<Sha256>::new_from_slice(&[7; 32]).unwrap();
        canonical(&mut mac, generation, call);
        mac.finalize().into_bytes().to_vec()
    }
    fn check(
        session: &mut Session,
        peer: Peer,
        id: &str,
        time: u64,
        now: Instant,
        wall: u64,
    ) -> Result<(), Error> {
        let mut call = call(id, time);
        let signature = sign(1, &call);
        call.signature = &signature;
        session.verify(peer, call, now, wall)
    }
    #[test]
    fn binds_complete_peer_and_expiry_without_consuming_rejected_requests() {
        let now = Instant::now();
        let mut session = Session::new(peer(), [7; 32], 1, now, Duration::from_secs(60)).unwrap();
        for field in 0..4 {
            let mut wrong = peer();
            match field {
                0 => wrong.uid += 1,
                1 => wrong.pid += 1,
                2 => wrong.pid_version += 1,
                _ => wrong.audit_session += 1,
            }
            assert_eq!(
                check(&mut session, wrong, "a", 100, now, 100),
                Err(Error::WrongPeer)
            );
        }
        check(&mut session, peer(), "a", 100, now, 100).unwrap();
        assert_eq!(
            check(
                &mut session,
                peer(),
                "b",
                100,
                now + Duration::from_secs(60),
                100
            ),
            Err(Error::Expired)
        );
    }
    #[test]
    fn every_signed_field_and_generation_are_bound() {
        let now = Instant::now();
        let mut session = Session::new(peer(), [7; 32], 1, now, Duration::from_secs(60)).unwrap();
        let signature = sign(1, &call("a", 100));
        for field in 0..4 {
            let mut changed = call("a", 100);
            changed.signature = &signature;
            match field {
                0 => changed.id = "b",
                1 => changed.timestamp = 99,
                2 => changed.command = "detach",
                _ => changed.payload = b"changed",
            }
            assert_eq!(
                session.verify(peer(), changed, now, 100),
                Err(Error::InvalidSignature)
            );
        }
        check(&mut session, peer(), "a", 100, now, 100).unwrap();
        assert_eq!(
            check(&mut session, peer(), "a", 100, now, 100),
            Err(Error::Replay)
        );
        let mut successor = Session::new(peer(), [7; 32], 2, now, Duration::from_secs(60)).unwrap();
        assert_eq!(
            check(&mut successor, peer(), "a", 100, now, 100),
            Err(Error::InvalidSignature)
        );
    }
    #[test]
    fn future_and_old_timestamps_are_refused() {
        let now = Instant::now();
        let mut session = Session::new(peer(), [7; 32], 1, now, Duration::from_secs(60)).unwrap();
        for timestamp in [101, 69] {
            assert_eq!(
                check(&mut session, peer(), "a", timestamp, now, 100),
                Err(Error::StaleRequest)
            );
        }
    }
    #[test]
    fn replay_memory_is_bounded_and_cannot_evict_fresh_requests() {
        let now = Instant::now();
        let mut session = Session::new(peer(), [7; 32], 1, now, Duration::from_secs(100)).unwrap();
        for index in 0..MAX_REPLAYS {
            check(&mut session, peer(), &format!("id-{index}"), 100, now, 100).unwrap();
        }
        assert_eq!(
            check(&mut session, peer(), "extra", 100, now, 100),
            Err(Error::Capacity)
        );
        assert_eq!(
            check(&mut session, peer(), "id-0", 100, now, 100),
            Err(Error::Replay)
        );
        check(
            &mut session,
            peer(),
            "extra",
            131,
            now + Duration::from_secs(31),
            131,
        )
        .unwrap();
    }
}
