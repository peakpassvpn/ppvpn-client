# Single-active-core platform build and measurement contract

Status: preparation only. The implementation commits and final candidate measurements are pending.
The functional baseline is `1ddf0a168610172773fe3f479133ecbe5acf932f`
(client 0.7.4, Sail v0.22.0 at `7513c15139e3c8cddb59952d93a8f8b59ec934f1`).
Packaging core code once is insufficient: the runtime acceptance gate is exactly one active
forwarding core. macOS retains authenticated mixed HTTP/SOCKS and UDP.
The user approved removing local proxy servers, proxy authentication/port UI and the
unrelated App Standard forwarder on both iOS and Android. Android removal is a product
decision, not a claim that Android has the same process restrictions as iOS.
The earlier UI stdio child plus privileged TUN PoC has two runtime roles and
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
| iOS device arm64 | Rust bridge release; tunnel `mobile` profile (thin LTO, one codegen unit); Release Runner + PacketTunnel; iOS 15 minimum | App has no Standard forwarding engine or proxy server; PacketTunnel is sole forwarding owner. Require mobile implementation/control API commits | App and extension executable bytes, each embedded shared library, resources; record xcarchive separately from its Products application |
| iOS simulator arm64 | Existing debug simulator smoke recipe | ABI/host integration smoke only | Diagnostic row; never compare its debug bytes with device Release |
| iOS exported IPA | Export baseline/candidate archives under identical export, signing and thinning options | Authorized usable distribution profiles; preserve exact ExportOptions and export logs | Actual compressed IPA bytes plus uncompressed Payload app; no upload |
| iOS device download | Matching device model/OS, architecture, thinning settings and delivery report | Actual App Thinning report / device download evidence | Report Apple's device-specific download/install values separately; an IPA or xcarchive is not this metric |
| Android universal APK | Same release Rust bridge, mobile-profile VpnService library, R8/strip/signing/SDK/NDK/JDK/build number, arm64-v8a + x86_64 | App has no Standard forwarder or proxy server; VpnService alone owns forwarding | Compressed signed APK, DEX/resources and per-ABI ELF libraries |
| Android ABI APK | `apk-split`, same release recipe, one row per arm64-v8a / x86_64 APK | Same per-ABI content and signing as its baseline | Never compare an ABI APK with universal APK bytes |
| Android AAB | `aab`, same selected ABIs and upload-signing conditions | Candidate Android implementation commit | Container bytes and per-module native/DEX/resources; AAB is not a Play device download |
| Android device splits | Matching bundletool/device spec and signing; generated from matching AABs | Available bundletool, isolated device/emulator and mobile owner acceptance | Record actual selected split APKs separately; do not substitute Apple or universal APK results |


Before each new build, preserve source commit, source changes, Cargo.lock hash, Sail revision,
Flutter/Dart/Rust/Xcode/SDK versions, enabled crate features, Rust flags, deployment target,
architectures, build number and signing/strip settings. Record generated-source changes
separately from authored changes. A reference checkout containing generated files or device
diagnostics is not a clean reproducible source snapshot.

The conditions JSON records the historical macOS recipe. App metadata identifies Xcode
26.6 (17F113) and macOS SDK 26.5 (25F70); the retained Flutter 3.47.7 SDK's release engine
has the same arm64 Mach-O UUID as the signed baseline framework. This cross-check does not
independently attest every Dart compiler invocation. Preserve source/build logs alongside
these artifact facts, and record the actual candidate toolchain before comparing. The
comparator rejects unknown/null conditions. Source commit arguments are supplied by the
caller; inventory cannot prove how a binary was compiled.

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
| Desktop mixed proxy equivalence | HTTP forwarding/CONNECT, SOCKS authentication rejection and success, UDP associations, DNS and routing policy, ingress/protocol coverage and connection accounting. Include real routed data, not only successful handshakes. |
| macOS lifecycle | TUN-on/off/recovery races, connect/disconnect cancellation, sleep/wake, helper authentication, daemon restart, rollback and logout. Report which events necessarily end sessions. |
| iOS lifecycle | Identify which process owns the core in each system state; prove no overlap during handoff. Exercise extension restart, host suspension/termination and VPN start/stop. Any system-imposed interruption or unavailable proxy endpoint must be explicit; macOS continuity is not assumed on iOS. |
| Approved mobile removals | iOS and Android local HTTP/SOCKS listeners, proxy authentication/port UI and unrelated App Standard forwarding instance are intentionally removed. These are disclosed feature changes, not protocol regressions on desktop. |
| Retained mobile functionality | Login, VPN, rules, statistics, logs, line selection and pin remain. With VPN off, PING/TCP checks must use lightweight capabilities without starting a second forwarding core. Android retains socket protect, physical-network updates, revoke/onRevoke and foreground notification lifecycle. |
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


## Apple push build/export integration

The common and platform receivers originate in mobile-owned `e45b79e`. The build script
adds both sources to an existing generated target entry, without editing the owned Runner
source templates. This includes them in Xcode projects using explicit source lists and
avoids duplicate copied definitions. It does not start a receiver or enable APNs.

`PPVPN_APPLE_PUSH_ENABLED=0` is the default. Enabling requires the explicit real profile
path in `PPVPN_APPLE_PUSH_PROFILE`. `tools/apple/push_config.py prepare` derives the
platform entitlement and app environment stamp from that profile, checking the exact topic,
team, App ID prefix and expiry. App ID prefixes can differ from Team IDs; the validator
checks both separately, as documented in [Apple TN2415](https://developer.apple.com/library/archive/technotes/tn2415/).
It preserves unrelated VPN/App Group entitlements. No DEBUG, build flavor
or receipt heuristic selects TestFlight's APNs environment.

The macOS packager carries APNs through its final inside-out signing pass and embeds the
matching profile. iOS verification reads the exported IPA's actual signed app and embedded
profile after export. App Store/distribution exports require production APNs. A re-sign that
changes environment or team without a matching app stamp is rejected; signed products are
never patched to conceal a mismatch. The final validator may be re-run on a separately
re-signed app/IPA. It checks configuration, not real APNs delivery or backend binding.

The IPA check refuses unsafe ZIP paths/duplicates/symlinks and requires extraction headroom.
An unsigned disabled-push diagnostic build may be inspected only with an explicit option;
that path cannot pass enabled APNs or distribution validation. Existing Agent retirement,
backend registration/account generation, launch integration and real notification acceptance
remain owned integration gates. Android's current push transport is unchanged.

## Android build and measurement

`tools/android/build-size-artifact.sh CHECKOUT OUTPUT apk|aab|apk-split|all` executes the existing
Linux Android recipe against one clean isolated checkout, with no upload or installation.
It validates SDK/build-tools, API-26 NDK compilers and tools, checks disk headroom, and records
source/lockfile/build number/toolchain/backend-hash provenance without emitting credentials.
Run the baseline and integrated candidate serially with identical conditions. The optional
`--preflight` checks readiness without building. Do not pass the read-only reference checkout.

`apk-split` first prepares JNI libraries with the standard recipe, then uses Flutter's
`--split-per-abi` release build; both arm64-v8a and x86_64 APKs remain separate outputs.
`aab` keeps the existing Play App Bundle recipe. `all` reuses one JNI preparation and
serially emits universal APK, both ABI APKs and AAB; it does not run parallel native builds.
Output directories must be new and empty. APK signatures are verified with apksigner and
AAB signatures with jarsigner/keytool; certificate SHA-256 digests are recorded for matching
baseline/candidate signing conditions. The driver does not replace Android push,
modify VpnService, drop socket protect, provision SDKs or clean shared caches.

Use `tools/android/measure_size.py inventory` with explicit `--kind apk|aab`,
`--variant universal|abi-split|bundle`, `--abis` and matching `--conditions`. It validates ELF
architectures against native ABI directories and reports container/DEX/native/resource bytes
and hashes. Its `compare` rejects unlike variants, ABI sets, optimization/signing/toolchain
conditions or baselines. AAB/universal bytes are never labeled as device download size.

Both measurement tools accept `--approved-removal mobile-local-proxy-server`,
`--approved-removal mobile-proxy-settings-ui` and `--approved-removal app-standard-forwarder`
for the mobile candidate. Keep these out of the optimization/signing conditions shared by
both builds. Reports disclose those changes; total savings cannot be attributed exclusively
to runtime deduplication. macOS inventory rejects mobile-removal flags.

The initial local toolchain inventory found no Android SDK, NDK, usable JDK, Gradle, adb or
bundletool and no retained APK/AAB baseline. Android outputs therefore remain unbuilt until
an SDK-equipped Linux build slot and integrated mobile commit are available. No Apple or
historical two-runtime PoC sizes are substituted for Android results.

## Contained package regression

`python3 tools/apple/package_fixture.py` is an opt-in real macOS packaging regression.
It compiles tiny synthetic Mach-O files, signs the temporary app ad hoc and produces a
real DMG/PKG with the production packaging script. It verifies artifact hashes/lengths
against release metadata and never installs or launches the fixture. Its output is
explicitly fixture-only and must never enter client size comparisons. DiskImages access
is required; a sandbox may reject `hdiutil` with a device-configuration error.

DMG retries no longer detach arbitrary volumes named PPVPN. PKG payload validation consumes
the complete pkgutil listing, preventing an early grep exit from turning a valid payload
into a pipefail/SIGPIPE error. No shared cache or other checkout is cleaned.
