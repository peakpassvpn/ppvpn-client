# Shared forwarding core and optional TUN capture

Status: protocol v1, fake-driver implementation, macOS Shared role and runtime
observation; native Sail capture support pending. Existing platform hosts still
use separate Standard/Tun instances. This document does not claim that those hosts have migrated.

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

macOS: a signed per-user LaunchAgent is the sole forwarding owner. The UI uses
authenticated IPC; the root service only brokers privileged capture resources
and never parses profiles or starts a forwarding runtime. Disconnecting VPN or
exiting the UI detaches capture and clears owned system proxy settings, while
the user agent and established mixed sessions stay alive. Platform IPC,
installation, FD validation/transport and Runner changes belong to the desktop
specialist. Broker handle closure alone cannot revoke the agent's duplicate;
lease loss must remove owned routes/DNS and explicitly detach capture.

Mobile product boundary: iOS and Android remove their local HTTP/SOCKS server,
proxy credentials/ports and app-side Standard forwarding instance. PacketTunnel
(iOS) or VpnService (Android) is the unique active forwarding owner while VPN is
on. The app retains validated profile/selection/pins and direct entrance probes
without creating another forwarding engine. iOS cannot migrate live sessions
across app/extension processes; Android removal is a product choice, not that
same OS restriction. Socket protection, physical-network switching, revoke and
foreground-notification lifecycle remain Android host responsibilities. Traffic,
connection logs and proxied availability while on come from the VPN owner; off,
the host may display explicitly saved snapshots and must not fabricate live
traffic or silently start another engine. Mobile platform and bridge changes
belong to the mobile specialist; APNs work does not replace Android push.

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


## Shared role and runtime observation

`Role::Shared` is currently accepted only for macOS with an authenticated mixed
listener configured. Its initial start has mixed listeners and the full DNS and
routing policy, with no TUN inbound. Local DNS remains owned by the forwarder.
Internal capture preparation creates a candidate with the same profile,
selection, credentials, DNS, rules and outbounds, changing only capture ingress.
A host-lease candidate disables automatic route setup because the broker owns
that resource ledger. Preparation does not mutate the runtime or consume a host
handle; the frozen native adapter must resolve and adopt the lease separately.
Shared startup refuses listener fallback, and profile changes requiring restart
return an error. These guards do not implement native attach/detach by themselves.
Legacy Standard/Tun translation is retained; mobile hosts use Tun as their only
forwarding owner and the lightweight control API below in the app.

`/v1/get-runtime-observation` exposes a best-effort `RuntimeObservation`:
`owner={instance,generation}`, `backend`, `native_runtime_id`, `state`, `running`,
and `active_runtime_ids`. The instance nonce is private per Runtime object;
generation counts successful starts, starting at zero before the first start,
and stays constant on reload. Sail IDs and the running registry come directly
from Sail. They identify live instances within this process only, and IDs may be
reused after an object is destroyed. Fake tests explicitly report backend `fake`
with no native ID or registry. A snapshot can race lifecycle operations; native
capture transactions must hold the Engine operation lease. The observation is
not authorization and does not prove there is no forwarding process elsewhere.

The following identities have separate scopes and must not be substituted:

| Value | Meaning |
| --- | --- |
| Forwarder instance nonce + generation | Runtime object and its successful start; survives reload |
| Native Sail runtime ID | Actual process-local Sail instance; may be reused after destruction |
| Capture status epoch | Monotonic accepted attach/detach attempt, including rolled-back attempts |
| Capture lease attachment epoch | Exact successful attachment being detached; bound to forwarder owner |
| Broker service/session generation | Authentication incarnation and audit-bound owner; invalidates stale grants |
| Broker capture resource ID and expiry | Broker resource ledger and its independently renewed lifetime |

`HostLease.lease_id` is an opaque resolver token bound to the authenticated broker
owner and resource; it is not a raw descriptor, a capture epoch, or a runtime
start generation. Broker renewals do not change the forwarder generation or
capture epoch. Broker-side idempotent cleanup does not relax core detach's exact
owner/attachment/expected-epoch checks; after an uncertain reply, fetch status
and reconcile the existing lease instead of replaying with invented epochs.
The macOS candidate uses a 45-second broker lease renewed every 15 seconds;
that timer is independent of IPC credential expiry and the capture transaction.
