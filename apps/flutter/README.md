# Flutter desktop prototype

Step 2 of #330 (issue #387): one Flutter window with the overview and the
nodes pages, plus the menu bar / tray icon, on Windows, macOS and Linux. It
runs on the same client as the native apps (`crates/desktop`), through
flutter_rust_bridge. It does not replace the native apps and is not part of
any release.

## Layout

- `rust/`: the bridge crate `ppvpn-flutter` (a workspace member). `api.rs` is
  everything Dart can call; `bridge.rs` holds the one `Client`, its listener
  and the conversions; `hooks.rs` holds the platform hooks.
- `lib/theme.dart`: every design token in one place: the unified design
  signed off on #387 (2026-10-20). It covers colours for light and dark,
  shadows, type, sizes and radii, with pixel values from the design sources.
  Latin text uses Inter (bundled by forui), numbers and addresses JetBrains
  Mono (`assets/fonts`, OFL), and Chinese the system font.
- `lib/widgets.dart`: the shared pieces drawn to the design: buttons, the
  three-state switch, the latency pill, flags, the brand mark. Each is
  keyboard-operable and announced to screen readers.
- `lib/strings.dart`: the handoff's `strings.json` (zh and en) for the keys
  the prototype shows.
- `lib/shell.dart`: the window frame, following the navigation decided on
  #387. It has a left sidebar (200 wide, 34-high items: overview, nodes,
  logs) and the account card and settings at the bottom. Messages open from
  a bell with the unread badge at the top right of the content area. There
  is no system title bar. On macOS the traffic lights sit at the top of the
  sidebar; on Windows and Linux the caption buttons are at the top right of
  the content area, right of the bell.
- `lib/pages/`: sign-in, overview, nodes, logs, settings, and the message
  center (opened from the bell; `lib/messages_state.dart` holds its pages).
  `lib/tray.dart`: the tray icon and its menu. `lib/settings.dart`: the
  app's own settings.
- `assets/tray/`: the shipped tray icons, copied from the native apps: the
  macOS template images (`menubar-*`, from `apps/macos` `MenuBar-*`) and
  the Windows and Linux colour icons (`tray-*`, from `apps/assets/icons`)
  for not connected, in progress, connected and error.

## What it does and does not do

- Signs in with the device code, picks a node, and connects with the
  **system proxy** (compatible mode, on the in-process standard core) or in
  **enhanced mode** (TUN through the privileged service, #401). Enhanced
  mode installs the service on first use, after an explanation, behind the
  system's admin prompt, exactly as the native apps do (same paths and
  helpers: `rust/src/hooks.rs`). The helpers (`ppvpn-service-install`,
  `-uninstall`, and on macOS `ppvpn-service`) must sit next to the app's
  executable; `scripts/ci-build.sh` does not put them there. On Linux,
  `scripts/install-linux-layout.sh` does (see below).
- Quitting from the tray puts the system proxy back (`Client::shutdown`).
  Closing the window only hides it.
- The sign-in is kept in a file in the prototype's own data directory,
  readable only by the owner on macOS and Linux. The native apps use the
  keychain or keyring instead. The data, logs and sign-in are separate from
  the native apps':
  - macOS: `~/Library/Application Support/PPVPN Flutter`, logs in
    `~/Library/Logs/PPVPN Flutter`;
  - Linux: `$XDG_DATA_HOME/ppvpn-flutter`, logs in
    `$XDG_STATE_HOME/ppvpn-flutter/logs`;
  - Windows: `%LOCALAPPDATA%\PPVPN Flutter`.
- The settings page (#401 step 3) sets the appearance, the language and
  the routing mode, and shows the account, the version and the system
  service's status. Update checks and installing the service come in later
  steps: their rows are shown, disabled.
- Launch at login (#401 step 4, `rust/src/autostart.rs`) is the native
  apps' own, off by default, its state read from the system:
  `SMAppService.mainApp` on macOS (with the approval hint and System
  Settings › Login Items), the `PPVPN` value under HKCU's `Run` key on
  Windows (with the hint when Settings › Apps › Startup turned it off; the
  value is pointed at the current executable at every start), and
  `$XDG_CONFIG_HOME/autostart/com.peakpassvpn.ppvpn.desktop` on Linux. The
  Windows and Linux entries pass `--background`: the app starts in the
  tray with no window. For that, `scripts/ci-build.sh` takes the
  first-frame show out of those two runners, and `main.dart` shows the
  window otherwise.
- Appearance, language and the API override are kept in `settings.json` in
  the data directory above. The API override (Settings → Advanced) takes
  effect on the next launch; `PPVPN_API_BASE` wins over it. Without either
  the backend is `https://www.peakpassvpn.com`.
- Quit the native app before trying it: both write the same system proxy
  setting.
- Flags are 4:3 images from flag-icons (MIT, `assets/flags/LICENSE`), only
  for the countries we expect; any other country gets its code on a tile.
  There are no emoji flags: platforms draw them differently, and Windows
  draws them as letters.
- Not yet (to-dos on #387 and #401): the node search field and context
  menu, the message rows' context menu, check for updates in the tray
  (in-app update, #401 step 5).

## Building

Builds and tests run only in CI (`ci.yml`, job `flutter`), on the three
platforms, and only when `apps/flutter/` or `crates/desktop/` changes.
`scripts/ci-build.sh` does the following:

1. generates the platform runner (`flutter create`), with the window
   hidden until `main.dart` shows it on Windows and Linux;
2. generates the bridge glue (`flutter_rust_bridge_codegen generate`);
3. analyses the Dart code;
4. builds the Rust library and the app in release mode;
5. copies the library into the bundle.

`scripts/smoke.sh` then starts the app and checks that it is still running
after 20 s. It takes a screenshot and notes the resident memory and the
bundle's sizes. The job summary shows those numbers, and the artifact
`flutter-<os>` holds the screenshot. Nothing generated is committed (see
`.gitignore`).

`e2e/run.sh` runs the signed-in flow end to end on each platform (#401
step 1). It covers device sign-in, choosing a node, connecting in standard
mode, a request through the node, disconnecting and signing out. The
setup is:

- the real client and core, against `e2e/mock_backend.py` (Python standard
  library, on loopback) and `test/fakenode` as the node;
- `integration_test/signed_in_test.dart` drives the UI;
- `PPVPN_FLUTTER_HOME` keeps the app's data apart, and
  `PPVPN_TEST_TRUST_LOCAL_BACKEND=1` lets the client accept the loopback
  backend.

Pinned versions:

| Component | Version | Pinned in |
| --- | --- | --- |
| Flutter | 3.47.7 | `scripts/install-flutter.sh` |
| flutter_rust_bridge | 2.13.0 | crate, Dart package and codegen |
| forui | 0.27.6 | `pubspec.yaml` |
| tray_manager | 0.8.0 | `pubspec.yaml` |
| nativeapi | 0.5.2 | `pubspec.yaml` |
| window_manager | 0.5.2 | `pubspec.yaml` |

## Linux layout and the enhanced-mode end-to-end test

`scripts/install-linux-layout.sh <bundle> <service-bin-dir>` (root, Linux
only) installs the bundle as the native packages do
(`apps/linux/packaging/nfpm.yaml`). The executable becomes
`/usr/lib/ppvpn/ppvpn`, root-owned and 0755, because that is the only
client the privileged service accepts. `lib/` and `data/` sit beside it,
and so do `ppvpn-service`, `ppvpn-service-install` and
`ppvpn-service-uninstall`, which is where `rust/src/hooks.rs` looks for
them. There is no deb or rpm yet. To remove it, run
`/usr/lib/ppvpn/ppvpn-service-uninstall` and then `rm -rf /usr/lib/ppvpn`.

`e2e/linux_enhanced.sh` is the enhanced-mode test (#401 step 2), the
counterpart of `crates/desktop/src/e2e_linux.rs`. CI runs it in the Linux
`flutter` job after the build as
`sudo test/netns/run.sh --script e2e/linux_enhanced.sh <fakenode>`, which
puts it inside run.sh's network namespace and checks that the host is left
as it was. Before that, the job installs the layout and the service with
`ppvpn-service-install`, then stops the unit, because a systemd unit runs in
the host's namespace. The script then:

1. starts `test/fakenode` in the uplink namespace,
   `/usr/lib/ppvpn-service/ppvpn-service` in the test namespace and
   `e2e/linux_enhanced_backend.py` on loopback;
2. writes a saved sign-in (`credential.json`) to a temporary
   `XDG_DATA_HOME`;
3. starts the installed release app under its own Xvfb and D-Bus session,
   with `PPVPN_FLUTTER_E2E=enhanced`;
4. checks the TUN, the policy rules (9091–9101) and table 2091 while
   connected;
5. checks that `GET http://192.0.2.10:8080/` answers through the node;
6. checks that the routing state is gone after a disconnect and after the
   app quits.

`PPVPN_FLUTTER_E2E=enhanced` (`lib/e2e.dart`) is a test-only switch and is
off unless the variable is set. Once the app is signed in with a profile,
it turns enhanced mode on. It prints `E2E ready`, `E2E phase <phase>` and
`E2E action-error <error>` to stdout. It reads `connect`, `disconnect` and
`quit` lines from stdin. On `quit` or SIGTERM it disconnects and then quits
as the tray's Quit does.
