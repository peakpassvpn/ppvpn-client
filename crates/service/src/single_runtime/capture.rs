//! A capture-only lease. This module cannot stop a forwarding Engine.
use std::time::{Duration, Instant};

pub const LEASE: Duration = Duration::from_secs(45);
pub const RENEW_EVERY: Duration = Duration::from_secs(15);

/// Values obtained from an authenticated OS peer and a verified request, never
/// trusted directly from JSON. PID version and audit session prevent PID reuse
/// and cross-login-session adoption. Authentication/replay checks precede this API.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Owner {
    pub uid: u32,
    pub pid: u32,
    pub pid_version: u32,
    pub audit_session: u32,
    pub session_id: String,
    pub generation: u64,
}

impl Owner {
    fn valid(&self) -> bool {
        self.uid != 0
            && self.pid != 0
            && self.pid_version != 0
            && !self.session_id.is_empty()
            && self.session_id.len() <= 128
            && self.generation != 0
    }
}

/// Scoped OS resources only: interface, owned routes, DNS registration/ledger.
/// Cleanup must be idempotent. It must undo capture even if another process
/// retains a duplicated TUN fd; closing our fd alone does not accomplish that.
/// The implementation must roll back partial allocation before returning error.
pub trait Resources {
    fn cleanup(&mut self) -> Result<(), String>;
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CaptureId(u64);
impl CaptureId {
    pub fn get(self) -> u64 {
        self.0
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidOwner,
    Busy,
    NotOwner,
    Expired,
    GenerationExhausted,
    NotQuarantined,
    Create(String),
    Cleanup(String),
    Quarantined,
}

struct Active<R> {
    owner: Owner,
    id: CaptureId,
    deadline: Instant,
    resources: R,
    quarantined: bool,
}

pub struct Capture<R> {
    next: u64,
    active: Option<Active<R>>,
}
impl<R> Default for Capture<R> {
    fn default() -> Self {
        Self {
            next: 0,
            active: None,
        }
    }
}

impl<R: Resources> Capture<R> {
    /// Reserve before allocating OS resources. Never calls `create` for an
    /// invalid owner, occupied slot or exhausted generation counter.
    pub fn acquire(
        &mut self,
        owner: Owner,
        now: Instant,
        create: impl FnOnce(CaptureId) -> Result<R, String>,
    ) -> Result<CaptureId, Error> {
        if !owner.valid() {
            return Err(Error::InvalidOwner);
        }
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        self.next = self.next.checked_add(1).ok_or(Error::GenerationExhausted)?;
        let id = CaptureId(self.next);
        let resources = create(id).map_err(Error::Create)?;
        self.active = Some(Active {
            owner,
            id,
            resources,
            deadline: now + LEASE,
            quarantined: false,
        });
        Ok(id)
    }

    fn authorize(&self, owner: &Owner, id: CaptureId) -> Result<&Active<R>, Error> {
        let active = self.active.as_ref().ok_or(Error::NotOwner)?;
        if &active.owner != owner || active.id != id {
            return Err(Error::NotOwner);
        }
        Ok(active)
    }

    pub fn renew(&mut self, owner: &Owner, id: CaptureId, now: Instant) -> Result<(), Error> {
        let active = self.authorize(owner, id)?;
        if active.quarantined {
            return Err(Error::Quarantined);
        }
        if now >= active.deadline {
            return Err(Error::Expired);
        }
        self.active.as_mut().unwrap().deadline = now + LEASE;
        Ok(())
    }

    /// Wrong/stale owners cause no resource changes. Cleanup errors retain the
    /// resource slot, preventing a successor from racing ambiguous OS state.
    pub fn detach(&mut self, owner: &Owner, id: CaptureId) -> Result<(), Error> {
        self.authorize(owner, id)?;
        self.cleanup()
    }

    fn cleanup(&mut self) -> Result<(), Error> {
        let Some(active) = self.active.as_mut() else {
            return Ok(());
        };
        match active.resources.cleanup() {
            Ok(()) => {
                self.active = None;
                Ok(())
            }
            Err(error) => {
                active.quarantined = true;
                Err(Error::Cleanup(error))
            }
        }
    }

    /// Broker watchdog only. Monotonic expiry is independent of auth lifetime
    /// and of the UI's wall clock; it does not hold a forwarding runtime handle.
    pub fn expire(&mut self, now: Instant) -> Result<bool, Error> {
        let Some(active) = self.active.as_ref() else {
            return Ok(false);
        };
        if active.quarantined {
            return Err(Error::Quarantined);
        }
        if now < active.deadline {
            return Ok(false);
        }
        self.cleanup().map(|()| true)
    }

    /// Trusted broker recovery after reconciling the resource ledger.
    pub fn retry_cleanup(&mut self) -> Result<(), Error> {
        if !self
            .active
            .as_ref()
            .is_some_and(|active| active.quarantined)
        {
            return Err(Error::NotQuarantined);
        }
        self.cleanup()
    }

    pub fn broker_shutdown(&mut self) -> Result<(), Error> {
        self.cleanup()
    }
    pub fn occupied(&self) -> bool {
        self.active.is_some()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    #[derive(Clone, Default)]
    struct Fake(Arc<Mutex<(usize, bool)>>);
    impl Resources for Fake {
        fn cleanup(&mut self) -> Result<(), String> {
            let mut state = self.0.lock().unwrap();
            state.0 += 1;
            if state.1 {
                Err("route cleanup failed".into())
            } else {
                Ok(())
            }
        }
    }
    fn owner() -> Owner {
        Owner {
            uid: 501,
            pid: 42,
            pid_version: 3,
            audit_session: 7,
            session_id: "s".into(),
            generation: 1,
        }
    }
    #[test]
    fn invalid_or_busy_requests_do_not_allocate() {
        let mut capture = Capture::default();
        let now = Instant::now();
        let mut invalid = owner();
        invalid.uid = 0;
        assert_eq!(
            capture.acquire(invalid, now, |_| -> Result<Fake, String> {
                panic!("must not allocate")
            }),
            Err(Error::InvalidOwner)
        );
        capture
            .acquire(owner(), now, |_| Ok(Fake::default()))
            .unwrap();
        assert_eq!(
            capture.acquire(owner(), now, |_| { panic!("must not allocate") }),
            Err(Error::Busy)
        );
    }
    #[test]
    fn expiry_cleans_capture_and_never_extends_expired_grant() {
        let fake = Fake::default();
        let mut capture = Capture::default();
        let now = Instant::now();
        let id = capture.acquire(owner(), now, |_| Ok(fake.clone())).unwrap();
        assert_eq!(
            capture.expire(now + LEASE - Duration::from_nanos(1)),
            Ok(false)
        );
        assert_eq!(
            capture.renew(&owner(), id, now + LEASE),
            Err(Error::Expired)
        );
        assert_eq!(capture.expire(now + LEASE), Ok(true));
        assert_eq!(fake.0.lock().unwrap().0, 1);
        assert!(!capture.occupied());
    }
    #[test]
    fn renewal_is_capture_only_and_uses_monotonic_time() {
        let mut capture = Capture::default();
        let now = Instant::now();
        let fake = Fake::default();
        let id = capture.acquire(owner(), now, |_| Ok(fake.clone())).unwrap();
        capture.renew(&owner(), id, now + RENEW_EVERY).unwrap();
        assert_eq!(capture.expire(now + LEASE), Ok(false));
        assert_eq!(capture.expire(now + RENEW_EVERY + LEASE), Ok(true));
        assert_eq!(fake.0.lock().unwrap().0, 1);
    }
    #[test]
    fn every_owner_component_and_capture_generation_are_bound() {
        let mut capture = Capture::default();
        let now = Instant::now();
        let fake = Fake::default();
        let id = capture.acquire(owner(), now, |_| Ok(fake.clone())).unwrap();
        for field in 0..6 {
            let mut other = owner();
            match field {
                0 => other.uid += 1,
                1 => other.pid += 1,
                2 => other.pid_version += 1,
                3 => other.audit_session += 1,
                4 => other.session_id.push('x'),
                _ => other.generation += 1,
            }
            assert_eq!(capture.renew(&other, id, now), Err(Error::NotOwner));
            assert_eq!(capture.detach(&other, id), Err(Error::NotOwner));
        }
        capture.detach(&owner(), id).unwrap();
        let next = capture.acquire(owner(), now, |_| Ok(fake.clone())).unwrap();
        assert_ne!(id, next);
        assert_eq!(capture.detach(&owner(), id), Err(Error::NotOwner));
        assert_eq!(fake.0.lock().unwrap().0, 1);
    }
    #[test]
    fn failed_cleanup_quarantines_until_reconciled() {
        let mut capture = Capture::default();
        let now = Instant::now();
        let fake = Fake::default();
        fake.0.lock().unwrap().1 = true;
        let id = capture.acquire(owner(), now, |_| Ok(fake.clone())).unwrap();
        assert!(matches!(
            capture.detach(&owner(), id),
            Err(Error::Cleanup(_))
        ));
        assert!(capture.occupied());
        assert_eq!(capture.renew(&owner(), id, now), Err(Error::Quarantined));
        assert_eq!(
            capture.acquire(owner(), now, |_| panic!("must not allocate")),
            Err(Error::Busy)
        );
        fake.0.lock().unwrap().1 = false;
        capture.retry_cleanup().unwrap();
        assert!(!capture.occupied());
    }
    #[test]
    fn failed_allocation_does_not_occupy_but_consumes_generation() {
        let mut capture: Capture<Fake> = Capture::default();
        let now = Instant::now();
        assert_eq!(
            capture.acquire(owner(), now, |_| Err("rolled back".into())),
            Err(Error::Create("rolled back".into()))
        );
        assert_eq!(
            capture
                .acquire(owner(), now, |_| Ok(Fake::default()))
                .unwrap()
                .get(),
            2
        );
        capture.broker_shutdown().unwrap();
        assert_eq!(capture.expire(now + LEASE), Ok(false));
    }
}
