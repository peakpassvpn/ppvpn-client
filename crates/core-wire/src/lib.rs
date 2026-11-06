//! Capture protocol v1. No runtime, platform handles, Sail, accounts or I/O.
//! A host must negotiate capabilities before creating a native capture driver.

use serde::{Deserialize, Serialize};

pub const CAPTURE_PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForwarderIdentity {
    /// Opaque instance id, not a PID. A process change creates a new identity.
    pub instance: String,
    /// A stop/start changes this generation; capture changes never do.
    pub generation: u64,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Preservation {
    pub dispatcher: bool,
    pub mixed_tcp: bool,
    pub mixed_udp: bool,
    pub dns_state: bool,
    pub selection: bool,
    pub counters: bool,
}

impl Preservation {
    pub fn complete(&self) -> bool {
        self.dispatcher
            && self.mixed_tcp
            && self.mixed_udp
            && self.dns_state
            && self.selection
            && self.counters
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureCapabilities {
    pub protocol_version: u32,
    pub live_attach: bool,
    pub live_detach: bool,
    pub transactional_rollback: bool,
    pub preserves: Preservation,
    /// Always false in v1: live sessions cannot move between OS processes.
    pub cross_process_migration: bool,
}

impl CaptureCapabilities {
    pub fn unsupported() -> Self {
        Self {
            protocol_version: CAPTURE_PROTOCOL_VERSION,
            live_attach: false,
            live_detach: false,
            transactional_rollback: false,
            preserves: Preservation::default(),
            cross_process_migration: false,
        }
    }

    pub fn supports_live_capture(&self) -> bool {
        self.protocol_version == CAPTURE_PROTOCOL_VERSION
            && self.live_attach
            && self.live_detach
            && self.transactional_rollback
            && self.preserves.complete()
            && !self.cross_process_migration
    }
}

/// Tokens name process-local resources; never deserialize raw FDs or pointers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum CaptureResource {
    /// The runtime opens and owns the device and its capture-only resources.
    Managed { resource_id: String },
    /// The host retains its original handle and network-settings lease. The
    /// runtime resolves this token locally and owns only a duplicate.
    HostLease { lease_id: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureSpec {
    pub resource: CaptureResource,
    /// Platform-prepared settings must match this IPv6 policy.
    pub ipv6: bool,
}

impl CaptureSpec {
    fn valid(&self) -> bool {
        let id = match &self.resource {
            CaptureResource::Managed { resource_id } => resource_id,
            CaptureResource::HostLease { lease_id } => lease_id,
        };
        !id.is_empty()
            && id.len() <= 128
            && id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b))
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLease {
    pub owner: ForwarderIdentity,
    pub attachment_epoch: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AttachCapture {
    pub protocol_version: u32,
    pub expected_epoch: u64,
    pub spec: CaptureSpec,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DetachCapture {
    pub protocol_version: u32,
    pub expected_epoch: u64,
    pub lease: CaptureLease,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapturePhase {
    Detached,
    Attaching,
    Attached,
    Detaching,
    RecoveryRequired,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "SCREAMING_SNAKE_CASE")]
pub enum CaptureErrorCode {
    CaptureUnsupported,
    CaptureProtocolUnsupported,
    CaptureEpochMismatch,
    CaptureNotRunning,
    CaptureOwnerStopping,
    CaptureAlreadyAttached,
    CaptureAlreadyDetached,
    CaptureLeaseMismatch,
    CaptureInvalidResource,
    CaptureBusy,
    CaptureOperationFailed,
    CaptureRollbackFailed,
    CaptureRecoveryRequired,
    CaptureEpochExhausted,
    CaptureTransactionMismatch,
}
impl CaptureErrorCode {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::CaptureUnsupported => "CAPTURE_UNSUPPORTED",
            Self::CaptureProtocolUnsupported => "CAPTURE_PROTOCOL_UNSUPPORTED",
            Self::CaptureEpochMismatch => "CAPTURE_EPOCH_MISMATCH",
            Self::CaptureNotRunning => "CAPTURE_NOT_RUNNING",
            Self::CaptureOwnerStopping => "CAPTURE_OWNER_STOPPING",
            Self::CaptureAlreadyAttached => "CAPTURE_ALREADY_ATTACHED",
            Self::CaptureAlreadyDetached => "CAPTURE_ALREADY_DETACHED",
            Self::CaptureLeaseMismatch => "CAPTURE_LEASE_MISMATCH",
            Self::CaptureInvalidResource => "CAPTURE_INVALID_RESOURCE",
            Self::CaptureBusy => "CAPTURE_BUSY",
            Self::CaptureOperationFailed => "CAPTURE_OPERATION_FAILED",
            Self::CaptureRollbackFailed => "CAPTURE_ROLLBACK_FAILED",
            Self::CaptureRecoveryRequired => "CAPTURE_RECOVERY_REQUIRED",
            Self::CaptureEpochExhausted => "CAPTURE_EPOCH_EXHAUSTED",
            Self::CaptureTransactionMismatch => "CAPTURE_TRANSACTION_MISMATCH",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceKind {
    Device,
    Handle,
    Route,
    Dns,
    Worker,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureLeftover {
    pub kind: ResourceKind,
    /// An opaque ownership token. Diagnostics must not insert secrets here.
    pub resource_id: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureStatus {
    pub protocol_version: u32,
    pub owner: ForwarderIdentity,
    pub epoch: u64,
    pub phase: CapturePhase,
    pub lease: Option<CaptureLease>,
    pub spec: Option<CaptureSpec>,
    pub leftovers: Vec<CaptureLeftover>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureError {
    pub code: CaptureErrorCode,
    pub status: Box<CaptureStatus>,
}

/// An internal transaction plan, not a wire request. The native driver stages
/// changes on exactly this owner without restarting it or replacing dispatch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureChange {
    Attach(CaptureSpec),
    Detach(CaptureLease),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureTransaction {
    epoch: u64,
    owner: ForwarderIdentity,
    change: CaptureChange,
}

impl CaptureTransaction {
    pub fn epoch(&self) -> u64 {
        self.epoch
    }
    pub fn owner(&self) -> &ForwarderIdentity {
        &self.owner
    }
    pub fn change(&self) -> &CaptureChange {
        &self.change
    }
}

/// Only a native driver can establish these outcomes. `RolledBack` requires
/// restoration of the prior capture state and zero staged resource leftovers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CaptureOutcome {
    Committed,
    RolledBack,
    RollbackFailed(Vec<CaptureLeftover>),
}

#[derive(Debug, Clone)]
struct Pending {
    transaction: CaptureTransaction,
    before: CaptureStatus,
}

/// Pure transition authority. A host serializes access and runs native work
/// between begin/complete. Querying status never waits for native I/O. A driver
/// must settle or report recovery-required on cancellation; dropping a task is
/// not evidence that its routes/DNS/device have rolled back.
#[derive(Debug)]
pub struct CaptureMachine {
    capabilities: CaptureCapabilities,
    status: CaptureStatus,
    pending: Option<Pending>,
}

impl CaptureMachine {
    pub fn new(owner: ForwarderIdentity, capabilities: CaptureCapabilities) -> Self {
        Self {
            capabilities,
            status: CaptureStatus {
                protocol_version: CAPTURE_PROTOCOL_VERSION,
                owner,
                epoch: 0,
                phase: CapturePhase::Detached,
                lease: None,
                spec: None,
                leftovers: vec![],
            },
            pending: None,
        }
    }
    pub fn capabilities(&self) -> &CaptureCapabilities {
        &self.capabilities
    }
    pub fn status(&self) -> &CaptureStatus {
        &self.status
    }
    fn error(&self, code: CaptureErrorCode) -> CaptureError {
        CaptureError {
            code,
            status: Box::new(self.status.clone()),
        }
    }
    fn preflight(&self, version: u32, epoch: u64, running: bool) -> Result<(), CaptureError> {
        if version != CAPTURE_PROTOCOL_VERSION {
            return Err(self.error(CaptureErrorCode::CaptureProtocolUnsupported));
        }
        if !self.capabilities.supports_live_capture() {
            return Err(self.error(CaptureErrorCode::CaptureUnsupported));
        }
        if epoch != self.status.epoch {
            return Err(self.error(CaptureErrorCode::CaptureEpochMismatch));
        }
        if self.status.phase == CapturePhase::RecoveryRequired {
            return Err(self.error(CaptureErrorCode::CaptureRecoveryRequired));
        }
        if self.pending.is_some() {
            return Err(self.error(CaptureErrorCode::CaptureBusy));
        }
        if !running {
            return Err(self.error(CaptureErrorCode::CaptureNotRunning));
        }
        Ok(())
    }
    pub fn begin_attach(
        &mut self,
        request: AttachCapture,
        running: bool,
    ) -> Result<CaptureTransaction, CaptureError> {
        self.preflight(request.protocol_version, request.expected_epoch, running)?;
        if self.status.phase != CapturePhase::Detached {
            return Err(self.error(CaptureErrorCode::CaptureAlreadyAttached));
        }
        if !request.spec.valid() {
            return Err(self.error(CaptureErrorCode::CaptureInvalidResource));
        }
        self.begin(CaptureChange::Attach(request.spec), CapturePhase::Attaching)
    }
    pub fn begin_detach(
        &mut self,
        request: DetachCapture,
        running: bool,
    ) -> Result<CaptureTransaction, CaptureError> {
        self.preflight(request.protocol_version, request.expected_epoch, running)?;
        if self.status.phase != CapturePhase::Attached {
            return Err(self.error(CaptureErrorCode::CaptureAlreadyDetached));
        }
        if self.status.lease.as_ref() != Some(&request.lease) {
            return Err(self.error(CaptureErrorCode::CaptureLeaseMismatch));
        }
        self.begin(
            CaptureChange::Detach(request.lease),
            CapturePhase::Detaching,
        )
    }
    fn begin(
        &mut self,
        change: CaptureChange,
        phase: CapturePhase,
    ) -> Result<CaptureTransaction, CaptureError> {
        let Some(epoch) = self.status.epoch.checked_add(1) else {
            return Err(self.error(CaptureErrorCode::CaptureEpochExhausted));
        };
        let transaction = CaptureTransaction {
            epoch,
            owner: self.status.owner.clone(),
            change,
        };
        self.pending = Some(Pending {
            transaction: transaction.clone(),
            before: self.status.clone(),
        });
        self.status.epoch = epoch;
        self.status.phase = phase;
        Ok(transaction)
    }
    /// Native settlement became unknowable (worker panic or termination).
    /// Invalidate its completion token; a late callback cannot claim success.
    pub fn require_recovery(&mut self, leftovers: Vec<CaptureLeftover>) -> CaptureError {
        self.pending = None;
        self.status.phase = CapturePhase::RecoveryRequired;
        self.status.leftovers = leftovers;
        self.error(CaptureErrorCode::CaptureRecoveryRequired)
    }

    pub fn complete(
        &mut self,
        transaction: &CaptureTransaction,
        outcome: CaptureOutcome,
    ) -> Result<CaptureStatus, CaptureError> {
        let Some(pending) = self
            .pending
            .as_ref()
            .filter(|p| &p.transaction == transaction)
        else {
            return Err(self.error(CaptureErrorCode::CaptureTransactionMismatch));
        };
        let before = pending.before.clone();
        self.pending = None;
        match outcome {
            CaptureOutcome::Committed => {
                match &transaction.change {
                    CaptureChange::Attach(spec) => {
                        self.status.phase = CapturePhase::Attached;
                        self.status.spec = Some(spec.clone());
                        self.status.lease = Some(CaptureLease {
                            owner: self.status.owner.clone(),
                            attachment_epoch: transaction.epoch,
                        });
                    }
                    CaptureChange::Detach(_) => {
                        self.status.phase = CapturePhase::Detached;
                        self.status.spec = None;
                        self.status.lease = None;
                    }
                }
                self.status.leftovers.clear();
                Ok(self.status.clone())
            }
            CaptureOutcome::RolledBack => {
                self.status = CaptureStatus {
                    epoch: transaction.epoch,
                    ..before
                };
                Err(self.error(CaptureErrorCode::CaptureOperationFailed))
            }
            CaptureOutcome::RollbackFailed(leftovers) => {
                self.status = CaptureStatus {
                    epoch: transaction.epoch,
                    phase: CapturePhase::RecoveryRequired,
                    leftovers,
                    ..before
                };
                Err(self.error(CaptureErrorCode::CaptureRollbackFailed))
            }
        }
    }
}

#[cfg(feature = "driver")]
pub mod driver;

/// Read-only observation of one forwarding owner. Registry IDs describe only
/// this process; this is not proof of uniqueness across processes or devices.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RuntimeObservation {
    pub owner: ForwarderIdentity,
    pub backend: RuntimeBackend,
    pub native_runtime_id: Option<u64>,
    pub state: String,
    pub running: bool,
    /// Actual Sail running registry; absent for test backends.
    pub active_runtime_ids: Option<Vec<u64>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RuntimeBackend {
    Sail,
    Fake,
}

#[cfg(test)]
mod tests;
