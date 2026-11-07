# macOS single forwarding runtime: integration contract (work in progress)

This is a dependency contract, not a claim that migration is complete. Baseline
service Connect launches a privileged Role::Tun engine; desktop EngineLauncher
launches a separate Role::Standard engine. Neither is the target architecture.

## Ownership

A signed per-user LaunchAgent owns one continuously running forwarding Engine.
The UI is an IPC client, not an Engine host, and does not spawn a stdio core.
The root service becomes a capture resource broker; it does not parse profiles,
run DNS policy, or serve Core API forwarding calls. Install/update operations
remain independently privileged and authenticated. Agent packaging is a build
integration dependency; do not silently fall back to an embedded UI engine.

The agent's socket must be in a verified uid-owned 0700 directory, socket 0600,
with no symlink traversal or inherited attacker-selected paths. A singleton lock
precedes bind; do not unlink a live endpoint. Both endpoints validate OS audit
uid, pid, pid-version and audit-session plus the release signing requirement and
installed image identity. Root is not accepted as the user's forwarding identity.
HMAC keys belong to the authenticated peer and service generation, expire and
renew independently of capture, and never occur in logs. Request-id replay windows
and signed replies must remain. Debug signature exceptions must not become release
policy. The current PID-only auth-session binding is insufficient for the new
broker boundary; do not reuse it as proof of this stronger identity contract.

## Shared core / engine-host / core-wire dependencies

The final native symbols are provided by shared-core PR #592 at 052b412. The following semantics remain the acceptance contract:

* A Shared user Engine can attach a host-supplied owned TUN resource and later detach
  it without stop/start/apply-profile, listener rebind, credential reset, route-mode
  change, counter reset or closure of existing local proxy streams/UDP associations.
* Attach acknowledges a capture generation only after inbound readiness. Failure
  rolls back capture resources, retaining the live local proxies. Detach is
  idempotent and leaves the same Engine identity and existing flows intact.
* Host-FD support on macOS is required. Baseline TunConfig::with_fd is mobile-only
  startup configuration; it is not a macOS dynamic attach interface. The broker
  must retain interface name/index, v4/v6 settings and route/DNS cleanup ownership;
  an fd alone is insufficient to replace Sail's current host setup and healing.
* Engine-host must expose capture state/events separately from runtime state, plus
  network changes and existing Core API calls over IPC. Network change or capture
  loss must not map to a whole-engine restart.
* Core-wire must specify bounded framed authenticated calls, event backpressure,
  cancellation and descriptor-ticket binding. Tickets identify broker generation,
  capture generation, owner session and a single-use nonce. An fd integer in JSON
  is never a process-to-process descriptor transfer.

The service-local single_runtime primitives are candidates for this integration;
shared core/wire crates are intentionally not edited by this specialist.

## Capture lease and FD feasibility

Capture alone uses a 45-second lease; the agent renews the root resource lease every five seconds. Owner
identity includes OS uid/pid/pid-version/audit-session and application session /
generation. A stale owner cannot renew or detach its successor. Failed cleanup
quarantines the slot; the broker must not grant new capture on ambiguous OS state.

Use Unix SCM_RIGHTS with CLOEXEC and exact ancillary validation, after peer and
signed descriptor-ticket validation, on a serialized connection. Darwin may create undisclosed descriptors when
recvmsg ancillary storage truncates: reserve the full XNU rights bound, reject extra fds
and close every delivered copy, and treat unexpected truncation as a process
containment/recovery event rather than assuming Linux-style automatic closure.
The sender keeps its own fd; the receiver owns a distinct fd. Closing the broker's copy does NOT
revoke the receiver's copy. Lease revocation therefore needs explicit route/DNS
cleanup (and interface down as necessary), then generation-bound detach; EOF alone
is not resource revocation. Recovery must reconstruct the broker's cleanup ledger
and reconcile owned resources before any grant.

OS utun creation, address assignment, Sail runtime adoption, route/DNS healing,
and rollback must be tested together in an isolated machine/VM before replacing
existing service Connect. They are not yet validated by descriptor-passing tests.

NetworkExtension is not assumed as a shortcut. Apple's public interface is
NEPacketTunnelProvider.packetFlow and requires the Network Extension entitlement:
https://developer.apple.com/documentation/networkextension/nepackettunnelprovider
Public packetFlow does not document exporting a raw fd. Do not use KVC/private fd
extraction as a new architecture dependency. A packet-pump alternative would need
an explicit engine interface, packet ordering/backpressure and signed deployment
validation. The existing macOS utun behavior is retained until equivalent capture
and recovery are proven. iOS lifecycle guarantees belong to the mobile specialist;
this contract does not claim a provider process survives VPN-off.

## Lifecycle and acceptance

* VPN-off: detach capture, restore owned routes/DNS; keep the agent Engine, mixed
  HTTP/SOCKS listener, port, credentials and established proxy connections.
* UI exit: stop capture and UI-owned system-proxy settings, keep the per-user
  forwarding service according to explicit agent lifetime; UI reconnection obtains
  a new authenticated session. Logout/user-session end shuts the agent down.
* Account sign-out explicitly clears account state, revokes local credentials and
  shuts down the user runtime; unlike VPN-off, it is allowed to close connections.
* Lost capture lease/root-broker failure: report capture unavailable and clean
  capture resources; keep local forwarding. Agent failure necessarily loses its
  connections; do not claim continuity through process death. Restart is explicit
  and observable, not a second concurrent runtime.
* Sleep/wake: suspend does not resurrect an expired capture grant. Reconcile OS
  resources and reacquire a new generation while preserving the agent runtime.

Isolated acceptance must hold continuous authenticated HTTP CONNECT, SOCKS TCP
and SOCKS UDP traffic through repeated capture on/off; assert runtime identity,
listener port and credentials unchanged, uninterrupted stream payload sequence,
UDP association validity, rule/global routing and local DNS behavior. Add expired
lease, concurrent grants, stale generation, PID reuse, other uid/audit-session,
unsigned/wrong installed image, replayed ticket/request, wrong/missing/extra fd,
oversized ancillary data, partial attach/cleanup, broker death and sleep/wake.
No test in this branch may install a service or modify the developer's live
system proxy/routes/DNS. Resource acceptance remains pending a dedicated VM.


## Implemented boundary in this branch

Integration now includes shared contract 94ec64d / PR #592, an explicit per-user
agent entry point, authenticated framed RPC and an opt-in desktop IPC launcher.
See [macos-runtime-integration.md](macos-runtime-integration.md) for build commands,
Runner/ApplePush ownership, notification audit and remaining native-system gates.

All capture DTOs and transaction epochs are ppvpn-core-wire's. The broker resource
lease consumes CaptureLease and has no separate transition machine. Authentication
uses the existing Request/Response JSON HMAC envelope; the earlier candidate
length-prefixed canonical form has been removed. Full OS-peer binding, bounded
replay state and CSPRNG session issuance are retained. Release peer verification
checks audit uid/pid-version/session, hardened signature and installed image.

FD transport still reserves the Darwin rights bound and returns explicit
ContainmentRequired on unexpected truncation. Native utun adoption, root resource
provider and route/DNS rollback are not claimed from Unix socket tests. The pinned
adapter explicitly refuses dynamic capture without creating a privileged core.

The installed/default product is not changed by the opt-in entry/feature. No test
in this branch installs it or changes live network settings. Whole-product
acceptance requires the signed packaging and accepted native capture adapter.

The native candidate is integrated against frozen Sail 5932868; see [macOS integration](macos-runtime-integration.md) for exact entries, implemented ledger/identity behavior and system tests still required. Local packet-transport tests are not system-route acceptance.
