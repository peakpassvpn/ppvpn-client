//! Broker OS-resource leases, NOT a second capture transaction state machine.
//! CaptureMachine/CaptureController and all capture DTOs are ppvpn-core-wire's.
use ppvpn_core_wire::{CaptureLease, ForwarderIdentity};
use std::collections::HashMap;
use std::time::{Duration, Instant};
pub const LEASE: Duration = Duration::from_secs(45);
pub const RENEW_EVERY: Duration = Duration::from_secs(15);
const MAX_FORWARDERS: usize = 256;
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
/// Capture-only OS resources. Cleanup must undo routes/DNS even while another
/// process retains a duplicated fd. Partial creation must roll back on error.
pub trait Resources {
    fn cleanup(&mut self) -> Result<(), String>;
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Error {
    InvalidOwner,
    InvalidLease,
    Busy,
    NotOwner,
    Expired,
    StaleLease,
    Capacity,
    NotQuarantined,
    Create(String),
    Cleanup(String),
    Quarantined,
}
struct Active<R> {
    owner: Owner,
    lease: CaptureLease,
    deadline: Instant,
    resources: R,
    quarantined: bool,
}
pub struct Capture<R> {
    // Broker replay guard for OS grants. Epochs originate in the shared wire
    // coordinator; the broker does not allocate a parallel capture generation.
    seen: HashMap<(String, u64), u64>,
    active: Option<Active<R>>,
}
impl<R> Default for Capture<R> {
    fn default() -> Self {
        Self {
            seen: HashMap::new(),
            active: None,
        }
    }
}
impl<R: Resources> Capture<R> {
    pub fn acquire(
        &mut self,
        owner: Owner,
        lease: CaptureLease,
        now: Instant,
        create: impl FnOnce(&CaptureLease) -> Result<R, String>,
    ) -> Result<(), Error> {
        if !owner.valid() {
            return Err(Error::InvalidOwner);
        }
        if lease.owner.instance.is_empty()
            || lease.owner.instance.len() > 128
            || lease.owner.generation == 0
            || lease.attachment_epoch == 0
        {
            return Err(Error::InvalidLease);
        }
        if self.active.is_some() {
            return Err(Error::Busy);
        }
        let key = (lease.owner.instance.clone(), lease.owner.generation);
        if self
            .seen
            .get(&key)
            .is_some_and(|epoch| *epoch >= lease.attachment_epoch)
        {
            return Err(Error::StaleLease);
        }
        if !self.seen.contains_key(&key) && self.seen.len() >= MAX_FORWARDERS {
            return Err(Error::Capacity);
        }
        self.seen.insert(key, lease.attachment_epoch);
        let resources = create(&lease).map_err(Error::Create)?;
        self.active = Some(Active {
            owner,
            lease,
            resources,
            deadline: now + LEASE,
            quarantined: false,
        });
        Ok(())
    }
    fn authorize(&self, owner: &Owner, lease: &CaptureLease) -> Result<&Active<R>, Error> {
        let active = self.active.as_ref().ok_or(Error::NotOwner)?;
        if &active.owner != owner || &active.lease != lease {
            return Err(Error::NotOwner);
        }
        Ok(active)
    }
    pub fn renew(
        &mut self,
        owner: &Owner,
        lease: &CaptureLease,
        now: Instant,
    ) -> Result<(), Error> {
        let active = self.authorize(owner, lease)?;
        if active.quarantined {
            return Err(Error::Quarantined);
        }
        if now >= active.deadline {
            return Err(Error::Expired);
        }
        self.active.as_mut().unwrap().deadline = now + LEASE;
        Ok(())
    }
    pub fn detach(&mut self, owner: &Owner, lease: &CaptureLease) -> Result<(), Error> {
        self.authorize(owner, lease)?;
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
    /// Operate only on this authenticated, live OS grant. The core owns the
    /// capture transaction; failed mutation remains available for rollback.
    pub fn resources<T>(
        &mut self,
        owner: &Owner,
        lease: &CaptureLease,
        now: Instant,
        call: impl FnOnce(&mut R) -> Result<T, String>,
    ) -> Result<T, Error> {
        let active = self.authorize(owner, lease)?;
        if active.quarantined {
            return Err(Error::Quarantined);
        }
        if now >= active.deadline {
            return Err(Error::Expired);
        }
        call(&mut self.active.as_mut().unwrap().resources).map_err(Error::Cleanup)
    }
    pub fn broker_shutdown(&mut self) -> Result<(), Error> {
        self.cleanup()
    }
    pub fn occupied(&self) -> bool {
        self.active.is_some()
    }
    pub fn forwarder(&self) -> Option<&ForwarderIdentity> {
        self.active.as_ref().map(|active| &active.lease.owner)
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
            let mut s = self.0.lock().unwrap();
            s.0 += 1;
            if s.1 {
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
    fn lease(epoch: u64) -> CaptureLease {
        CaptureLease {
            owner: ForwarderIdentity {
                instance: "runtime-a".into(),
                generation: 1,
            },
            attachment_epoch: epoch,
        }
    }
    #[test]
    fn invalid_busy_and_stale_requests_do_not_allocate() {
        let mut c = Capture::default();
        let now = Instant::now();
        let mut bad = owner();
        bad.uid = 0;
        assert_eq!(
            c.acquire(bad, lease(1), now, |_| -> Result<Fake, String> {
                panic!("must not allocate")
            }),
            Err(Error::InvalidOwner)
        );
        c.acquire(owner(), lease(1), now, |_| Ok(Fake::default()))
            .unwrap();
        assert_eq!(
            c.acquire(owner(), lease(2), now, |_| panic!("must not allocate")),
            Err(Error::Busy)
        );
        c.detach(&owner(), &lease(1)).unwrap();
        assert_eq!(
            c.acquire(owner(), lease(1), now, |_| panic!("must not allocate")),
            Err(Error::StaleLease)
        );
        c.acquire(owner(), lease(3), now, |_| Ok(Fake::default()))
            .unwrap();
    }
    #[test]
    fn expiry_and_renewal_affect_only_os_resources() {
        let mut c = Capture::default();
        let now = Instant::now();
        let f = Fake::default();
        c.acquire(owner(), lease(1), now, |_| Ok(f.clone()))
            .unwrap();
        c.renew(&owner(), &lease(1), now + RENEW_EVERY).unwrap();
        assert_eq!(c.expire(now + LEASE), Ok(false));
        assert_eq!(
            c.renew(&owner(), &lease(1), now + RENEW_EVERY + LEASE),
            Err(Error::Expired)
        );
        assert_eq!(c.expire(now + RENEW_EVERY + LEASE), Ok(true));
        assert_eq!(f.0.lock().unwrap().0, 1);
    }
    #[test]
    fn all_owner_fields_and_shared_wire_lease_are_bound() {
        let mut c = Capture::default();
        let now = Instant::now();
        let f = Fake::default();
        c.acquire(owner(), lease(1), now, |_| Ok(f.clone()))
            .unwrap();
        for field in 0..6 {
            let mut other = owner();
            match field {
                0 => other.uid += 1,
                1 => other.pid += 1,
                2 => other.pid_version += 1,
                3 => other.audit_session += 1,
                4 => other.session_id.push('x'),
                _ => other.generation += 1,
            };
            assert_eq!(c.renew(&other, &lease(1), now), Err(Error::NotOwner));
            assert_eq!(c.detach(&other, &lease(1)), Err(Error::NotOwner));
        }
        for field in 0..3 {
            let mut other = lease(1);
            match field {
                0 => other.owner.instance.push('x'),
                1 => other.owner.generation += 1,
                _ => other.attachment_epoch += 1,
            };
            assert_eq!(c.detach(&owner(), &other), Err(Error::NotOwner));
        }
        assert_eq!(f.0.lock().unwrap().0, 0);
    }
    #[test]
    fn cleanup_failure_quarantines_the_grant() {
        let mut c = Capture::default();
        let now = Instant::now();
        let f = Fake::default();
        f.0.lock().unwrap().1 = true;
        c.acquire(owner(), lease(1), now, |_| Ok(f.clone()))
            .unwrap();
        assert!(matches!(
            c.detach(&owner(), &lease(1)),
            Err(Error::Cleanup(_))
        ));
        assert_eq!(c.renew(&owner(), &lease(1), now), Err(Error::Quarantined));
        assert_eq!(
            c.acquire(owner(), lease(2), now, |_| panic!("must not allocate")),
            Err(Error::Busy)
        );
        f.0.lock().unwrap().1 = false;
        c.retry_cleanup().unwrap();
        assert!(!c.occupied());
    }
    #[test]
    fn failed_creation_consumes_shared_epoch_without_occupying() {
        let mut c: Capture<Fake> = Capture::default();
        let now = Instant::now();
        assert_eq!(
            c.acquire(owner(), lease(1), now, |_| Err("rolled back".into())),
            Err(Error::Create("rolled back".into()))
        );
        assert_eq!(
            c.acquire(owner(), lease(1), now, |_| panic!("must not allocate")),
            Err(Error::StaleLease)
        );
        c.acquire(owner(), lease(2), now, |_| Ok(Fake::default()))
            .unwrap();
        c.broker_shutdown().unwrap();
        assert!(!c.occupied());
    }
}
