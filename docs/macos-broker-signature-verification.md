# Broker static verification and handshake deadlines

The host read timeout remains 2400ms, broker read/write 600ms, and the Core
transaction step deadline 3s. The live peer checks are unchanged: kernel audit
UID/PID/version/session, the protected exact installed path, Developer ID and
Team requirement, hardened runtime, `proc_pidpath`, then a second audit-token
process signature check. No client identity comes from its JSON frame.

## Observed cause and regression

In run 42388650018, 2098ms of a 2124ms handshake read overlapped that same
connection's static file signature validation. The corresponding 1442ms sample
in run 42388471086 had the same shape. These are observations of the slow
mechanism, not a complete trace of the older 75f3da44 timeout, which lacked
per-signature timing. Serial work from other transactions remains a separate
hypothesis; this change does not introduce a worker pool or change ledger order.

`handshake_tests` drives the production host request, broker transaction,
framing/HMAC/session checks and audit-token verification flow. Its signing/path
backend is controlled by a channel barrier; it does not claim to replace real
Apple Security API acceptance. The legacy repeated-static-check control reaches
WouldBlock/TimedOut at the actual 2400ms socket limit, before request submission
and without an occupied ledger. The warmed generation branch completes without
entering that slow validator, while retaining both live checks and path checks.
Cold-client validation precedes connect, and failed static or either live check
cannot emit an accepted handshake or allocate a grant.

Run these tests in an isolated process because the broker's actual production
role assertion checks the process-global forwarding-runtime registry:

```
cargo test --locked -p ppvpn-service --lib handshake_regression -- --ignored --test-threads=1
```

The macOS CI job explicitly runs the full service library and then this ignored
suite in a separate process. The signed 20-cycle native gate independently tests
the real Security APIs, FD transfer, routes, DNS and original held flows.

## Installation generation and safety

Only the fixed standalone `broker::IMAGE` layout can reuse a result. Its
`Contents` must contain only `MacOS`, containing only `ppvpn-service`. App or
resource-bearing bundle paths always receive a fresh full static validation.

A generation includes the requirement identifier and Team, full file SHA-256,
device/inode, ownership, mode, link count, length, modification/change timestamps,
and every protected ancestor's device/inode/ownership/mode. Directory content
length and timestamps are excluded: adding an unrelated root-owned sibling
cannot invalidate this image's proof. Directory replacement or protection
changes still invalidate it. The entire installed chain is checked against the
existing no-symlink/root-owned/no-group-or-world-write policy on every connection.

A no-follow, close-on-exec descriptor hashes every byte. Descriptor and path
identities must match before and after reading; a cache hit is confirmed by a
fresh generation read after the lookup. A miss clears the prior entry, performs
the complete existing `check_file` validation, and confirms the generation again
before publishing a result. A failure or concurrent change returns an error.
The cache cannot authenticate a different path, requirement, image, or writable
installation. Both live process signature checks still run for every connection.

This relies on the existing installer contract: all installed helper processes
must exit before their immutable image is replaced. It does not cache live peers
or skip their audit-token checks. Apple's static validation result is valid only
while code remains unmodified; the generation binds reuse to those exact bytes
and protected installation identity ([Apple API documentation](https://developer.apple.com/documentation/security/secstaticcodecheckvalidity(_:_:_:))).

The broker performs initial full validation before binding its endpoint. Agent
startup also prevalidates, and every host request ensures local prevalidation
before opening its socket. Thus a warm broker cannot consume its 600ms hello
budget while a newly starting client performs its cold local file check. Failed
prevalidation exposes no endpoint/connection, hello, authentication key or FD.
