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
- `lib/theme.dart`: every design token in one place, from the desktop design
  handoff. It keeps the brand and semantic colours, and uses the macOS sizes,
  spacing, radii and window size on every platform. Controls are forui's
  neutral ones. A cross-platform design (desktop and mobile) will replace
  these values.
- `lib/strings.dart`: the handoff's `strings.json` (zh and en) for the keys
  the prototype shows.
- `lib/shell.dart`: the window frame, following the navigation decided on
  #387. It has a left sidebar (200 wide, 34-high items: overview, nodes,
  logs) and the account card and settings at the bottom. Messages open from
  a bell with the unread badge at the top right of the content area. There
  is no system title bar. On macOS the traffic lights sit at the top of the
  sidebar; on Windows and Linux the caption buttons are at the top right of
  the content area, right of the bell.
- `lib/pages/`: sign-in, overview, nodes. `lib/tray.dart`: the tray icon and
  its menu.
- `assets/tray/`: placeholder tray icons for the four states (off,
  connecting, on, error), told apart by shape. The brand mark replaces them.

## What it does and does not do

- Signs in with the device code, picks a node, and connects in **compatible
  mode** (the system proxy on the in-process standard core). Enhanced mode
  is shown but disabled: the prototype installs no privileged service.
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
- `PPVPN_API_BASE` overrides the backend (`https://www.peakpassvpn.com`).
- Quit the native app before trying it: both write the same system proxy
  setting.
- Not in the prototype: the logs, messages and settings pages (their sidebar
  items and the bell lead to a placeholder), the node context menu,
  bundled flag images (node names carry the backend's emoji flag, which
  Windows draws as letters), animated tray icons.

## Building

Builds and tests run only in CI (`ci.yml`, job `flutter`), on the three
platforms, and only when `apps/flutter/` or `crates/desktop/` changes.
`scripts/ci-build.sh` does the following:

1. generates the platform runner (`flutter create`);
2. generates the bridge glue (`flutter_rust_bridge_codegen generate`);
3. analyses the Dart code;
4. builds the Rust library and the app in release mode;
5. copies the library into the bundle.

`scripts/smoke.sh` then starts the app and checks that it is still running
after 20 s. It takes a screenshot and notes the resident memory and the
bundle's sizes. The job summary shows those numbers, and the artifact
`flutter-<os>` holds the screenshot. Nothing generated is committed (see
`.gitignore`).

Pinned versions:

| Component | Version | Pinned in |
| --- | --- | --- |
| Flutter | 3.47.7 | `scripts/install-flutter.sh` |
| flutter_rust_bridge | 2.13.0 | crate, Dart package and codegen |
| forui | 0.27.6 | `pubspec.yaml` |
| tray_manager | 0.8.0 | `pubspec.yaml` |
| window_manager | 0.5.2 | `pubspec.yaml` |
