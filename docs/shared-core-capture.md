# Shared forwarding core and optional TUN capture

Status: protocol v1 and fake-driver implementation; native Sail support pending. Existing platform hosts still use separate Standard/Tun
instances. This document does not claim that those hosts have migrated.

## Invariants

One active forwarding runtime owns the dispatcher, mixed HTTP/SOCKS listeners,
authentication, TCP connections, SOCKS UDP associations, selected nodes, DNS and
routing policy, and cumulative statistics. Attaching or detaching capture must
not stop/start that runtime, replace its dispatcher, or rebuild mixed listeners.
Capture-only sessions may end on detach; existing mixed sessions must survive.

## Versioned interface

`ppvpn-core-wire` defines capture protocol v1: capabilities, capture status,
attach/detach requests, expected state epoch, ownership tokens, results and
stable error codes. Existing Core API v1 remains compatible. New capture calls
are separately capability-gated; absence of support is an explicit refusal.

The native resource resolver supplies a process-local TUN handle. Wire messages
never transfer ownership by serializing a raw descriptor or pointer. The process
that opened the original handle keeps it; the runtime owns its duplicate until
capture has detached and cleanup has completed.

## Transactions

Attach prepares capture-specific device, routes and DNS resources, then commits
publication to the existing dispatcher. Failure rolls back only those resources.
Detach removes capture admission and owned routes/DNS/device resources while
leaving mixed listeners and sessions intact. A failed detach restores the prior
attached state; failed rollback exposes a recovery-required state and leftovers,
blocks further capture mutations, and keeps the forwarding runtime alive.
Operations are serialized and compare the caller's expected epoch. Stop and
shutdown are separate, explicit forwarding-core operations. No capture failure
falls back to restarting the core or creating a second runtime.

## Platform boundaries and dependencies

macOS: the service is the sole forwarding owner; UI and other clients control it
through authenticated IPC. Disconnecting VPN detaches capture. The service must
remain alive while local proxy sessions are in use. Platform IPC/installation
and Runner changes belong to the desktop specialist.

iOS: Network Extension owns the active forwarding core while VPN is active.
A UI process cannot migrate live TCP, UDP or DNS state into/out of the extension.
Stopping the extension ends its listeners and sessions. A design requiring a
proxy while the extension is absent must explicitly stop the former owner before
starting another; uninterrupted cross-process migration is unsupported. The
mobile specialist owns provider lifecycle, network settings and handle supply.

Sail dependency: native capture attach/detach on the same embedded instance,
transactional rollback, resource-scoped cleanup, mixed TCP/UDP preservation and
capability reporting. The pinned Sail v0.22.0 adapter advertises unsupported
until those guarantees are implemented and verified; ordinary reload's
`needs_restart` is never converted into a silent restart.

## Acceptance

First lock protocol and transaction behavior with deterministic fake runtime
tests, including failed attach/detach, rollback failure, stale epoch, concurrent
requests and preservation of forwarding identity/connections/selection/stats.
Then test actual Sail with established HTTP/SOCKS TCP and UDP sessions through
attach/detach on isolated macOS, plus iOS provider lifetime and process-boundary
limits. Fake success is not platform acceptance.

## Published Rust/JSON surface (v1)

DTO-only dependency: `ppvpn-core-wire` (serde, no Sail). Host coordinator:
`ppvpn-core-wire` feature `driver`, exporting `driver::CaptureDriver` and
`driver::CaptureController`. `ppvpn-core` re-exports the protocol as `capture`.

- `CaptureCapabilities`: `protocol_version`, `live_attach`, `live_detach`,
  `transactional_rollback`, `preserves` (dispatcher, mixed TCP, mixed UDP, DNS
  state, selection, counters), `cross_process_migration=false`.
- `AttachCapture`: `protocol_version=1`, `expected_epoch`, `spec` containing
  `resource={kind:managed,resource_id}` or `{kind:host_lease,lease_id}`, plus
  `ipv6`. Tokens resolve in the owner's process; raw `fd` fields are rejected.
- `DetachCapture`: version, expected epoch, and exact `CaptureLease` returned
  by attach (`owner={instance,generation}`, `attachment_epoch`).
- `CaptureStatus`: version, owner, epoch, phase, optional lease/spec, leftovers.
  Phase is detached/attaching/attached/detaching/recovery_required.
- `CaptureError`: stable uppercase `code` and the resulting status. Core API
  transports map it to their usual error envelope plus optional `capture_status`;
  legacy error envelopes omit that field. Capability
  refusal and malformed/version/stale requests do not begin native work.

Core API paths are `/v1/get-capture-capabilities`, `/v1/get-capture-status`,
`/v1/attach-capture`, `/v1/detach-capture`. The pinned legacy adapter reports
unsupported and refuses mutations; there is no automatic stop/start fallback.
The reusable controller is independently testable before the Engine adapter.

An accepted transaction reserves its next epoch even when rolled back, so old
requests cannot be replayed. The controller checks owner identity before and
after native work. Driver panic or owner replacement is recovery-required,
never a successful rollback. Cancelled queued calls do not mutate later;
accepted calls finish settlement even if the IPC caller disappears. Hosts poll
status after transport uncertainty. Driver code must implement bounded native
work, hold the forwarder's lifetime/configuration lease, and report cleanup
uncertainty explicitly. Forwarder shutdown is not a capture detach.

The state machine/coordinator does not implement OS device setup or guarantee
native socket preservation by itself. The platform and Sail acceptance tests
remain required before advertising these capabilities to users.


Before terminating the forwarding owner, call `CaptureController::quiesce()`:
it rejects new/queued capture work with `CAPTURE_OWNER_STOPPING` and waits for
accepted work to settle. It does not stop forwarding or detach capture. Only
then may the platform perform its explicit forwarding shutdown and complete
cleanup. A subsequent owner/run gets a new identity and controller.

## Native acceptance matrix

| Case | Required evidence |
| --- | --- |
| macOS attach while mixed traffic runs | Same runtime/dispatcher identity; existing authenticated HTTP/CONNECT and SOCKS TCP continue, SOCKS UDP association continues, selection and counters preserved |
| macOS VPN off | Only capture resources detach; mixed listeners and established proxy sessions stay alive; no second forwarding runtime |
| Failed attach | Prior DNS/routes/device state restored; mixed traffic continues; no staged leftovers |
| Failed detach | Prior capture lease/settings restored; rollback uncertainty explicitly recovery-required |
| Service shutdown | Quiesce first, settle accepted work, then explicit full cleanup; never interpret UI disconnect as forwarding shutdown |
| iOS extension stop | Sessions end with their owning process; no promise of migration into the UI, no concurrent replacement core |
| Authentication/routing | Existing per-node and routed credentials, profile routing and UDP semantics unchanged |

The default unit-test graph includes core-wire with its driver tests. They use
only synthetic resources and do not open TUN or change system routes/DNS.


## Lock and lifetime order for the native adapter

Capture coordination takes its operation lock, then the Engine operation lease,
then Sail's capture transaction lock. Native attach/detach must hold the Engine
lease through rollback, preventing a concurrent profile/routing apply or full
shutdown from changing the resources underneath it. Forwarding shutdown must
quiesce capture before acquiring the Engine operation lease; reversing this
order would deadlock a transaction waiting for that lease. Status reads take
only the short snapshot mutex. No OS cleanup executes under that snapshot lock.

This initial interface change does not add `Role::Shared`, merge the current
role-dependent DNS/translation paths, remove TUN restart checks, or wire the
native capture driver into Engine. Those changes require Sail's same-instance
capture primitives and preservation acceptance. Platform hosts must keep the
new path gated off until that integration exists, rather than launching a
legacy second core as an undocumented fallback.
