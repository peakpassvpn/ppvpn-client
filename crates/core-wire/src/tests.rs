use super::*;

pub(super) fn capabilities() -> CaptureCapabilities {
    CaptureCapabilities {
        protocol_version: CAPTURE_PROTOCOL_VERSION,
        live_attach: true,
        live_detach: true,
        transactional_rollback: true,
        preserves: Preservation {
            dispatcher: true,
            mixed_tcp: true,
            mixed_udp: true,
            dns_state: true,
            selection: true,
            counters: true,
        },
        cross_process_migration: false,
    }
}
pub(super) fn owner() -> ForwarderIdentity {
    ForwarderIdentity {
        instance: "forwarder-a".into(),
        generation: 1,
    }
}
fn machine() -> CaptureMachine {
    CaptureMachine::new(owner(), capabilities())
}
pub(super) fn attach(epoch: u64) -> AttachCapture {
    AttachCapture {
        protocol_version: 1,
        expected_epoch: epoch,
        spec: CaptureSpec {
            resource: CaptureResource::HostLease {
                lease_id: "provider-handle-1".into(),
            },
            ipv6: true,
        },
    }
}
fn attached(m: &mut CaptureMachine) -> CaptureLease {
    let tx = m.begin_attach(attach(m.status().epoch), true).unwrap();
    m.complete(&tx, CaptureOutcome::Committed)
        .unwrap()
        .lease
        .unwrap()
}
fn detach(m: &CaptureMachine, lease: CaptureLease) -> DetachCapture {
    DetachCapture {
        protocol_version: 1,
        expected_epoch: m.status().epoch,
        lease,
    }
}

#[test]
fn protocol_shapes_are_versioned_and_do_not_accept_raw_handles() {
    let request = attach(0);
    let value = serde_json::to_value(&request).unwrap();
    assert_eq!(
        value,
        serde_json::json!({"protocol_version":1,"expected_epoch":0,
        "spec":{"resource":{"kind":"host_lease","lease_id":"provider-handle-1"},"ipv6":true}})
    );
    assert_eq!(
        serde_json::from_value::<AttachCapture>(value.clone()).unwrap(),
        request
    );
    let mut raw = value;
    raw["spec"]["resource"]["fd"] = 7.into();
    assert!(serde_json::from_value::<AttachCapture>(raw).is_err());
    assert_eq!(
        serde_json::to_value(CaptureErrorCode::CaptureUnsupported).unwrap(),
        "CAPTURE_UNSUPPORTED"
    );
}

#[test]
fn unsupported_capabilities_refuse_before_a_transaction_or_epoch_change() {
    let mut m = CaptureMachine::new(owner(), CaptureCapabilities::unsupported());
    let before = m.status().clone();
    assert_eq!(
        m.begin_attach(attach(0), true).unwrap_err().code,
        CaptureErrorCode::CaptureUnsupported
    );
    assert_eq!(m.status(), &before);
    let mut partial = capabilities();
    partial.preserves.mixed_udp = false;
    assert!(!partial.supports_live_capture());
    let mut m = CaptureMachine::new(owner(), partial);
    assert_eq!(
        m.begin_attach(attach(0), true).unwrap_err().code,
        CaptureErrorCode::CaptureUnsupported
    );
}

#[test]
fn attach_detach_keep_one_forwarding_owner_and_monotonic_epochs() {
    let mut m = machine();
    let lease = attached(&mut m);
    assert_eq!(lease.owner, owner());
    assert_eq!(m.status().phase, CapturePhase::Attached);
    let tx = m.begin_detach(detach(&m, lease), true).unwrap();
    assert_eq!(m.status().phase, CapturePhase::Detaching);
    let status = m.complete(&tx, CaptureOutcome::Committed).unwrap();
    assert_eq!(status.phase, CapturePhase::Detached);
    assert_eq!(
        (status.owner, status.epoch, status.lease),
        (owner(), 2, None)
    );
}

#[test]
fn failed_attach_rolls_back_and_does_not_reuse_the_reserved_epoch() {
    let mut m = machine();
    let tx = m.begin_attach(attach(0), true).unwrap();
    assert_eq!(
        m.complete(&tx, CaptureOutcome::RolledBack)
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureOperationFailed
    );
    assert_eq!(m.status().phase, CapturePhase::Detached);
    assert_eq!(m.status().epoch, 1);
    assert_eq!(
        m.begin_attach(attach(0), true).unwrap_err().code,
        CaptureErrorCode::CaptureEpochMismatch
    );
    assert!(m.begin_attach(attach(1), true).is_ok());
}

#[test]
fn failed_detach_restores_the_exact_lease_and_settings() {
    let mut m = machine();
    let lease = attached(&mut m);
    let before = m.status().clone();
    let tx = m.begin_detach(detach(&m, lease), true).unwrap();
    m.complete(&tx, CaptureOutcome::RolledBack).unwrap_err();
    assert_eq!(m.status().lease, before.lease);
    assert_eq!(m.status().spec, before.spec);
    assert_eq!(m.status().phase, CapturePhase::Attached);
    assert_eq!(m.status().epoch, before.epoch + 1);
}

#[test]
fn rollback_failure_exposes_owned_leftovers_and_blocks_capture_mutations() {
    let mut m = machine();
    let tx = m.begin_attach(attach(0), true).unwrap();
    let leftovers = vec![CaptureLeftover {
        kind: ResourceKind::Route,
        resource_id: "capture-route-1".into(),
    }];
    let error = m
        .complete(&tx, CaptureOutcome::RollbackFailed(leftovers.clone()))
        .unwrap_err();
    assert_eq!(error.code, CaptureErrorCode::CaptureRollbackFailed);
    assert_eq!(error.status.phase, CapturePhase::RecoveryRequired);
    assert_eq!(error.status.leftovers, leftovers);
    assert_eq!(
        m.begin_attach(attach(1), true).unwrap_err().code,
        CaptureErrorCode::CaptureRecoveryRequired
    );
    assert_eq!(m.status().owner, owner());
}

#[test]
fn old_owner_or_attachment_cannot_detach_a_current_capture() {
    let mut m = machine();
    let mut lease = attached(&mut m);
    lease.owner.generation += 1;
    assert_eq!(
        m.begin_detach(detach(&m, lease), true).unwrap_err().code,
        CaptureErrorCode::CaptureLeaseMismatch
    );
    let old = m.status().lease.clone().unwrap();
    let tx = m.begin_detach(detach(&m, old.clone()), true).unwrap();
    m.complete(&tx, CaptureOutcome::Committed).unwrap();
    attached(&mut m);
    assert_eq!(
        m.begin_detach(detach(&m, old), true).unwrap_err().code,
        CaptureErrorCode::CaptureLeaseMismatch
    );
}

#[test]
fn a_transaction_is_single_use_and_cannot_complete_a_different_attempt() {
    let mut m = machine();
    let old = m.begin_attach(attach(0), true).unwrap();
    m.complete(&old, CaptureOutcome::RolledBack).unwrap_err();
    let current = m.begin_attach(attach(1), true).unwrap();
    assert_eq!(
        m.complete(&old, CaptureOutcome::Committed)
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureTransactionMismatch
    );
    assert_eq!(m.status().phase, CapturePhase::Attaching);
    m.complete(&current, CaptureOutcome::Committed).unwrap();
    assert_eq!(
        m.complete(&current, CaptureOutcome::Committed)
            .unwrap_err()
            .code,
        CaptureErrorCode::CaptureTransactionMismatch
    );
}

#[test]
fn concurrent_requests_cannot_prepare_two_capture_transactions() {
    let m = std::sync::Arc::new(std::sync::Mutex::new(machine()));
    let barrier = std::sync::Arc::new(std::sync::Barrier::new(3));
    let threads: Vec<_> = (0..2)
        .map(|_| {
            let (m, barrier) = (m.clone(), barrier.clone());
            std::thread::spawn(move || {
                barrier.wait();
                m.lock().unwrap().begin_attach(attach(0), true)
            })
        })
        .collect();
    barrier.wait();
    let results: Vec<_> = threads.into_iter().map(|t| t.join().unwrap()).collect();
    assert_eq!(results.iter().filter(|r| r.is_ok()).count(), 1);
    assert_eq!(m.lock().unwrap().status().epoch, 1);
}

#[test]
fn invalid_protocol_resource_and_stopped_core_do_not_mutate_state() {
    let mut m = machine();
    let mut req = attach(0);
    req.protocol_version = 2;
    assert_eq!(
        m.begin_attach(req, true).unwrap_err().code,
        CaptureErrorCode::CaptureProtocolUnsupported
    );
    let mut req = attach(0);
    req.spec.resource = CaptureResource::HostLease {
        lease_id: "7:raw-fd".into(),
    };
    assert_eq!(
        m.begin_attach(req, true).unwrap_err().code,
        CaptureErrorCode::CaptureInvalidResource
    );
    assert_eq!(
        m.begin_attach(attach(0), false).unwrap_err().code,
        CaptureErrorCode::CaptureNotRunning
    );
    assert_eq!(m.status().epoch, 0);
}

#[test]
fn an_uncertain_worker_cannot_later_complete_its_old_transaction() {
    let mut m = machine();
    let tx = m.begin_attach(attach(0), true).unwrap();
    let error = m.require_recovery(vec![CaptureLeftover {
        kind: ResourceKind::Worker,
        resource_id: "capture-worker-1".into(),
    }]);
    assert_eq!(error.status.phase, CapturePhase::RecoveryRequired);
    assert_eq!(
        m.complete(&tx, CaptureOutcome::Committed).unwrap_err().code,
        CaptureErrorCode::CaptureTransactionMismatch
    );
    assert_eq!(
        m.begin_attach(attach(1), true).unwrap_err().code,
        CaptureErrorCode::CaptureRecoveryRequired
    );
}
