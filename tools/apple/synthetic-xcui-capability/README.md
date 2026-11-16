# Minimal managed-Mac XCTest capability probe — review before one execution

Scope: one synthetic AppKit window, real standard XCTest UI launch/query/two clicks/readback and a screenshot of that synthetic application only. No PPVPN Client/SDK/profile/account/helper, no359 installation/launch, no network listener, no system-proxy/TUN/DNS change and no keychain/signing-key access. It does not exercise or bypass the denied local Finder access. Do not compile or execute the UI test on the user's Mac.

## Exact probe

`App/main.swift` creates a single native window with a nonce label, counter and button. It refuses a launch without a UUID-shaped test nonce. Its only custom action increments an in-memory counter.

`UITests/WindowRoundTripTests.swift` uses `XCUIApplication(bundleIdentifier:)`, launches only `com.peakpassvpn.qa.auto359.capability`, verifies the matching nonce, clicks the synthetic button twice and waits for actual count1/count2. It retains a synthetic-App screenshot and receipt in xcresult, and terminates that App. No interruption monitor, system-dialog click, TCC grant or other-app operation exists.

`WindowServerPreflight.swift` reads active console/display/screen presence. `AXIsProcessTrusted()` is called **without a requesting prompt** and its result is diagnostic for this wrapper only; it is not misrepresented as XCTest-agent trust. Missing WindowServer/active console fails77 before the UI test. Actual standard-XCTest input/readback must pass independently.

The supplied fixed `Auto359SyntheticCapability.xcodeproj` and shared scheme have one synthetic App target and one standard macOS UI-test target. The generated synthetic test products use ordinary ad-hoc development signing, not a private identity. No signed359 bytes, entitlements, hardened-runtime policy or signature checks are changed. No SPM/network dependency is declared. Only the existing Xcode tools and Python standard library are needed. No XcodeGen, brew, pip, SPM or other project-generator dependency exists. Runtime does not install tools.

`run_probe.py` defaults to input-hash review. `--execute` additionally requires the exact approved manifest hash, actual non-root arm64 Darwin, real hosted-job environment, a new output directory inside RUNNER_TEMP and absence of the fixed PPVPN installed images/jobs. It uses the supplied project directly to build the synthetic products, then separately invokes test-without-building with only one test and parallel execution disabled. It requires one passed test, zero failed/skipped tests and a recognized xcresult summary, plus known cleanup. Unknown summary/schema or timeout cannot pass.

The nonprivileged `watchdog.py` is independent of xcodebuild. It can signal only current-UID processes whose exact executable path is inside this run's derived directory and whose basename is one of the two synthetic products. It never kills an OS testmanager service or uses broad pkill/killall. The test command has a150s timeout and the watchdog a210s deadline after build. Unresolved cleanup remains failure with the watchdog armed. No installed service/state/proxy needs restoration at this stage because none is changed.

## Inputs and one-shot managed procedure for Root

Root/QA first approve `INPUT-MANIFEST.json` and its SHA from `PREPARATION-RESULT.json`. Stage **all listed files**, Xcode26/XCTest and system Python3 on a new dedicated arm64 hosted Mac VM before the probe. Use an exclusive job/concurrency key such as `auto359-synthetic-xcui-capability`; do not share the GUI with any other task. This packet contains no workflow dispatch, resource provisioning, permission request, signer or artifact publication code.

Review-only command, safe without a Mac GUI and used during preparation:

```
python3 run_probe.py
```

Only after Root assigns the concrete managed resource and gives the one execution instruction:

```
python3 run_probe.py --execute \
  --approved-manifest-sha256 <exact reviewed INPUT-MANIFEST.json SHA256> \
  --out "$RUNNER_TEMP/auto359-synthetic-xcui-capability"
```

Retain RESULT.json, WindowServer-preflight.log, test/build logs, xcresult, summary JSON and owned cleanup/watchdog receipts. The stage has no fixture network traffic. GitHub's ordinary job/result artifact communications remain unchanged; no proxy/interface settings are touched. Do not run sudo, edit TCC databases, enable remote sharing/VNC, disable SIP or click system authorization dialogs to make the probe pass.

## Pass and failure interpretation

- PASS means exactly the synthetic standard XCUI launch/query/two-click/readback plus cleanup worked on that VM. It is not a359 App, Flutter accessibility, helper, owner, held-flow or restore PASS.
- WindowServer/active-console failure is reported specifically. A target launch, accessibility/input failure or timeout is retained as a failing XCTest result; inspect the native log to distinguish actual AX/TCC denial, invisible window or launch failure. Do not relabel an unexplained timeout as a known TCC error or silently allow a system prompt.
- Build/schema/toolchain failure is infrastructure failure, not an actual GUI-capability result. Repair only the narrow probe after reviewing that evidence, then bind a new input manifest; no speculative full-App deployment.

## Next stage only after actual capability PASS and QA

Reuse the exact already-notarized359 archive (SHAaa54be70…b5229c), matching helpers, immutable source047/treeaf5 and validated fixture profile5754a674…c6533a/marker c6c7d66f…43bdb7. QA must accept clean isolation and the actual restore/watchdog before Root's full-AppGO. The normal-trust item is already accepted; do not repeat notary submission or read a signing key.

Normal signed359 identity requires actual fixed root-owned App/helper install paths; a temporary copied App/helper cannot pass its unchanged production audit-token/path policy. Explicit helper private-dir arguments and private runtime.sock are valid isolation mechanisms but do not remove those identity checks. The real CompatibleOn path writes system proxy settings. Therefore full App installation/service/proxy changes may happen only in the fresh disposable test VM, never on the user Mac or ordinary account/Surge. A full-stage independent watchdog must snapshot/restore the VM's prior proxy state in time to preserve GitHub result return. This capability-stage watchdog does **not** claim that full restoration ability.

For later external signed-App UI driving, Apple also documents `XCUIApplication(url:)`, which can target the exact installed App path without rebuilding/resigning it. That is a future narrow adapter after this generic native capability probe succeeds. It must verify actual signed359 identity, actual Flutter controls/capabilities and the original authenticated Inspect owner; no synthetic nonce/counter is evidence for those facts. Follow the existing `UI-ACCEPTANCE-359-RUNBOOK.md` for separate periodic/Manual and active-owner-loss scenarios; no new control port, second simultaneous Core or fake ACK/health is allowed.

Primary API references: [XCUIApplication](https://developer.apple.com/documentation/xcuiautomation/xcuiapplication), [bundle identifier initializer](https://developer.apple.com/documentation/xcuiautomation/xcuiapplication/init(bundleidentifier:)), [command-line testing](https://developer.apple.com/library/archive/technotes/tn2339/_index.html).
