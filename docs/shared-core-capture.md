# Shared forwarding core and optional TUN capture

Status: protocol v1 and macOS Shared native adapter on the frozen Sail dynamic
TUN candidate. Native packet-transport and Engine transaction tests pass; real
macOS broker routes/DNS/device acceptance and platform host migration are still
required. Existing platform hosts have not been migrated by this PR.

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
capability reporting. This draft pins the frozen v0.22.0-based candidate
`593286824540aac651ac2f84c6e751fb0e70a4da`; candidate CI/release pin and actual
platform acceptance remain gates before shipping. Ordinary reload's
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
`/v1/attach-capture`, `/v1/detach-capture`. Engine methods expose the same typed
requests/status. Native support requires a Shared owner or explicitly opted-in mobile Tun owner,
a native host hook and a physical-network snapshot before start. Other
configurations report unsupported. No automatic stop/start fallback exists.

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
All dynamic candidates disable automatic route setup: Sail rejects dynamic
auto_route/auto_redirect, and the host/broker owns that resource ledger.
Translation preparation does not mutate the runtime or consume a host handle;
the native adapter resolves the lease through its explicit process-local hook.
Shared startup refuses listener fallback, and profile changes requiring restart
return an error. Native capture then applies only that inbound on the existing Sail Instance,
without profile reload or runtime restart.
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


## Native host hooks and physical network

`EngineConfig::with_capture_host(CaptureHost::new(Arc<dyn CaptureHostApi>))`
registers process-local hooks. The field is skipped by serde and cannot be
injected by an IPC configuration. Exact public types are in
`ppvpn_core::capture_host`; the wire still carries opaque HostLease tokens.

`CaptureHostApi::prepare(owner, attachment_epoch, spec, settings)` verifies the authenticated broker lease,
owner, expiry and resource identity against the exact forwarding identity and
reserved attachment epoch, reserves an original resource, and returns
`Arc<dyn CaptureAttachment>`. Preparation failure must already roll back any
side effects; leftover IDs report uncertainty. `CaptureNetworkSettings` gives
MTU, IPv4/prefix, optional IPv6/prefix and DNS server addresses. Routes, original
DNS state and ownership are exclusively the host's ledger.

The attachment declares Device or Packets transport. Device supplies the exact
actual interface name and a fast, local `duplicate_device() -> OwnedFd`; it must
not perform blocking IPC there. The original remains host-owned. The core sets
CLOEXEC on its duplicate and, on macOS, reads public UTUN_OPT_IFNAME to verify
type/name. A mismatched device or a capture device used as the physical uplink
is refused. Sail's host-FD path adopts the descriptor without configuring OS
addresses/routes. Packet transport passes a bounded `CapturePacketFlow` to
`publish_packets`, using public packet APIs with no OS descriptor extraction.

Async attachment callbacks are `commit_attach`, `rollback_attach`,
`commit_detach` and `rollback_detach`, each returning `CaptureHostResult<()>`.
They must settle within their own bounds and track all late broker work if an
await is dropped; the core also bounds each step at three seconds. Broker
callbacks must not re-enter Engine lifecycle/configuration calls, since the
Engine operation lease is held. The core keeps pending resource handles alive
through recovery instead of discarding the original ledger.

Attach commits native ingress, publishes packet transport if needed, then
publishes owned OS settings. Failure detaches native capture and rolls back
host preparation; clean rollback consumes the attempt epoch. Detach removes OS
admission first, then drains native capture. Failed OS detach restores the
original ledger before reporting rolled back. An unclean/Draining native detach
cannot honestly restore capture: it becomes recovery-required with OS admission
removed and the resource ledger retained. Mixed forwarding remains alive.

Call `set_network_state(NetworkState)` on Shared before start, and thereafter
only for changes to the **physical** upstream. Like mobile host state, it
overrides Sail detection. Capture's own route/DNS notifications must not be
translated into a global network-changed/reset call or a new physical uplink.
Native capture never calls that reset. IPv6 DNS policy follows the supplied
physical state, so OS TUN changes cannot trigger a policy reload through the
desktop IPv6 probe. Real physical interface changes remain observable and may
close connections that cannot survive them; this is not globally disabled.

A controller is installed for each successful forwarding start. `stop` closes
capture admission, settles accepted work and detaches its exact active lease
before stopping forwarding. Explicit full shutdown is bounded by the existing
10-second limit and reports capture cleanup uncertainty as a leftover; a broker
must reconcile its owned ledger rather than treat a closed FD copy as revocation.
Dropped shutdown waiters do not cancel accepted native detach. An asynchronous
native failure/Draining state invalidates a still-settled Attached state on status
read, quarantining later mutations. Native callbacks never create another core.

The native Runtime tests establish authenticated HTTP CONNECT, SOCKS TCP and
SOCKS UDP first, then attach/detach three times while checking the same owner,
native ID, connection IDs and cumulative counters. Separate native Engine tests
exercise failed attach, failed detach with exact lease restoration, dirty
rollback, explicit stop and a fresh generation after restart. Host OS hooks in
those Engine tests are an in-memory ledger; these results do not certify actual
macOS route/DNS cleanup, peer authentication, FD transport or network-notification
behavior. Those remain the desktop integration/isolated-system acceptance gate.


## Mobile VPN owner and public packet transport

A mobile `Role::Tun` with an explicitly registered `capture_host` opts into the
same native coordinator, with DNS/routing policy but no startup TUN or mixed
listener. It requires physical `set_network_state` before start. A raw
`TunConfig.fd` is rejected in that mode so there is no hidden startup-FD fallback.
iOS rejects Device transport; the extension supplies `CaptureTransport::Packets`
and consumes `CapturePacketFlow` through public NEPacketTunnelFlow read/write
APIs. Android may choose its existing static public VpnService FD path (no
capture_host) or explicit dynamic Device/Packets transport; native capture
preserves its SocketProtect callback. No mobile mode creates an App Standard
forwarder. Platform host changes must opt into this API explicitly; legacy
mobile behavior is retained until that migration is integrated.

For the public packet mode, `CaptureHostApi::prepare(owner, attachment_epoch,
spec, settings)` binds a host-private token to the extension's packet flow and
its planned tunnel settings. `publish_packets` transfers the bounded queues;
`commit_attach` publishes the extension's owned network settings, and detach/
rollback callbacks reconcile exactly those settings. The supplied settings
include IPv4, optional IPv6, MTU and DNS. Mobile capture requires `spec.ipv6=true` (including the physical-no-IPv6
hand-off policy) as existing VPN behavior does; an attempt to omit it is refused
before opening native ingress or publishing host network settings. Direct App probes remain the lightweight ProfileSession API.
Capture preservation flags describe what is preserved; they do not advertise a
mobile mixed server. `local_proxy_metadata` and credential APIs remain disabled
on this mobile Tun owner.


Every dynamic owner, including explicit mobile packet/device mode, refuses a
profile reload that Sail says needs restart. It cannot silently discard the
native attachment or its OS ledger. Hosts must handle an explicit failed reload
and keep the acknowledged running profile, or perform a deliberate VPN stop/
start through the normal teardown. Existing static legacy Tun behavior is
unchanged until a host opts into native capture.


Packet hosts must generation-fence and join their own read/write callbacks as
part of detach/rollback, including an output packet already returned by recv.
Native clean covers only Sail's queues/stack/tasks, not an in-flight external
packetFlow write. A host must finish that barrier and its OS ledger transaction
before advertising capture off. FD hosts must not concurrently read the original;
nonblocking mode is shared by duplicated open-file descriptions. Transport and
actual interface identity remain fixed for the staged resource. A native ingress
that has already failed during host commit is rolled back rather than reported
as a successful attachment.


Shared endpoint data-plane bypass rules are restricted to TUN ingress, so they
cannot preempt a mixed user's fixed-node credential route or the routed user's
profile rules. Their bootstrap DNS matchers remain explicit physical-resolver
overrides in the DNS policy. Legacy translation is unchanged; Shared capture
preparation keeps these same route and DNS rules in both attached/detached plans.
