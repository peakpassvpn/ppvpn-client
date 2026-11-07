# macOS single forwarding runtime: native host candidate

This opt-in candidate imports shared-core PR #592 at
38562690393327564143b92a163888443fe5d9e5 and frozen Sail
593286824540aac651ac2f84c6e751fb0e70a4da. It is not a release or a claim of
macOS system acceptance. No old two-runtime packet bridge is used.

## Reproducible entries

Build the helper with `cargo build --locked -p ppvpn-service --features macos-user-runtime --bins`.
Build the UI logic with
`cargo build --locked -p ppvpn-desktop --features macos-user-runtime`.
Packaging must explicitly enable that feature; defaults remain unchanged.

The same root-owned, hardened release-signed installed helper image runs two
roles: root launchd invokes `ppvpn-service --capture-broker`; the per-user
LaunchAgent invokes `ppvpn-service --user-runtime-login`.
The login entry derives the OS home using getpwuid_r and chooses the default
private data/log directories. The explicit
`--user-runtime PRIVATE_DIRECTORY LOG_DIRECTORY` entry remains available.
PRIVATE_DIRECTORY must be ClientConfig.data_dir; LOG_DIRECTORY must be
ClientConfig.log_dir. Both must already exist, be canonical, owned by this uid,
and have mode 0700. The agent refuses root/setuid execution. Both roles use the
fixed installed helper bundle and the existing release signing identifiers.
A single-runtime helper with no role arguments refuses startup instead of
falling back to the legacy root forwarding entry. There is no development signature/path bypass. An unsigned local executable
cannot impersonate an installed helper or UI.

The feature-enabled install-service generates the root daemon's explicit
`--capture-broker` argument and atomically installs the global user job at
`/Library/LaunchAgents/com.peakpassvpn.ppvpn.user-runtime.plist`. Its ProgramArguments
refer to the fixed root helper with `--user-runtime-login`; Aqua domains receive
one unprivileged job each. It runs at login, restarts after unsuccessful exits,
and permits 75 seconds for ordered shutdown. Install/update first boot out user
jobs, then stop root, replace the validated helper, start root, and bootstrap
existing GUI domains. Uninstall stops/removes the user job before removing root.
Packaging must actually invoke this signed installer on initial installation;
merely embedding it in the App does not provision either role.

Installation ownership contract for the packaging owner:

| Location | Owner and permissions | Lifetime |
| --- | --- | --- |
| `/Applications/PPVPN.app` | root; no world/group writable descendants | Single-runtime PKG must retain root ownership; release signature and exact process path required |
| `/Library/PrivilegedHelperTools/com.peakpassvpn.ppvpn.service.bundle` | root; non-writable to users; executable 0555 (root-owned and executable by user agents) | Immutable shared image for broker and per-user role; atomic replacement |
| `/Library/LaunchAgents/com.peakpassvpn.ppvpn.user-runtime.plist` | root 0644, root non-writable ancestors | Installer-managed global user job, removed on uninstall |
| User `Library/Application Support/PPVPN` and `Library/Logs/PPVPN` | connecting uid 0700, canonical path, no symlink ancestors | Created by login role after self-validation; account data survives helper uninstall |
| Root broker directory/journal/socket | root protected directory; authenticated socket admission | Independent root singleton, lease and crash recovery |

`/Applications` alone may retain Apple's root:admin group-write permission;
helper ancestors never receive that exception. Both UI and helper must be root-owned; a user-owned App is rejected, even if signed.
Packaging must suppress the legacy postinstall chown in this mode. Helpers and UI must share the
`macos-user-runtime` build mode; the build hash is mode-salted, and `--build-info`
reports the mode without starting a runtime. Signing/packaging must enable the
mode for the Rust UI bridge, Swift FFI framework, and service/installer together.
The legacy mode retains its existing ownership policy. ApplePush continues to
have its existing separate responsibility. Do not replace the current user's helper or alter current routes to
exercise this candidate; use a dedicated macOS VM/test machine.

## One owner, two capture resource copies

Only the user service constructs the forwarding Engine, with Role::Shared,
physical-network bootstrap, destination-aware Sail physical binding and a native
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
ledger until release, expiry or process death cleanup. Final attachment release
settles before the detach transaction completes, preventing an immediate next
attach from racing a queued release of the old disarmed grant.

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
native detach, not a forwarding stop. While root is restarting, the monitor
waits for its authenticated signed readiness response before beginning a core
detach; root completes journal recovery before accepting this probe. A missing
channel cannot be treated as OS cleanup success, and readiness never renews a
lease. Real partial cleanup errors remain quarantined/RecoveryRequired.

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

The macOS core exports one process-wide `capture_host::fd_spawn_guard`. Core DNS
holds it only through child spawn; host raw socket creation and SCM_RIGHTS receive
hold it until FD_CLOEXEC is set. Root child spawns use the same API. The shared
lock closes the demonstrated receive/fcntl child-inheritance window. It must not
be replaced by a service-local mutex or held while waiting for child exit.

## Physical network, ingress and process lifetime

The agent reads SystemConfiguration Global IPv4/IPv6 primary-interface state and
that interface's addresses, excluding tun/utun/ppp competitors. Sail auto_detect_interface binds non-loopback outbound sockets to that physical
view with IP_BOUND_IF/IPV6_BOUND_IF; loopback stays on loopback, including core
DNS-local. A blanket fd-only host callback would incorrectly bind those local
sockets to en0 and is intentionally absent. Offline fails closed. Shared Engine receives the bootstrap before start and only
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

No existing Swift Runner files or ApplePush callbacks are changed here. The global
LaunchAgent uses the existing installer lifecycle and needs no competing Runner
registration callback. Keep account/auth snapshot bridging in
the existing Rust Client. ApplePush registration, APNs callback lifetime and removal
of the push agent are separate mobile acceptance; this candidate claims none of it.

## Validation and remaining acceptance

The service test uses the actual Sail runtime and core native capture coordinator,
with isolated packet transport. It holds authenticated HTTP CONNECT, SOCKS TCP and
one SOCKS UDP association through twenty successful attach/detach transactions and
presentation disconnect, checking stable payloads, credentials, port and owner.
It does not open a system utun or change host routes/DNS. Physical-view/loopback binding,
FD validation, HMAC/audit identity, lease expiry/quarantine and route ownership
negatives have separate local checks.

Required before release: signed UI/agent/root staging in an isolated macOS VM;
actual utun transfer and repeated VPN on/off with held mixed TCP/UDP; capture lease
expiry, root/agent death, partial OS failures and restart journal recovery; DNS and
rule/global encrypted-node paths; sleep/wake and physical moves; multi-user and
replay/permission negatives; exact process/runtime count; then reproducible signed
app/helper/DMG size comparison. The old two-runtime size PoC is not acceptance.


## Exact-source isolated native harness

`macos-capture-acceptance` enables an additional `--capture-acceptance` role and
is rejected by `tools/release-features-check.sh`; shipping artifacts must not
include it. It reuses EngineForwarder, the actual NativeHost and authenticated
root broker in the exact installed immutable signed helper image. It refuses
non-GitHub-hosted execution and requires a root-owned isolated-runner marker.
The normal login job is stopped before this sole forwarding owner starts.

The manual CI input `macos_capture_artifact` downloads an unpublished draft
`runtime-qa-<sha>` artifact, verifies its source SHA, file hashes and actual Apple
Developer ID signatures, and invokes `tools/desktop/macos-capture-acceptance.sh`.
No signing requirement is weakened for CI. The script verifies the generated
root/user job roles, then exercises twenty real utun attach/detach cycles with held
authenticated HTTP CONNECT, SOCKS TCP and SOCKS UDP, plus a distinct per-node
SOCKS TCP stream through a real separate loopback SS 2022 fixture. Its decoded
upstream and target TCP accept counters must stay exactly one; credentials and
payloads are checked after every attach/detach and presentation disconnect.
The Go fixture uses the existing pinned sing-box dependencies and shared Go
caches, contains no Sail forwarding owner, and is included in the artifact's
hash manifest. Per-node HTTP CONNECT and UDP are not claimed by this probe. Ordinary OS UDP DNS requests
to the tunnel DNS and ordinary OS HTTP requests to a public IP verify native
capture ingress and physical egress. Signed broker replies report an empty real
Sail registry; the forwarding process checks exactly one live runtime and stable
owner/credentials. The script records route/DNS snapshots and uninstalls its own
fresh installation on exit. It refuses a pre-existing installation.

This harness does not establish GUI, encrypted-node/global policy, sleep/wake,
physical move, multi-user, root crash or partial-failure acceptance. Its results
must be reported separately from shipping-binary and full-App acceptance.
