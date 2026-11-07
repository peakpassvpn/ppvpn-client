Apple push receiver and iOS entry integration

Scope
  ApplePushState.swift is shared Foundation-only lifecycle/payload code.
  ios/Runner/ApplePush.swift and macos/Runner/ApplePush.swift are independent
  native adapters. The iOS bridge is explicitly included by ci-build.sh and
  starts at native launch before Flutter attaches. macOS entry remains with
  the desktop owner. No existing Agent is removed or changed; no default
  build enables APNs. Authenticated HTTP effects still require the backend
  contract implementation.

Integration boundaries
  Include the common state and matching receiver in the App's target, NOT
  PacketTunnel. Keep a strong receiver reference for the App's lifetime.
  Install it by the end of launch and forward native success/failure token
  callbacks. macOS AppDelegate/window integration and packaging remain with
  their respective owners; no existing macOS Runner file is replaced here.
  Attach register/unregister/delivery handlers to receiver.state. Supply the
  authenticated stable account id via setAccount, never a display name.
  A null id means completed anonymous restoration or explicit sign-out.
  Initial restoration remains pending until the owner is known.

Environment and rollout
  PPVPNApplePushEnabled must explicitly be true. macOS reads the actual
  signed com.apple.developer.aps-environment using the public macOS API.
  On iOS, packaging must derive PPVPNAPNsEntitlementEnvironment from the
  actual aps-environment entitlement/profile and verify equality AFTER
  archive export/re-signing. development maps to sandbox; production maps
  to production; missing/unknown values disable registration.
  Do not infer environment from DEBUG or receipt sandbox: TestFlight APNs
  uses production. Do not use private entitlement-reading APIs on iOS.

Backend/client transport contract required before activation
  This code does not silently extend the existing token-only device API.
  A registration needs platform (ios/macos), bundle_id (APNs topic), token,
  explicit APNs environment, authenticated account and a stable installation
  identity. request and accountGeneration are LOCAL completion identities;
  they are NOT authentication or anti-replay credentials.
  Server identity must distinguish environment+topic+token and atomically
  associate it with the authenticated user. Define reassignment and stale
  registration protection before enabling transport. Never trust an account
  id received over the network as authority to bind a token to another user.
  Serialize registration/rotation/unregister with auth logout while the old
  bearer is usable. Cancel/await in-flight operations before dropping auth;
  rejecting a late local acknowledge does NOT revoke a late server write.
  Retire old account/token registrations, process APNs invalid-token errors,
  retry transient failures, and ensure queued retries cannot resurrect logout.
  Do not create credentials, assume an unsupported environment field is
  honored by an old server, or send unsolicited test pushes.

Delivery contract
  extra is an object containing stable delivery_id, optional account_id,
  and positive signed-64-bit message_id. Only ids cross to the client.
  No payload body or URL is rendered as trusted inbox content. Missing or
  malformed message ids mean inbox lookup rather than arbitrary navigation.
  Current auth must authorize every fetch and discard stale asynchronous
  responses. Wrong-account tagged events are dropped. Foreground unscoped
  legacy payloads cannot directly display their bodies.
  Delivery/tap dedupe are separate bounded process-local histories; backend
  dedupe is still needed. Request identifiers are only fallback identifiers.
  Silent background payloads without a delivery_id have no cross-callback
  dedupe guarantee. A bounded cold-launch mailbox survives initial auth and
  bridge startup, but explicit logout/account replacement discards it.
  iOS receiveBackground calls the optional authenticated fetch handler only
  after account/dedupe acceptance. The normal deliver callback is an event
  hint; it must not launch a duplicate fetch alongside that handler.
  Completion is at most once; a 25-second timer schedules failure if the
  handler never completes. This is not permission to keep the app resident.

Agent retirement acceptance gate
  Validate a new signed isolated package, exact profile/team/topic/env,
  real foreground/background/terminated OS alert delivery, click after cold
  launch, token rotation, account switch/logout, retries, server rejections
  and offline reconciliation. Get explicit authorization before any real
  push or replacing an installed package. Inspect old/new duplicate display.
  Only then remove the corresponding platform Agent through coordinated
  packaging/AppDelegate changes. Silent push is best effort, not a daemon.

Offline validation
  apps/flutter/checks/apple/run.sh compiles the real shared state, runs its
  regression cases and typechecks both native adapters against real SDKs.
  No app is installed/launched and no OS APNs registration is invoked.
  It does not replace signed-package, Flutter integration or device delivery
  acceptance. The test owns and cleans only its unique temporary Swift cache.
