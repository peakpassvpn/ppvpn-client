# macOS user-runtime integration on shared capture protocol v1

Status: opt-in build path, not release-ready native TUN acceptance. The source
base includes shared contract commit 94ec64d from PR #592. No unaccepted Sail
patch or packet bridge is included. The fixed Sail v0.22.0 still advertises
CAPTURE_UNSUPPORTED; no second privileged forwarding Engine is used as fallback.

## Build / run boundaries

Build the service with `cargo build --locked -p ppvpn-service --bin ppvpn-service`.
The privileged helper also has `ppvpn-service --capture-broker`: startup refuses
non-root callers and admission blocks all legacy forwarding lifecycle/Core API
commands after authentication, retaining administrative version/status/update
handling. It skips the legacy forwarding watchdog and network cleanup path.
Native resource grants stay CAPTURE_UNSUPPORTED until the accepted provider is
installed; this role cannot use the old privileged Engine as a fallback.

The user service's explicit invocation is `ppvpn-service --user-runtime PRIVATE_DIRECTORY LOG_DIRECTORY`.
It refuses root, setuid execution, non-private directories, writable or unsigned
installation images and unexpected executable paths. The production identity is
still the signed installed helper bundle; this is not an unsigned development
bypass. A test VM must stage/sign that image and the UI. Do not replace the live
installed helper to exercise this entry point.

Build the desktop opt-in path with
`cargo build --locked -p ppvpn-desktop --features macos-user-runtime`.
That feature excludes the embedded Engine launcher from production UI compilation.
It connects only to the private runtime.sock in ClientConfig.data_dir and checks
the server's OS audit uid/pid-version, signing requirement and installed image
before accepting handshake keys. Missing agent or unsupported capture is an
explicit error, not a fallback to an embedded or privileged forwarding Engine.
No packaging script or default release feature is changed here. LOG_DIRECTORY
must match ClientConfig.log_dir and be private to this uid; the agent writes
UTC daily ppvpn-core.YYYY-MM-DD.log with seven-file retention via a bounded
engine log channel, keeping filesystem writes off the forwarding executor.

The user agent creates one Standard Engine, independently of UI socket lifetime.
Signed UI calls use the existing service Request/Response HMAC envelope; there is
no second binary canonical form. Capture DTOs, identities and transaction epochs
come from ppvpn-core-wire. The OS-resource lease consumes CaptureLease rather than
allocating a duplicate capture generation. It remains a broker resource lease,
not another capture transition authority.

The user gateway refuses legacy privileged Connect with CAPTURE_UNSUPPORTED and
other broker commands with BROKER_COMMAND_REQUIRED. The actual native broker
resource provider and user-agent capture driver cannot be enabled until their
Sail interfaces and rollback behavior are accepted. Root service defaults remain legacy for installed builds; new packaging must
select --capture-broker explicitly. The opt-in UI never calls that endpoint as
a forwarding fallback.

## Lifetime / ingress

UI exit detaches presentation, not the daemon. Its generic stop request is not
forwarded to Engine.stop. Explicit account/profile end sends a signed owner_reason
through the same existing Core API envelope; this stops ingress and existing
sessions. It is distinct from VPN-off. The agent alone shuts its Engine down on
its own exit/fatal state, and reports shutdown leftovers as failure.

The agent's authenticated mixed listener stays independent of capture. Its
unauthenticated OS system-proxy listener starts disabled and follows explicit
Compat enable/disable requests; it is not expanded into an always-on unauthenticated
cross-user proxy. UI exit restores its own OS settings. The requirement to retain
mixed connections is not used to justify expanding unauthenticated ingress.
Profile-switch/credential lifecycle remains part of whole-client acceptance.
Agent failure necessarily loses its connections.

## Network notification audit at locked Sail 7513c151

1. Desktop Client.network_changed calls Enhanced.network_changed, which wakes a
   health/recovery check; it does not directly stop Standard. The legacy enhanced
   recovery path can replace its privileged Engine, so it is not a live-capture
   implementation and is not used as a fallback in the opt-in path.
2. Standard Engine.set_network_state explicitly rejects macOS (mobile push API).
   The remote UI launcher deliberately does not synthesize a mobile platform or
   send generic host network resets. The service's Sail monitor owns detection.
3. Sail platform/route_socket.rs default_route selects actual unscoped IPv4 default
   routes; its comment and implementation exclude TUN split routes. Existing
   protocol/tun/auto_route.rs uses more-specific macOS routes (1/8 through 128/1),
   preserving the real default route. The broker MUST preserve this behavior.
4. Sail net/network.rs NetworkState.moved_to treats default interface/index,
   gateway/kind changes and address loss as moves. NetworkChange.closes closes
   DefaultRoute flows on those moves, and even an unchanged generic reset may
   close flows. A mere added interface/address is not necessarily a move. Thus
   capture attach/detach must not manufacture default-route/reset events.
5. Retaining the default route is necessary but not yet a runtime proof. The new
   external-FD adapter must preserve socket egress/protection, underlay DNS,
   topology events, capture-only cleanup and sleep/wake handling. Test live TCP
   and SOCKS UDP associations through actual capture transactions before claiming
   preservation. Real underlay loss and agent death are not promised lossless.

These findings are a source audit; no live OS routes/DNS/proxy settings were
changed. Fake broker/socket tests are not a substitute for actual TUN acceptance.

## Runner / ApplePush ownership

This branch does not replace MainFlutterWindow.swift or AppDelegate.swift. Build
scripts and LaunchAgent plist ownership stay with the build specialist. Reserve
new UserRuntimeBridge.swift for desktop user-agent readiness/registration; its
single initialization call belongs beside existing Runner bootstrap, coordinated
with the mobile/push specialist. ApplePush owns native push callbacks and the
future ApplePush receiver invocation. Do not remove or overwrite those callbacks
when adding user-runtime bootstrap. Account binding/refresh/logout remains through
the stable existing Rust Client/auth snapshot bridge; no second native account
store or callback is introduced here. Native APNs equivalence/removal of the push
agent is the mobile specialist's acceptance, independent of the forwarding agent.

## Remaining real-system gates

- Accepted Sail dynamic TUN plus host FD ownership/route/DNS rollback API and
  core adapter; no NeedsRestart or whole-runtime fallback.
- Minimal privileged native resource provider, exact signed agent role admission,
  single-use descriptor-ticket binding, expiry cleanup and restart ledger recovery.
- Build-owned signed LaunchAgent packaging, per-user private directory creation,
  logout/update/uninstall ordering; test-only isolated VM staging.
- Actual signed UI-to-agent-to-broker tests holding HTTP CONNECT, SOCKS TCP/UDP,
  ports, credentials and payload sequences stable across repeated capture on/off.
- Rule/global DNS correctness, sleep/wake, capture loss, broker death, partial
  cleanup and permission/replay negatives. Build volume comparison follows this
  functional acceptance; the old two-runtime size PoC is not final evidence.


## Evidence scope

The real mixed fixture uses one Standard Engine, authenticated HTTP CONNECT and
SOCKS TCP/UDP to localhost-only echo endpoints. It checks unauthenticated HTTP
rejection, ten refused capture requests, continuous TCP payload sequence and the
same UDP association, proxy credentials/port and owner identity, followed by
presentation-gateway detach. Its route policy is rules/direct for these local
targets; encrypted-node/global mode, actual TUN changes and system DNS are NOT
covered. Do not present it as VPN toggling acceptance.
