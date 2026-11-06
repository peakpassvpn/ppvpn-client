# Apple single-active-core build and measurement contract

Status: preparation only. The implementation commits and final candidate measurements are pending.
The functional baseline is `1ddf0a168610172773fe3f479133ecbe5acf932f`
(client 0.7.4, Sail v0.22.0 at `7513c15139e3c8cddb59952d93a8f8b59ec934f1`).
Packaging core code once is insufficient: the runtime acceptance gate is exactly one active
forwarding core. The earlier UI stdio child plus privileged TUN PoC has two runtime roles and
cannot satisfy that gate, even if it produces a smaller installer.

## Build matrix

Build one large target at a time. Keep the injected Go caches and existing Cargo caches;
do not remove caches to gain disk space. Use each checkout's own build output and record
free space before starting. If there is insufficient space, stop before compiling rather
than deleting other checkouts or artifacts.

| Target | Baseline recipe | Candidate gate | Measurement boundary |
| --- | --- | --- | --- |
| macOS arm64 app | Flutter release; arm64 bridge release; UniFFI release + `strip -x`; service profile (`s`, fat LTO, one codegen unit, strip=true) | Core runtime commit + desktop integration commit, generated consumers and packaging handoff agreed | Regular file bytes for the whole app, including login item, helpers, frameworks and resources; no symlink dereferencing |
| macOS arm64 DMG/PKG | Same complete app, ad-hoc signing, no notarization; HFS+ UDZO zlib-level=9 and pkgbuild | Same recipe and signing for both measurements | Actual installer bytes and SHA-256; no mounting or installation during measurement |
| macOS x86_64 | Same supported architecture and optimization recipe | Runtime/desktop APIs and native helpers still compile for Intel | Separate row; do not compare Intel or universal output against arm64 |
| iOS device arm64 | Rust bridge release; tunnel `mobile` profile (thin LTO, one codegen unit); Release Runner + PacketTunnel; iOS 15 minimum | Mobile implementation commit + runtime transport commit; system lifecycle boundary documented | App and extension executable bytes, each embedded shared library, resources; record xcarchive separately from its Products application |
| iOS simulator arm64 | Existing debug simulator smoke recipe | ABI/host integration smoke only | Diagnostic row; never compare its debug bytes with device Release |
| iOS exported IPA | Export baseline/candidate archives under identical export, signing and thinning options | Authorized usable distribution profiles; preserve exact ExportOptions and export logs | Actual compressed IPA bytes plus uncompressed Payload app; no upload |
| iOS device download | Matching device model/OS, architecture, thinning settings and delivery report | Actual App Thinning report / device download evidence | Report Apple's device-specific download/install values separately; an IPA or xcarchive is not this metric |

Before each new build, preserve source commit, source changes, Cargo.lock hash, Sail revision,
Flutter/Dart/Rust/Xcode/SDK versions, enabled crate features, Rust flags, deployment target,
architectures, build number and signing/strip settings. Record generated-source changes
separately from authored changes. A reference checkout containing generated files or device
diagnostics is not a clean reproducible source snapshot.

The conditions JSON is a template. Its unknown historical toolchain fields are deliberately
`null`; the comparator rejects incomplete conditions. Pin and record the actual toolchains
for a fresh baseline and candidate before using the comparison as final evidence. Source
commit arguments are provenance supplied by the caller; the inventory cannot prove how a
binary was compiled.

## Current packaging entry points

- `apps/flutter/scripts/ci-build.sh`: Flutter release shell, FRB generation and platform bridge.
- `crates/desktop/scripts/build-apple.sh`: UniFFI bindings from the compiled library;
  macOS slices selected with `PPVPN_APPLE_TARGETS`, then `strip -x`.
- `tools/desktop/build-service.sh macos-arm64`: service profile and native helpers.
- `apps/flutter/scripts/package-macos.sh`: complete app assembly including push agent,
  Sparkle and service helpers; `apps/macos/scripts/package-app.sh` signs and packages it.
- `crates/tunnel-host/scripts/build-xcframework.sh OUT ios`: device tunnel static library
  under `mobile`, checked against its C header. Simulator is a separate slice.
- iOS build: existing `ci-build.sh` device Release/no-codesign path for isolated build
  inspection; archive/export separately when the agreed signing conditions are available.
  Do not invoke release.yml's App Store upload path for this experiment.

Do not transplant the old PoC's `external-core` features or stdio role into the final
packager without the runtime owners' contract. The new runtime crate/feature names,
executable location, launch/IPC requirements, entitlement changes and packaging output
paths must come from implementation commits. If a feature dependency is trimmed, save the
before/after feature graph and prove that all baseline protocols and push capabilities remain.

## Functional and ABI gates

| Gate | Required evidence |
| --- | --- |
| One active forwarding core | Runtime instance identity and lifecycle traces across standard mode, TUN attach/detach and recovery. A single Mach-O or one executable path does not establish this. Never start a fallback second core silently. |
| macOS proxy continuity | Maintain already established authenticated HTTP CONNECT and SOCKS connections while repeatedly enabling/disabling VPN. Verify mixed listener identity, port and credentials remain stable. Use an isolated test service/host, not the user's installed VPN. |
| Mixed proxy equivalence | HTTP forwarding/CONNECT, SOCKS authentication rejection and success, UDP associations, DNS and routing policy, ingress/protocol coverage and connection accounting. Include real routed data, not only successful handshakes. |
| macOS lifecycle | TUN-on/off/recovery races, connect/disconnect cancellation, sleep/wake, helper authentication, daemon restart, rollback and logout. Report which events necessarily end sessions. |
| iOS lifecycle | Identify which process owns the core in each system state; prove no overlap during handoff. Exercise extension restart, host suspension/termination and VPN start/stop. Any system-imposed interruption or unavailable proxy endpoint must be explicit; macOS continuity is not assumed on iOS. |
| Push and updates | macOS login-item/APNs/notification path, iOS token registration and notification navigation, Sparkle and installer/helper verification stay covered. No deletion of push or update code merely to reduce bytes. |
| FRB | Preserve the 14 `frb_` exports plus `store_dart_post_cobject` used by the baseline; verify generated Dart/Rust content-hash agreement and run actual bridge calls. |
| UniFFI | Baseline packaged library has 176 `uniffi_`/`ffi_` exports. Verify generated Swift/C declarations and UniFFI checksums against the built library; unchanged names alone do not validate ABI types. Regenerate consumers together when a reviewed API changes. |
| iOS export stripping | Preserve Runner `Bridge.exports` and PacketTunnel `Tunnel.exports`; use `check-ios-exports.py` against final binaries, not only an intermediate static library. Keep archive stripping non-global and check dynamic export lookup. |

## Read-only measurement

`tools/apple/measure_size.py` uses only Python's standard library; the optional `abi`
command calls Apple's `nm`. Inventory never signs, strips, installs, mounts or launches code.
It reports actual Mach-O slices, per-file SHA-256, bundle identity/version fields and grouped
program/library/resource totals. An archive inventory includes dSYMs in its artifact total
and reports the contained application separately. IPA members are streamed without extraction.
Installer inventories are opaque byte/hash measurements, not installation acceptance.

```sh
python3 -m unittest discover -s tools/apple -p '*_test.py' -v

python3 tools/apple/measure_size.py inventory \
  --artifact artifacts/baseline/PPVPN.app --platform macos --kind app \
  --source-commit BASELINE_SHA --functional-baseline FUNCTIONAL_BASELINE_SHA \
  --conditions baseline-conditions.json --output baseline-app.json

python3 tools/apple/measure_size.py inventory \
  --artifact artifacts/candidate/PPVPN.app --platform macos --kind app \
  --source-commit CANDIDATE_SHA --functional-baseline FUNCTIONAL_BASELINE_SHA \
  --conditions candidate-conditions.json --output candidate-app.json

python3 tools/apple/measure_size.py compare baseline-app.json candidate-app.json \
  --output app-comparison.json

python3 tools/apple/measure_size.py abi \
  --baseline-library artifacts/baseline/libppvpn_client.dylib \
  --candidate-library artifacts/candidate/libppvpn_client.dylib \
  --baseline-binding artifacts/baseline/ppvpn_client.swift \
  --candidate-binding artifacts/candidate/ppvpn_client.swift \
  --output uniffi-exports.json
```

Use `--required-symbols` with an explicit export list when checking FRB/tunnel contracts.
ABI comparison fails when required exports disappear or supplied generated bindings differ;
a reviewed coordinated ABI change needs its own consumer validation. This does not replace
UniFFI checksum validation or FRB runtime tests.

Run separate inventories for each DMG, PKG, xcarchive and IPA. Never subtract unlike metrics.
Comparison rejects different platforms, artifact kinds, functional baselines, build conditions
or actual Mach-O architectures. Savings use baseline minus candidate, with both decimal MB
(1,000,000 bytes) and MiB (1,048,576 bytes); negative savings mean growth.

## Historical artifact recheck

The existing macOS arm64 baseline was re-read without rebuilding or changing it:

| Metric | Bytes |
| --- | ---: |
| App regular files (190) | 148451345 |
| DMG | 58300514 |
| PKG | 57243603 |
| Flutter bridge dylib | 57642816 |
| Agent UniFFI library | 38719344 |
| Service executable | 20287328 |
| Installer helper | 388832 |
| Uninstaller helper | 354176 |

All five component hashes and both installer hashes match the preserved baseline manifest;
all detected Mach-O files are arm64. This verifies retained artifact identity, not a fresh
reproducible build. The earlier 73,351,281-byte PoC app remains explicitly excluded from final
single-active-core results. There is no final candidate size yet. There is also no attested
clean iOS archive/export baseline or App Thinning result yet.
