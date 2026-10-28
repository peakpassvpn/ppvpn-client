# PPVPN for macOS

Native SwiftUI + AppKit app (macOS 13+): menu bar extra, one main window
(shown at launch), and a Settings scene. All client logic lives in the shared
Rust crate `crates/desktop`, consumed through its UniFFI Swift package.
Paths and commands here are relative to the repository root.

## Build

```bash
# 1. The privileged-service helpers (embedded into Contents/MacOS; the service
#    runs the enhanced-mode engine in process). A Debug build works without them.
tools/desktop/build-service.sh macos

# 2. PPVPNClient Swift package + PPVPN.xcodeproj
apps/macos/scripts/bootstrap.sh            # --release for an optimised crate

# 3. Build / run
open apps/macos/PPVPN.xcodeproj
# or
xcodebuild -project apps/macos/PPVPN.xcodeproj -scheme PPVPN -configuration Debug build
```

## Tests

The app logic (`AppLogic`, no AppKit or SwiftUI) has XCTest tests that run on
macOS and Linux:

```bash
# macOS, after bootstrap.sh
swift test --package-path apps/macos/AppLogic

# Linux (CI pins Swift 6.4.0): builds the crate and a Linux PPVPNClient package
apps/macos/scripts/build-swift-linux.sh
swift test --package-path apps/macos/AppLogic
```

## Release DMG and package

One DMG and one first-install package per architecture (no universal build):

```bash
apps/macos/scripts/package-dmg.sh arm64    # → dist/macos/PPVPN-<version>-macos-arm64.{dmg,pkg}
apps/macos/scripts/package-dmg.sh x86_64   # → dist/macos/PPVPN-<version>-macos-x64.{dmg,pkg}
```

Runs `bootstrap.sh --release` (skip with `SKIP_BOOTSTRAP=1`), builds Release for
the one architecture, thins every Mach-O in the app to it (our code, the shared
`PPVPNClientFFI.framework`, Sparkle and the service helpers, which
are staged universal) and re-signs inside out, then checks each binary's
architecture, that the app and the push agent link the single framework in
`Contents/Frameworks`, and the signature. It ad-hoc signs the DMG and writes
`release-meta-macos-<arm64|x64>.json` (schema 1: platform, channel, version,
build, file, length, sha256, ed_signature, min_os, published_at, all of the
DMG, plus install_file, install_length and install_sha256 of the package),
from which the update site's feed is written. The package (`pkgbuild`,
unsigned) installs only to /Applications, is not relocatable, and runs
`packaging/postinstall`, which hands the bundle to the console user so that
Sparkle updates it without an administrator prompt (docs/security.md). `SPARKLE_PRIVATE_KEY_FILE` fills `ed_signature`;
`PPVPN_BUILD_NUMBER` (CI run number) makes `CFBundleVersion` = `<version>.<n>`.
With `PPVPN_UPDATE_SITE` and a channel set, each build's Sparkle feed is
`<site>/desktop/<channel>/appcast-macos-<arm64|x64>.xml`, so each
package only updates to the same architecture; a build without a channel has
no feed. CI (`tools/desktop/ci/build-macos-native.sh`)
builds both on an Apple silicon runner into one artifact.

## Signing and installing

Releases are signed with our Apple Developer ID (Team ID in
`crates/service/src/macos_team_id`, #471) and notarized. CI
(`tools/desktop/ci/build-macos-native.sh`) imports the Developer ID
Application and Installer identities into a temporary keychain from the
`MACOS_SIGNING_P12_*` and `MACOS_INSTALLER_P12_*` secrets, refuses
identities of any other kind or team, and passes the App Store Connect API
key (`ASC_KEY_ID`, `ASC_ISSUER_ID`, `ASC_KEY_P8`) to notarytool.
`package-app.sh` then:

- signs every piece of code inside out with the hardened runtime and a
  secure timestamp, keeping codesign's own designated requirements
  (identifier, Apple's anchor, the Team ID), which carry across builds and
  certificate renewals (keychain access, the push agent's SMAppService
  registration, the notification permission);
- takes `com.apple.security.cs.disable-library-validation` out of the
  entitlements: every embedded library carries the same Team ID;
- notarizes the app and staples its ticket, builds the DMG (signed) and the
  pkg (`productsign`, Developer ID Installer) from it, notarizes and staples
  both, and checks Gatekeeper's verdict on all three;
- only then signs the DMG for Sparkle and writes the release metadata.

A stable build without the identities or the API key fails. Without them
(local builds, pull requests) signing falls back to ad-hoc; CI's packaging
check signs with a throwaway self-signed identity, where each designated
requirement pins that certificate and the entitlements keep
disable-library-validation (no Team ID).

The privileged service accepts only a client with our Developer ID signature
(`crates/service/src/macsign.rs`). A service installed by a release before
the Developer ID (0.7.x and older pin the old self-signed certificate)
refuses the new app, which then reinstalls it once, behind the usual
administrator prompt.

The sign-in is kept in `~/Library/Application Support/PPVPN/credentials`:
0600 in a 0700 directory, written atomically, excluded from Time Machine and
deleted on sign-out (Debug builds use `credentials.debug`). Builds without a
Team ID could not keep it in the login keychain (it partitions their items by
cdhash, which every build changes). TODO: move back to the keychain
(`KeychainCredentialStore`) now that releases carry a Developer ID.

Installing: open the downloaded .pkg; Installer puts the app in
/Applications. The installed app has no quarantine attribute, opens without
asking and is not translocated. From the DMG instead, drag the app to
Applications in Finder. Updates installed by Sparkle keep the same
signature.

## Updates

Sparkle 2. The appcast URL and public key come from the `PPVPN_UPDATE_FEED_URL`
and `PPVPN_SPARKLE_PUBLIC_KEY` build settings (environment when running
`package-dmg.sh`). Builds without both have updates disabled and hide
"Check for Updates…". Appcasts are hosted on R2 alongside the other platforms.

`PPVPN.xcodeproj` and `PPVPNClient/` are generated and git-ignored; re-run
`bootstrap.sh` after `project.yml` or the ppvpn-client interface changes.

| Variable | Used by | Purpose |
|---|---|---|
| `PPVPN_CLIENT_CRATE` | `bootstrap.sh` | ppvpn-client checkout (default `crates/desktop`) |
| `PPVPN_BINARIES_DIR` | build phase | staged `*-universal-apple-darwin` binaries (default `build/binaries`); `package-dmg.sh` thins them |
| `PPVPN_API_BASE_DEFAULT` | build setting | backend base; www unless set (e.g. `xcodebuild … PPVPN_API_BASE_DEFAULT=<dev backend>`) |
| `PPVPN_UPDATE_FEED_URL`, `PPVPN_SPARKLE_PUBLIC_KEY` | build setting | Sparkle appcast + EdDSA key; empty disables updates |
| `PPVPN_PREVIEW=1` / `signed-in` | Debug run | fake backend (`PreviewBackend`) instead of the Rust client |

Credentials are stored in a 0600 file, not the keychain (see "Signing and
installing"): `credentials` for Release, `credentials.debug` for Debug.

## Logs

- App (ppvpn-client, standard-mode engine, push agent): `~/Library/Logs/PPVPN/`
- Privileged service and Enhanced Mode engine: `/Library/Logs/PPVPN/` (`ppvpn-service.log`,
  `ppvpn-core.log`, launchd's `ppvpn-service.out.log` / `.err.log`; readable without root;
  5 MB per file, 3 files kept). Uninstalling or reinstalling the service leaves them in place;
  builds up to 18 wrote them inside the helper bundle, where the uninstall deleted them.

## Layout

- `AppLogic` — SwiftPM package `PPVPNAppLogic` with no AppKit/SwiftUI: `AppState` (all UI state,
  derived from client snapshots), presentation, strings lookup, `ClientBackend` protocol,
  `RustBackend` (UniFFI), `PreviewBackend`
- `PPVPN/App` — scenes, app delegate, `AppModel` (`AppState` published to SwiftUI, AppKit hooks)
- `PPVPN/Platform` — `PlatformHooks` implementation: credentials, browser, privileged-service install
- `PPVPN/Views` — main window pages (login, overview, nodes, logs), menu bar, settings
