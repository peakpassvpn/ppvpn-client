# macOS single forwarding runtime: native host candidate

This opt-in candidate imports shared-core PR #592 at
052b412cb6b047639593c8a97d9e2cd3438e8652 and frozen Sail
593286824540aac651ac2f84c6e751fb0e70a4da. It is not a release or a claim of
macOS system acceptance. No old two-runtime packet bridge is used.

## Reproducible entries

Build the helper with `cargo build --locked -p ppvpn-service --bin ppvpn-service`.
Build the UI logic with
`cargo build --locked -p ppvpn-desktop --features macos-user-runtime`.
Packaging must explicitly enable that feature; defaults remain unchanged.

The same root-owned, hardened release-signed installed helper image runs two
roles: root launchd invokes `ppvpn-service --capture-broker`; the per-user
LaunchAgent invokes `ppvpn-service --user-runtime PRIVATE_DIRECTORY LOG_DIRECTORY`.
PRIVATE_DIRECTORY must be ClientConfig.data_dir; LOG_DIRECTORY must be
ClientConfig.log_dir. Both must already exist, be canonical, owned by this uid,
and have mode 0700. The agent refuses root/setuid execution. Both roles use the
fixed installed helper bundle and the existing release signing identifiers.
There is no development signature/path bypass. An unsigned local executable
cannot impersonate an installed helper or UI.

The helper modes alone do not install/register the LaunchAgent. Signed staging,
plist installation, bootstrap/logout and update/uninstall ordering belong to
packaging. Do not replace the current user's helper or alter current routes to
exercise this candidate; use a dedicated macOS VM/test machine.

## One owner, two capture resource copies

Only the user service constructs the forwarding Engine, with Role::Shared,
physical-network bootstrap, a protected outbound socket callback and a native
CaptureHost. The UI feature excludes the embedded Engine launcher and routes
both ordinary APIs and enhanced-mode capture through the user service.
Engine runtime observation supplies the actual instance/generation identity;
presentation transport refreshes that identity across whole-runtime starts.

The enhanced adapter negotiates capabilities, applies policy to the same owner,
and calls shared attach/detach transactions. VPN-off never dispatches Engine.stop.
The user service still rejects legacy privileged Connect; there is no old root
TUN forwarding fallback. Root's old administrative socket retains authenticated
version/status/update commands, rejecting forwarding commands. Its separate
capture broker holds no Engine and accepts no profiles, proxy credentials or
arbitrary OS commands/route prefixes.

CaptureHost.prepare asks the root broker for a staged utun, then validates the
actual interface of the SCM_RIGHTS descriptor. Root retains its original; the
agent retains one independently owned fd, and Sail receives a validated duplicate
through CaptureAttachment. Descriptor integers are never serialized. Core-wire
owns capture transactions, epochs and CaptureLease; the root lease consumes that
identity rather than allocating another capture generation.

Core commits native ingress before the host publishes OS admission. Host detach
removes owned admission while retaining the original for native rollback. Root
resources survive an individual IPC disconnect and remain in an exact lease
ledger until release, expiry or process death cleanup.

## Authentication and resource ledger

The private user socket and fixed root capture socket have independent singleton
locks. Root's directory is root-owned and unwritable to users; its socket accepts
connections so non-root agents can authenticate. Every broker connection verifies
OS audit uid/pid/pid-version/audit-session, hardened release signature and exact
installed helper image before reading handshake bytes. The agent verifies the
root server equivalently. UI peers are accepted only as the installed signed UI.
HMAC uses the existing service canonical Request/Response envelope, fresh keys,
request replay/freshness limits, signed replies and an exact helper build/protocol
handshake. No auth keys or payloads are logged.

Only one root capture grant may exist system-wide. It is bound to the exact
agent audit identity, actual shared forwarder owner, attachment epoch and opaque
token. The root lease is 45 seconds, renewed by the agent every five seconds.
Expired owners cannot revive the lease. Failed cleanup quarantines the slot.
Root checks the live audit-token process independently. Signed UI lease renewal
is separate: 45 seconds without UI capture renewal detaches capture in the agent,
while mixed forwarding remains live. A root lease loss is reconciled through
native detach, not a forwarding stop.

The broker configures only the shipped core addresses/MTU, a fresh utun, IPv4 /1
routes and requested IPv6 /1 routes. It never replaces a real default route or a
real network service's DNS. DNS is a dedicated temporary SCDynamicStore key,
removed by configd when the broker session closes. Renewals verify/heal owned
routes/DNS; another interface's route is not deleted or replaced as ours.

Before route mutation, the root broker fsyncs its cleanup journal. The journal
contains exact interface/index, boot identity, settings and route intents. An
independent random /128 interface marker fences recovery against recycled utun
names. Restart recovery must match that marker/index/boot before touching an
interface. Ambiguous ownership refuses a new grant. Successful cleanup removes
owned routes/DNS and brings the interface down even if a receiver still holds an
fd; closing root's copy alone is not treated as revocation. Unexpected Darwin
ancillary truncation is a process containment failure, not a recoverable leak.

**Open descriptor/exec integration gate:** root raw-socket creation and root child
spawns are serialized. The user receiver still requires the same shared gate as
core DNS scutil spawn; a service-local lock cannot cover that core operation.
Darwin SCM_RIGHTS followed by fcntl has a proven child-inheritance window. The
minimal public-core gate patch is prepared separately for the shared-core owner;
until it is merged and consumed here, this opt-in candidate must not be released
or claimed to have completed descriptor lifetime/security acceptance.

## Physical network, ingress and process lifetime

The agent reads SystemConfiguration Global IPv4/IPv6 primary-interface state and
that interface's addresses, excluding tun/utun/ppp competitors. Outbound sockets
are bound to the physical interface with IP_BOUND_IF/IPV6_BOUND_IF, before connect.
Offline fails closed. Shared Engine receives the bootstrap before start and only
changed physical snapshots thereafter. Capture-only routes/DNS are not pushed as
network moves or generic resets. Existing core DNS-local resolves through the
physical interface; the broker's supplemental DNS does not replace its service.
Actual underlay loss can end connections; capture changes must not manufacture it.

Authenticated mixed HTTP/SOCKS, node/routed credentials and UDP belong to the user
Engine. The OS system-proxy capability is available, but its unauthenticated
listener starts disabled and follows explicit Compat requests. UI exit restores
UI-owned OS proxy settings and detaches presentation; account/profile end is an
explicit signed whole-forwarder stop. The agent shuts down on logout/termination
or fatal owner failure and reports leftovers. Its daily core log uses the existing
bounded log channel and seven-file retention.

## Runner / ApplePush ownership

No existing Swift Runner files or ApplePush callbacks are changed here. Reserve
UserRuntimeBridge.swift for agent registration/readiness and coordinate its single
bootstrap call with the mobile specialist. Keep account/auth snapshot bridging in
the existing Rust Client. ApplePush registration, APNs callback lifetime and removal
of the push agent are separate mobile acceptance; this candidate claims none of it.

## Validation and remaining acceptance

The service test uses the actual Sail runtime and core native capture coordinator,
with isolated packet transport. It holds authenticated HTTP CONNECT, SOCKS TCP and
one SOCKS UDP association through ten successful attach/detach transactions and
presentation disconnect, checking stable payloads, credentials, port and owner.
It does not open a system utun or change host routes/DNS. Actual socket binding,
FD validation, HMAC/audit identity, lease expiry/quarantine and route ownership
negatives have separate local checks.

Required before release: signed UI/agent/root staging in an isolated macOS VM;
actual utun transfer and repeated VPN on/off with held mixed TCP/UDP; capture lease
expiry, root/agent death, partial OS failures and restart journal recovery; DNS and
rule/global encrypted-node paths; sleep/wake and physical moves; multi-user and
replay/permission negatives; exact process/runtime count; then reproducible signed
app/helper/DMG size comparison. The old two-runtime size PoC is not acceptance.
