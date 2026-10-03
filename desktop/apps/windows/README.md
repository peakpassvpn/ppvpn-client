# PPVPN for Windows (WinUI 3)

Native Windows shell for PPVPN: WinUI 3 / Windows App SDK 2.5, C#, .NET 8,
unpackaged and self-contained, win-x64 only, Windows 10 1809+.

**Status:** real. The client logic is the shared Rust crate `crates/ppvpn-client`
through its UniFFI bindings (`apps/shared/PPVPN.Client`, namespace `PPVPN.Ffi`); the
view models and the fake backend are the shared `apps/shared/PPVPN.App.Core`
(also used by the Linux app). This project holds only the WinUI views, the Windows
platform hooks and the Windows implementations of the App.Core seams.
`--fake-profile` swaps the crate for App.Core's in-process `FakeClientBackend`.

## Build and run

A Windows build machine (Windows 10 or 11, PowerShell) needs the .NET SDK 8.0.4xx,
MSVC, Rust, `uniffi-bindgen-cs` v0.11.0+v0.31.0, NSIS and Git. When working over
SSH, refresh `$env:Path` from the machine value plus `$env:USERPROFILE\.cargo\bin`
in every command.

When copying the sources from macOS, pack them with `COPYFILE_DISABLE=1 tar`
(excluding `bin`, `obj`, `target`, `Generated` and `runtimes`): otherwise tar adds
`._*` files that break dotnet.

```powershell
$env:Path = [Environment]::GetEnvironmentVariable("Path", "Machine") + ";$env:USERPROFILE\.cargo\bin"
cd C:\src\ppvpn   # the desktop/ directory of the checkout, wherever it is
# 1. the crate and its C# bindings (again after every crate change; -Release for packaging)
crates\ppvpn-client\scripts\build-dotnet.ps1
# 2. the app (talks to www unless $env:PPVPN_API_BASE says otherwise; see "Backend")
cd apps\windows\PPVPN.Windows
dotnet build -c Debug -p:Platform=x64
# -> bin\x64\Debug\net8.0-windows10.0.19041.0\win-x64\ppvpn.exe
#    + runtimes\win-x64\native\ppvpn_client.dll (copied from PPVPN.Client)
```

The build must stay warning-free. Standard mode (local proxies, speed tests) needs
`ppvpn-core.exe` next to `ppvpn.exe`; a dev build does not copy it, so copy
`vendor\ppvpn-core\<CURRENT>\build\ppvpn-core-windows-amd64.exe` there as
`ppvpn-core.exe` when you need it.

If the XAML compiler fails with `WMC9999` saying it cannot find
`Microsoft.UI.Xaml.Markup.Compiler.ErrorMessages.resources`, there is a real
XAML error underneath: the compiler crashes while formatting the message on a
non-English Windows. The actual stack is in
`obj\x64\<Configuration>\net8.0-windows10.0.19041.0\win-x64\output.json`
(`MSBuildLogEntries`). A common cause is an `x:Bind` function path whose
namespace prefix is wrong.

A GUI started from SSH never becomes visible. Launch it in the interactive
session of the signed-in user (`<user>` below):

```powershell
$a = New-ScheduledTaskAction -Execute C:\src\ppvpn\apps\windows\PPVPN.Windows\bin\x64\Debug\net8.0-windows10.0.19041.0\win-x64\ppvpn.exe -Argument "--fake-profile --page=nodes --demo-probe"
$p = New-ScheduledTaskPrincipal -UserId <user> -LogonType Interactive -RunLevel Limited
Stop-ScheduledTask ppvpnrun -ErrorAction SilentlyContinue   # a task still "Running" ignores Start
Register-ScheduledTask -TaskName ppvpnrun -Action $a -Principal $p -Force; Start-ScheduledTask ppvpnrun
```

This also matters for the real backend: Credential Manager refuses SSH sessions
(`CredRead` fails with 1312), so the crate can only restore or save a session in
the interactive session.

On a virtual machine, take screenshots from the hypervisor: a session started
over SSH cannot capture the interactive desktop.

### Command-line switches

| Switch | Effect |
| --- | --- |
| `--background` | Start hidden in the tray (used by *launch at sign-in*) |
| `--dev` | Show developer settings (API endpoint override) |
| `--theme=light\|dark\|system` | Override the saved appearance for this run |
| `--page=overview\|nodes\|inbox\|logs\|settings` | Open a page |
| `--fake-profile[=<scenario>]` | Use App.Core's `FakeClientBackend` instead of the crate. The scenario is what the backend answers for the personal team: `active` (default), `no-subscription`, `expired`, `team-disabled`, `invalid` (`FakeOptions.PersonalTeam`). The fake also lists a disabled team (猎户座) in the team picker. |
| `--fake-signed-out` | Fake: ignore the saved (fake) session |
| `--fake-browser=open\|pretend\|fail` | Fake: really open the browser, pretend to, or report failure |
| `--fake-login=approve\|deny\|expire\|never` | Fake: outcome of device login |
| `--fake-approve-after=<s>` | Fake: seconds until the login outcome (default 4) |
| `--fake-connect-fail` | Fake: first enhanced-mode connect fails (retryable) |
| `--fake-refresh-fail` | Fake: background profile refresh fails, which shows the InfoBar |
| `--demo-signin` / `--demo-enhanced` / `--demo-probe` / `--demo-expand` | Press sign in / turn enhanced mode on / test all / expand the first proxy |
| `--demo-teams` | Open the team picker on the overview once the teams are loaded |
| `--fake-service=installed\|approve\|deny` | Fake: pretend the privileged service is installed, or simulate the UAC prompt |
| `--fake-occupied` / `--fake-proxy-fail` / `--fake-method=compatible` | Fake: first enhanced connect is occupied / first compatible connect fails / start in compatible mode |
| `--demo-connect` / `--demo-mode=enhanced\|compatible` | Press the connect switch / set the connection method first (either backend) |
| `--demo-switch-after=<s>` / `--demo-disconnect-after=<s>` / `--demo-copy-proxy` | After connecting: switch method, disconnect, copy the proxy URL |
| `--demo-messages` / `--demo-detail` / `--demo-tray` / `--demo-dialog=install\|uninstall\|signout\|error` / `--demo-quit=<s>` | Open the message center (and its first message), the tray menu, a dialog; quit |

Without `--fake-profile` the app always runs the real crate; the other `--fake-*`
switches only tune the fake. The `--demo-*` switches run the same commands as the UI
(`Services/DemoDriver.cs`), with either backend, so every screen can be reached
without clicking. Unknown switches are ignored and logged.

The fake keeps its session in its own credential, `PPVPN/desktop.credentials.fake`,
so it never overwrites (or reads) a real session.

## Backend

`ClientConfig` (`App.xaml.cs`):

| Field | Value |
| --- | --- |
| `ApiBase` | Settings → Developer → API endpoint (`--dev`) if set (restart to apply), else the build property `PPVPN_API_BASE` |
| `DataDir` | `%LOCALAPPDATA%\PPVPN` (also holds the app's `app-settings.json`; `client-settings.json` there is the crate's) |
| `LogDir` | `%LOCALAPPDATA%\PPVPN\logs`: the crate's `ppvpn-client.<date>.log` and `ppvpn-core.<date>.log`, the app's `ppvpn-windows.<date>.log` |
| `CoreBinDir` | the app directory, where the installer puts `ppvpn-core.exe` |
| `Platform` | `windows` |
| `AppVersion` | `VersionPrefix` from `Directory.Build.props` |

`PPVPN_API_BASE` is an MSBuild property (`-p:PPVPN_API_BASE=...` or the environment
variable of the same name) embedded as assembly metadata `PPVPN.ApiBase`; default
`https://www.peakpassvpn.com` in every configuration (set it to work against dev).
`package.ps1 -ApiBase` and `ci-build.ps1` (the workflow's `api_base`) set it together
with the update feed, so a build never talks to one backend and updates from another.
The app log records the backend and API base at startup.

Strings the view models look up come from App.Core's shared catalogs
(`apps/shared/PPVPN.App.Core/Strings/*.json`, `JsonLocalizer`). `Strings/<lang>/Resources.resw`
holds only the `x:Uid` texts of the XAML views.

## Layout

```
PPVPN.Windows/
  Program.cs            Custom Main: single instance (AppInstance.FindOrRegisterForKey + redirect), then XAML
  App.xaml(.cs)         Composition root (ClientConfig, real or fake backend), styles, quit
  MainWindow.xaml(.cs)  Mica, extended title bar, NavigationView (LeftCompact), error InfoBar,
                        profile-invalid InfoBar, close-to-tray
  TrayIconHost.cs       H.NotifyIcon tray icon with a native popup menu (status, enhanced mode, open, quit)
  Ui.cs                 x:Bind helpers (visibility, severity, theme-aware status colours, profile glyphs)
  Platform/             Real Win32 implementations: PlatformHooks (Credential Manager, ShellExecute, SCM,
                        UAC helpers), HKCU Run, toasts + activation (AppNotifications.cs, ShortcutAumid.cs),
                        WinSparkle updater + installer quit event (AppUpdater.cs)
  Services/             Windows implementations of the App.Core seams (IAppServices, ISettingsStore, IAppLog),
                        BuildInfo (API base), startup options, demo driver
  Views/                LoginView, OverviewPage, NodesPage, InboxPage, LogsPage, SettingsPage, ProfileEmptyState
  Controls/             SettingsCard (Windows 11 settings row), TeamComboBox (disables inactive teams)
  Strings/zh-CN, en-US  Resources.resw (x:Uid texts only)
  Assets/               App icon, tray icons (grey/blue/green/red from assets/icons/tray-*.png)
PPVPN.PushAgent/        ppvpn-push-agent.exe (NativeAOT): the crate's PushAgent + raw WinRT toasts (see "Push agent")
Directory.Build.props   Version (single source of truth), company, product
installer/              ppvpn.nsi + strings.nsh (NSIS installer, zh-CN/en-US)
scripts/                package.ps1 (build the installer), ci-build.ps1 (CI wrapper), sign-file.ps1, appcast-example.xml

../shared/PPVPN.App.Core   view models, IClientBackend, FfiClientBackend, FakeClientBackend, JsonLocalizer
../shared/PPVPN.Client     generated bindings + NativeLoader + runtimes/win-x64/native/ppvpn_client.dll
```

Profile states come from App.Core (`MainViewModel.ProfileState`): NoSubscription,
SubscriptionExpired and TeamDisabled replace the overview's connection controls and
the node list with `Views/ProfileEmptyState` (purchase or renew, switch team, refresh);
Invalid shows a persistent warning InfoBar (`ProfileInvalidText`) while the previous
profile stays in use. Transient errors use the error InfoBar; App.Core decides which
failures get one (a cancelled or superseded device login, `ClientError.Cancelled`, gets
none; the app does not special-case it). Speed-test buttons only bind App.Core's commands:
a row's `ProbeCommand` and `ProbeAllCommand` are disabled until the standard-mode core is
ready (`NodeItemViewModel.CanProbe`, `NodesViewModel.CanProbe`).

## Notifications (backend inbox)

App.Core's `InboxViewModel` holds the message list and the unread count. OS
notifications for backend messages come from the push agent (below), also while the
app is closed; the app itself only shows the first-close notice.

- **Inbox page** (`Views/InboxPage`, nav item 消息 with an unread badge, "99+" capped):
  list with unread dot, severity icon, open link, mark read, mark all read, load more.
  It refreshes when shown (App.Core defers a refresh requested while Restoring until
  signed in) and when the unread count grows while it is shown. It shows a spinner
  during the first load, then the list or App.Core's `IsEmpty` text. The tray tooltip and menu add `TrayUnreadText`
  (the menu line opens the inbox).
- **Toasts** are classic Windows toasts (`Windows.UI.Notifications`) for the
  AppUserModelID `PeakPass.PPVPN`: title, content, and `scenario="urgent"` for Critical
  messages on Windows 11 22H2+ (Windows asks once whether to allow important
  notifications from PPVPN). The push agent shows them; their launch arguments are
  just `id=<pushId>` (`PushMessage.Id`, parsed by `AppNotifications.TryDecode`), never a
  link (see App.Core's click contract).
- **Identity.** `Main` sets `PeakPass.PPVPN` as the process's explicit AppUserModelID
  and writes `HKCU\Software\Classes\AppUserModelId\PeakPass.PPVPN` (DisplayName PPVPN,
  IconUri `Assets\AppLogo.png`, CustomActivator) on every start, so toasts show "PPVPN"
  and our icon, not "ppvpn.exe" (IconUri only when the file exists: Windows caches the
  icon per AUMID, and one cached from a missing file stays blank until WpnUserService
  restarts or the user signs in again). The installer stamps the same id on its shortcuts
  (`ppvpn.exe --set-shortcut-aumid <lnk>`, `Platform/ShortcutAumid.cs`) and removes the
  registration on uninstall.
- **Clicks.** The running instance registers the COM activator
  (`INotificationActivationCallback`, CLSID `FCD3C3FA-…`; `LocalServer32` is this exe with
  `--toast-activated`). A click while running arrives there; a click after quitting makes
  Windows start the app, which registers and receives it (cold start). Either way the
  window comes up on the inbox and `Inbox.ActivateNotification(pushId)` runs on the
  UI thread at once. App.Core resolves the push id through the crate's record of shown
  pushes: it opens the push's http(s) link, or the inbox message it points at, or shows
  the push as a read-only detail (`Inbox.IsDetailReadOnly`); an unknown id only opens the
  message center. If COM
  starts a second process while an instance runs, it takes the click and forwards it with
  `--notification-activated=…` through the single-instance redirection.
- Dev machines: COM caches the activator's `LocalServer32`. When both a dev build and
  the installed app have run, a click after quitting may start whichever registered
  first until you sign out. Installed users have one path, so this does not apply.
- Windows App SDK's `AppNotificationManager.Register()` is not used: in this
  self-contained, unpackaged app it fails with 0x8007007E (it needs the Windows App
  Runtime Singleton package's notification service, which a self-contained app does
  not install).

## Push agent

`ppvpn-push-agent.exe` (`PPVPN.PushAgent/`) shows backend pushes while the app is
closed. It is a separate NativeAOT exe without WinUI or the Windows App SDK, installed
next to `ppvpn.exe`, and loads the same `runtimes\win-x64\native\ppvpn_client.dll`
(PPVPN.Client's `NativeLoader`, from `AppContext.BaseDirectory`).

- **Crate.** `new PushAgent(new PushAgentConfig(%LOCALAPPDATA%\PPVPN, …\logs, "windows",
  version), listener)`; `Run()` on a background thread. The crate long-polls
  `/api/v1/push/pull`, acknowledges what was shown, re-reads `push-agent.json` (written by
  the app's crate after it registered the device, deleted on sign-out), keeps unshown
  messages pending (retried every 5 min, dropped after 24 h) and writes the heartbeat.
  Its log is `logs\ppvpn-push-agent.<date>.log`; the Windows side logs to
  `logs\ppvpn-push-agent-windows.<date>.log` (UTC dates, 7 days kept).
- **Toasts.** `OnPush` shows the toast through raw WinRT COM (`Toasts.cs`:
  `RoGetActivationFactory` + vtable calls for `ToastNotificationManager`,
  `ToastNotification`, `XmlDocument`), so no CsWinRT projection is needed under
  NativeAOT. Same AUMID and toast XML as the app, launch arguments `id=<pushId>`; every push is shown,
  Critical with `scenario="urgent"`, `displayTimestamp` from `created_at`. It returns
  false (the crate keeps the message pending) while `ToastNotifier.Setting` is not
  `Enabled` (turned off in Settings › System › Notifications, or by policy); the setting
  is polled every 60 s, and a change to `Enabled` calls `RetryPending()`. Focus Assist
  does not count: the toast goes to the notification center and is acknowledged.
- **Identity and clicks.** At start the agent writes the same per-user AUMID registration
  as the app (only when `ppvpn.exe` is in its directory); `LocalServer32` stays
  `ppvpn.exe --toast-activated`, so a click starts or reaches the app (the agent is not
  the activator).
- **Process.** One per session (mutex `Local\PPVPN.PushAgent`; a second start exits at
  once). The event `Local\PPVPN.PushAgent.Quit` stops it (`Stop()`, then exit). On
  `Revoked` (the backend rejected the push token) it deletes its Run value and exits; the
  app writes the value again and restarts it after the next registration. `Idle` (no
  `push-agent.json`) keeps it running; the crate re-checks the file every 10 s.
- **Autostart** is the app's job (`Platform/PushAgentAutostart.cs`), separate from launch
  at sign-in: signed in with `push-agent.json` present, the app writes
  `HKCU\Software\Microsoft\Windows\CurrentVersion\Run\PPVPNPushAgent` =
  `"<install dir>\ppvpn-push-agent.exe"` and starts the agent if it is not running;
  signed out, it removes the value and sets the quit event. Quitting the app leaves the
  agent running. `--fake-profile` runs leave the agent alone unless `--fake-push-agent`
  is given (then the fake's sign-in state and the real `push-agent.json` drive it).
- **Footprint** (Windows 11 VM, idle): the exe is 2.1 MB; with the release
  `ppvpn_client.dll` about 20 MB working set (mostly shared image pages), 4.5 MB
  private bytes, 9 threads. NativeAOT, workstation non-concurrent GC, invariant
  globalization.
- **Build.** `dotnet publish apps\windows\PPVPN.PushAgent -c Release -r win-x64` (MSVC
  needed, like the crate). `dotnet build` gives a JIT build for debugging. It publishes
  with `-warnaserror` and no AOT or trim warnings (`NativeLoader` suppresses IL3000 at
  its one `Assembly.Location` call, which NativeAOT never reaches).

## Platform hooks

`Platform/WindowsPlatformHooks.cs` implements the crate's `PPVPN.Ffi.PlatformHooks`:

- Credentials are one generic credential `PPVPN/desktop.credentials` in
  Windows Credential Manager (`CredReadW` / `CredWriteW` / `CredDeleteW`,
  persisted per machine for this user). The blob limit is 2560 bytes. The crate
  deletes a blob it cannot use (e.g. one written by the old fake) at startup.
- `open_url` only opens `http` and `https` URLs, through `ShellExecuteW` on an
  STA thread.
- `privileged_service_installed` asks the Service Control Manager for
  `ppvpn_service`, the `SERVICE_NAME` in `service/src/install.rs`.
- Install and uninstall run `ppvpn-service-install.exe` or
  `ppvpn-service-uninstall.exe` next to `ppvpn.exe` with `Verb=runas`
  (the names `installer/ppvpn.nsi` ships). The Tauri build's
  `-x86_64-pc-windows-msvc` names are also accepted. `ERROR_CANCELLED` (1223) becomes `PlatformException.Cancelled`,
  and a non-zero exit code becomes `Failed`.

Also real: launch at sign-in (`HKCU\Software\Microsoft\Windows\CurrentVersion\Run\PPVPN`
as `"…\ppvpn.exe" --background`), single instance, close-to-tray, the tray icon
and menu, Mica, the title bar, the light, dark and system theme.

Listener marshalling is App.Core's `SynchronizedListener`: it captures
`SynchronizationContext.Current` (WinUI installs a `DispatcherQueueSynchronizationContext`
in `Program.Main`) and `Post`s every callback. On quit the app log says, for example,
`listener: 129 callbacks, 129 from background threads, 0 delivered off the UI thread`.
Quit (tray menu or the installer's quit event) hides the window and tray icon and awaits
`MainViewModel.ShutdownAsync` (the crate's blocking `shutdown()` on the thread pool)
before exiting.

## Packaging constraint

`ppvpn-service` accepts a client only when its file name is `ppvpn.exe` or
`ppvpn-desktop.exe` **and** it runs from the service's install directory.
Other clients are disconnected at the handshake (`ServiceClientRejected`).
That is why `AssemblyName` is `ppvpn`, and why the installer must put
`ppvpn.exe` and its self-contained runtime in the same directory as
`ppvpn-service.exe`, `ppvpn-core.exe` and the install/uninstall helpers. The
crate talks to the service over `\\.\pipe\ppvpn-service` and to the
standard-mode core over `\\.\pipe\ppvpn-core-user-<32hex>`. The app itself
does not touch either pipe.

## Packaging (NSIS installer)

`scripts/package.ps1` runs on Windows (Windows PowerShell 5.1 is enough) with
the .NET 8 SDK, Rust (`x86_64-pc-windows-msvc`), MSVC and NSIS 3.08+
(`C:\Program Files (x86)\NSIS`, or pass `-Makensis`):

```powershell
powershell -ExecutionPolicy Bypass -File apps\windows\scripts\package.ps1 `
  [-CoreExe <ppvpn-core-windows-amd64.exe> [-CoreSha256 <hex>]] [-BuildNumber N] `
  [-ApiBase <url>] [-FeedUrl <appcast>] [-PublicKey <base64>] [-UpdateKeyFile <private key>] `
  [-Version x.y.z] [-Channel dev|stable]
```

It builds and signs only; it never uploads. `-Version` overrides `VersionPrefix`
(release tags; no pre-release suffixes: dev and stable differ only by channel
and build number). `-Channel` is recorded in the release metadata.

1. Reads `VersionPrefix` and `FileVersion` from `Directory.Build.props`
   (`dotnet msbuild -getProperty`).
2. Builds `ppvpn-service`, `ppvpn-service-install` and `ppvpn-service-uninstall`
   from `service/` (`cargo build --release --target x86_64-pc-windows-msvc`).
   Unsigned builds set `PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT=1`, as the Tauri CI
   did; signed builds need `PPVPN_WINDOWS_PUBLISHER_SHA256`.
3. Builds the crate and its bindings (`crates/ppvpn-client/scripts/build-dotnet.ps1 -Release`),
   then `dotnet publish` (Release, win-x64, self-contained) into `staging/windows-native/app`
   with `-p:PPVPN_API_BASE` from `-ApiBase` (else the environment, else the Release
   default). `runtimes\win-x64\native\ppvpn_client.dll` is where `NativeLoader` loads
   it from; other `runtimes\<rid>` folders are pruned. Then the push agent's NativeAOT
   `dotnet publish`; only `ppvpn-push-agent.exe` is staged (it must have been built
   against the same `ppvpn_client.dll`, which the script checks).
4. Copies `ppvpn-core.exe`: by default the vendored
   `vendor/ppvpn-core/<CURRENT>/build/ppvpn-core-windows-amd64.exe`, checked
   against that release's `manifest.json`. With `-CoreExe`, the sha256 comes
   from `-CoreSha256` or a `windows-SHA256SUMS` next to the file or in its parent.
5. Optional Authenticode signing through `sign-file.ps1`: `WINDOWS_SIGN_COMMAND`
   (with `%1`) or `WINDOWS_CERTIFICATE` (+ password, timestamp URL) for
   `scripts/sign-windows.ps1`. Nothing is signed when neither is set. makensis
   signs the installer and the uninstaller with `!finalize`/`!uninstfinalize`.
   `ppvpn_client.dll` and `ppvpn-push-agent.exe` are signed with the executables.
6. Writes `install-files.txt` (what this version installs) and runs makensis:
   `dist/windows/PPVPN-<version>-windows-x64-setup.exe` and `.sha256`.
7. With an EdDSA private key (`-UpdateKeyFile`, `PPVPN_UPDATE_PRIVATE_KEY_FILE`
   or the base64 key in `PPVPN_UPDATE_PRIVATE_KEY`), prints
   `sparkle:edSignature="…" length="…"` (and verifies it when the public key
   is known), then writes `dist/windows/release-meta-windows-x64.json`
   (see below). Without a key the metadata has no `ed_signature` and a
   warning is printed.

The installer (about 78 MB; the Windows App SDK runtime is most of it):

- Per machine, always `C:\Program Files\PPVPN` (no directory page: the
  privileged service trusts executables from its own directory, so that
  directory must stay admin-write-only).
- Everything in one directory: `ppvpn.exe` and its runtime, `WinSparkle.dll`,
  `runtimes\win-x64\native\ppvpn_client.dll`, `ppvpn-push-agent.exe`,
  `ppvpn-core.exe`, `ppvpn-service.exe`, `ppvpn-service-install.exe`,
  `ppvpn-service-uninstall.exe`, `install-files.txt`, `uninstall.exe`. The
  helpers use the plain names; `ppvpn-service-install.exe` finds
  `ppvpn-service.exe` next to itself, and the service starts `ppvpn-core.exe`
  from its own directory. The service writes `ppvpn-service.log` and
  `ppvpn-core.log` to `%ProgramData%\PPVPN\logs` (5 MB per file, 3 files
  kept), which uninstalling or reinstalling the service leaves in place.
- UI in Chinese or English from the Windows UI language (English is the fallback).
- Install: asks a running `ppvpn.exe` to quit through the named event
  `Local\PPVPN.Desktop.Quit` (the app's normal quit path, backend Shutdown
  included), waits up to 20 s, then kills what is left (other sessions, the old
  Tauri app); asks the push agent to quit (`Local\PPVPN.PushAgent.Quit`), waits up to
  10 s, then kills what is left, so `ppvpn_client.dll` is free; stops `ppvpn_service`; re-registers the service if it points at
  another binary (the Tauri layout used `ppvpn-service-x86_64-pc-windows-msvc.exe`);
  removes the files listed in the previous `install-files.txt` and the Tauri
  sidecar names; copies the files; runs `ppvpn-service-install.exe` (a failure
  shows a localized message and aborts with exit code 2); Start-menu shortcut,
  optional desktop shortcut (a page in the wizard; kept on upgrade;
  `/DESKTOPSHORTCUT` in silent mode), both stamped with the AppUserModelID `PeakPass.PPVPN`; Apps & features entry (same key as the
  Tauri build, so it replaces it). It registers no URL scheme and deletes the
  `ppvpn://` handler the Tauri build left in HKLM.
- The finish page's "Run PPVPN" and the relaunch after a silent upgrade start
  the app through Explorer, so it runs as the signed-in user, not elevated.
  A silent install relaunches the app only when it was running in this session.
  The push agent is started the same way after every install when its Run value
  `PPVPNPushAgent` exists for the installing user (the user is signed in to the app).
- Uninstall: quits the app and the push agent, runs `ppvpn-service-uninstall.exe`, removes the
  files, shortcuts, the Apps & features entry, the launch-at-sign-in value
  `HKCU\…\Run\PPVPN` and the push agent's `HKCU\…\Run\PPVPNPushAgent`, the notification registration
  (`HKCU\Software\Classes\AppUserModelId\PeakPass.PPVPN` and the activator's
  `HKCU\Software\Classes\CLSID\{FCD3C3FA-…}`) and old `ppvpn://` handlers. Per-user data is kept unless
  "Also remove my sign-in, settings and logs" is ticked (or `/PURGE` with `/S`):
  then `%LOCALAPPDATA%\PPVPN`, the `PPVPN/desktop.credentials` credential (and the
  fake backend's `PPVPN/desktop.credentials.fake`),
  `HKCU\Software\PPVPN` (WinSparkle state) and `C:\ProgramData\PPVPN`
  (service core state) go too.
- Silent: `/S` for install and uninstall.

### CI

`.github/workflows/desktop-native-windows.yml` (mirrors `macos-native.yml`)
runs on pull requests and pushes to main/dev that touch the Windows app, the
client crate, the service or the vendored core, on `workflow_dispatch`, and as
a reusable workflow (`workflow_call`: `channel`, `api_base`, optional `version`
and `build_number`; secret `SPARKLE_PRIVATE_KEY`; output `artifact`).

- `test`: `cargo test` for `crates/ppvpn-client`, its .NET bindings
  (`build-dotnet.ps1 -Release`; `uniffi-bindgen-cs` is installed and cached),
  `dotnet test apps/shared/PPVPN.App.Core.Tests`, then a warning-free
  `dotnet build` of the app (`-warnaserror`; NuGet advisories NU1901/NU1902, low and
  moderate, stay warnings, `Directory.Build.props`), and a warning-free NativeAOT
  `dotnet publish` of the push agent.
- `package`: `scripts/ci-build.ps1`, which verifies the vendored core with
  `scripts/verify-vendored-core.mjs` and runs `package.ps1` with `-ApiBase <api_base>`
  (the app's backend) and, for a channel build, the feed
  `<PPVPN_UPDATE_SITE>/desktop/<channel>/appcast-windows-x64.xml`,
  the public key from `vars.PPVPN_SPARKLE_PUBLIC_KEY` and the private key from
  `SPARKLE_PRIVATE_KEY` (written to a temporary file that is deleted afterwards,
  never printed; not available to pull requests). A build with a channel fails
  without both keys. The API base defaults like macOS: pull requests use the
  repository variable `PPVPN_DEV_API_BASE`, everything else `https://www.peakpassvpn.com`.
  The build number defaults to the run number.
- Uploads the artifact `desktop-native-windows-x64`: the setup exe and
  `release-meta-windows-x64.json`. The caller's publish job puts them in R2.
- Authenticode stays off unless `WINDOWS_CERTIFICATE` (+ password secret) and
  `vars.WINDOWS_TIMESTAMP_URL` / `vars.PPVPN_WINDOWS_PUBLISHER_SHA256` are set.

`ci-build.ps1` takes the same values as parameters or environment variables
(`PPVPN_RELEASE_CHANNEL`, `PPVPN_API_BASE`, `PPVPN_VERSION`, `PPVPN_BUILD_NUMBER`,
`PPVPN_SPARKLE_PUBLIC_KEY`, `SPARKLE_PRIVATE_KEY`), so it runs locally too.

## Updates

WinSparkle (native `WinSparkle.dll` from the `WinSparkle` NuGet package, called
through P/Invoke in `Platform/AppUpdater.cs`). It reads a Sparkle-format
appcast, shows its own update dialog (Chinese or English, like the app),
downloads the installer, checks its EdDSA (ed25519) signature and runs it with
`sparkle:installerArguments` (`/S`).

- Build properties (MSBuild `-p:` or environment variables of the same name),
  embedded as assembly metadata:
  - `PPVPN_UPDATE_FEED_URL`: no default; `ci-build.ps1` passes the channel's
    feed on the update site.
  - `PPVPN_UPDATE_PUBLIC_KEY`: base64 ed25519 public key; no default.
  - Without both, updates are disabled and Settings → About hides
    "Check for Updates" (like the macOS app).
- Settings → About → "检查更新" runs a check with UI. WinSparkle also checks
  in the background at startup on its own schedule (once a day); its state lives
  in `HKCU\Software\PPVPN\WinSparkle`.
- The appcast is a file on the update site. Its enclosure URL is the
  installer's GitHub Release asset, which redirects (302) to GitHub's storage.
  WinSparkle follows the redirect and checks the signature of the downloaded
  bytes. Tested on the VM with a local feed: a tampered installer and an
  enclosure without a signature are both rejected ("更新未正确签名") and
  nothing runs.
- Quitting for the installer: WinSparkle starts the installer and calls the
  shutdown callback. The app hides its window and tray icon at once and waits
  for the installer's quit event, so the installer sees it running, stops it
  through the normal quit path and relaunches it after the upgrade. If no
  event arrives within 30 s, the app quits on its own.
- WinSparkle is pinned to 0.9.2: 0.9.3 and 0.9.4 no longer load their Chinese
  translations (German still works), so Chinese Windows got an English dialog.

### Keys

One ed25519 key pair signs updates for Windows and macOS (CI secret). Both
tools use the same key format (base64 of the 32-byte seed):

```powershell
# WinSparkle (in the NuGet package: %USERPROFILE%\.nuget\packages\winsparkle\0.9.2\tools)
winsparkle-tool generate-key --file eddsa-private.key   # prints the public key
winsparkle-tool public-key --private-key-file eddsa-private.key
winsparkle-tool sign --private-key-file eddsa-private.key PPVPN-0.3.1-windows-x64-setup.exe
```

On macOS, Sparkle's `generate_keys` creates the key in the keychain and
`generate_keys -x eddsa-private.key` exports it in the same format. Pass the
public key as `PPVPN_UPDATE_PUBLIC_KEY` and the private key to `package.ps1`.

### Release metadata

`package.ps1` writes `dist/windows/release-meta-windows-x64.json` next to the
installer. Both become assets of the GitHub Release; the update site's appcast
is written from the JSON (`scripts/appcast-example.xml` shows the mapping). macOS uses the same
format.

```json
{
  "schema": 1,
  "platform": "windows-x64",
  "channel": "dev",
  "version": "0.3.1",
  "build": "0.3.1.2",
  "file": "PPVPN-0.3.1-windows-x64-setup.exe",
  "length": 78140672,
  "sha256": "<64 hex characters>",
  "ed_signature": "<base64 EdDSA signature>",
  "min_os": "10.0.17763",
  "installer_arguments": "/S",
  "published_at": "2026-09-29T12:47:51Z"
}
```

| Field | Meaning | Appcast |
| --- | --- | --- |
| `schema` | Format version, 1 | |
| `platform` | `windows-x64` | enclosure `sparkle:os` |
| `channel` | `dev` or `stable`; omitted for builds without a channel (pull requests) | (selects the feed) |
| `version` | User-visible version (`VersionPrefix`) | `sparkle:shortVersionString` |
| `build` | Monotonic `FileVersion` (`VersionPrefix.PpvpnBuildNumber`) that WinSparkle compares | `sparkle:version` |
| `file` | Installer file name in R2 | (backend download redirect) |
| `length` | Installer size in bytes | enclosure `length` |
| `sha256` | Installer sha256, hex | (backend check) |
| `ed_signature` | Base64 EdDSA signature of the installer; missing when no key was given | enclosure `sparkle:edSignature` |
| `min_os` | Minimum Windows version | `sparkle:minimumSystemVersion` |
| `installer_arguments` | `/S` | enclosure `sparkle:installerArguments` |
| `published_at` | RFC 3339 UTC | `pubDate` (RFC 822) |

Versioning: `VersionPrefix` in `Directory.Build.props` is the version;
`PpvpnBuildNumber` (default 1, CI may pass `-BuildNumber`) is the fourth part
of `FileVersion`. `build` must grow with every published installer.

## Known issues and notes

- **H.NotifyIcon.WinUI 2.3.2** is the last release that targets net8, and it
  was built against Windows App SDK 1.6. It has two quirks:
  - Its MenuFlyout-to-PopupMenu conversion does not invoke item commands, so
    `TrayIconHost` builds the native menu itself with `H.NotifyIcon.Core.PopupMenu`.
  - `TaskbarIcon.Icon` takes ownership and disposes the previous `Icon`, so
    a fresh `Icon` is loaded on every change.
- Status colours (green, amber, red) are resolved in code (`Ui.ToneBrush`)
  because theme dictionaries looked up from code ignore a per-window
  `RequestedTheme`. Pages refresh them on `ActualThemeChanged`.
- `x:Bind` function bindings are not re-evaluated when an argument becomes
  `null`, so bind visibility to bool properties (`HasReason`), not to
  `F(string?)`.
- Mica renders as a flat colour on the VM (no GPU). On real hardware it shows
  the wallpaper tint.
- Screenshots taken from scheduled-task launches show a keyboard focus
  rectangle on the first focusable control. There is no pointer input there,
  so WinUI assumes keyboard focus. This does not happen with normal use.
- The UI language follows the Windows display language: zh-CN, en-US, and
  zh-CN as the default fallback for the XAML texts. App.Core's `JsonLocalizer` picks
  zh-CN for any `zh-*` UI culture and en-US otherwise.
- Do not add a namespace or folder named `Client` to this app: it would make
  `PPVPN.Ffi.Client` ambiguous.
