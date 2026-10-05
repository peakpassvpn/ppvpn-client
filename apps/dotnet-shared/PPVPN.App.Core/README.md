# PPVPN.App.Core

Platform-neutral view models for the .NET desktop apps: Windows (WinUI 3) and
Linux (GTK 4 through Gir.Core). It targets `net8.0`, depends only on
`CommunityToolkit.Mvvm` and `../PPVPN.Client`, and references no UI toolkit.
The UI follows the native desktop design handoff, which is not part of this
repository; its string catalog is the test fixture
`PPVPN.App.Core.Tests/Fixtures/design-strings.json`. There is one product
change: a single **Connect** switch plus a connection mode (Enhanced /
Compatible) instead of the design's two switches.

## Layering

```
crates/desktop (Rust)
  └─ PPVPN.Client          generated UniFFI bindings, namespace PPVPN.Ffi
       └─ PPVPN.App.Core
            Backend/       IClientBackend: the Client API as an interface
                           FfiClientBackend (real) · FakeClientBackend (previews, tests)
            ViewModels/    MainViewModel (+ .Auth / .Connection / .Account / .Tray),
                           NodesViewModel, InboxViewModel, LogsViewModel, SettingsViewModel,
                           SynchronizedListener, Formatting / ErrorMessages, JsonLocalizer,
                           Abstractions (platform seams)
            Strings/       strings.json: the shared catalog (design keys + Error_*)
            tools/         check-xcstrings.py: macOS catalog drift check
                 └─ platform UI   WinUI 3 pages / GTK 4 widgets bind to the view models
```

- **PPVPN.Ffi** holds the data types (`ClientSnapshot`, `Node`, `ProfileStatus`,
  `ErrorCode`, …). App.Core uses them directly.
- **IClientBackend** mirrors `PPVPN.Ffi.Client` one to one, except that the
  blocking `shutdown()` is only `ShutdownAsync()`. `FfiClientBackend` wraps the
  generated client.
- **FakeClientBackend** is an in-process stand-in with the design's sample data
  (11 nodes, lines with labels, teams including a disabled one, the message
  samples) and realistic timing. `FakeOptions` drives the scenarios: login
  outcome, profile status per team, failed / occupied connect, failed system
  proxy, the connection method, message counts, `TimeScale`. `SetScenario`,
  `SetTeamActive`, `SimulateOccupied` and `PushMessage` change it at run time.
- **View models** hold all UI state. They derive it from the snapshots and the
  listener callbacks; views keep no connection state of their own.

```csharp
var strings = new JsonLocalizer(log);
var main = new MainViewModel(
    FfiClientBackend.Factory(config, platformHooks),   // or FakeClientBackend.Factory(...)
    services, settings, strings, log, prompts);        // optional: a TimeProvider
main.ShowRequested += (surface, messageId) => …;       // show a window
main.QuitRequested += async () => { await main.ShutdownAsync(); exit(); };
```

Create it on the UI thread. The factory receives a `SynchronizedListener`,
which captures the UI thread's `SynchronizationContext` and posts every crate
callback there, in order. The constructor reads the initial state with
`Snapshot()`; the crate never pushes the initial state or an unchanged one.

## What the UI binds

### Title bar, navigation, surfaces

| Member | Use |
|---|---|
| `StatusLine`, `StatusDotTone` | Short status line ("已连接", "正在连接", "未连接", or the failure/contention title; never the node name) + dot (Idle while signed out / restricted). Platforms show it with a small state icon and truncate it. |
| `ShowLogin`, `IsSignedIn`, `IsRestoring` | Login page vs. main content; hide nav, bell and account while signed out. |
| `UnreadNotifications`, `HasUnreadNotifications`, `UnreadBadgeText` | Bell badge (hidden at 0, "99+"). |
| `AccountEmail`, `AvatarInitial`, `TeamName`, `AccountSubtitle` | Account button and menu header. |
| `Teams` (`TeamOption`: `Title`, `IsCurrent`, `IsSelectable`, `DisabledTag`), `SwitchTeamCommand(TeamOption)` | Account menu radio items. |
| `OpenAccountSettingsCommand`, `SignOutCommand` | Menu items (sign-out confirms first). |
| `ShowRequested(AppSurface, ulong?)` | Show Main / Settings / MessageCenter. The id is informational: the platform only shows the window; App.Core opens (and marks read) the message itself. |
| `OpenMainCommand`, `OpenSettingsCommand`, `OpenMessagesCommand`, `CheckForUpdatesCommand`, `QuitCommand` | Tray and shortcuts. |
| `RefreshProfileCommand` | F5 on Overview / Nodes; `Inbox.RefreshCommand` in the message center; `Logs.ReloadCommand` on Logs. |

### Login

`Stage`, `UserCode`, `VerificationUrl`, `CountdownText` (`expiresIn`),
`BrowserNotOpened` (platform extra), `IsStartingLogin`; errors: `LoginErrorKind`
(Expired / Denied / Network / Other / StoreLocked), `LoginErrorTitle`, `LoginErrorMessage`,
`LoginRetryText` (`tryAgain` / `signInAgain`) with `SignInCommand`. Commands:
`SignInCommand`, `CancelSignInCommand`, `ReopenBrowserCommand`, `CopyCodeCommand`.
The local countdown reaching 0 cancels the login and shows the expired error.
StoreLocked (`CredentialStoreLocked`: the keychain / keyring holding the saved
login is locked) shows `errLockedT` + `Error_CredentialStoreLocked` ("unlock and
you'll be signed in again") with `tryAgain`; the same `SignInCommand` then calls
`RetryCredentialRestore()` (read the store now) instead of starting a sign-in,
and only signs in when no restore is pending.

### Overview

- **Connection card header** (the status header and the Connect switch are one
  block): `ConnectionTitle`, `ConnectionDetail`,
  `ConnectionTone` (`IsConnectionBusy` spins the circle, `IsConnectionError`
  colours the title), `CurrentNodeName`, `CurrentNodeCountryCode` (flag).
  Enhanced method: the design's enhanced titles (`h_on`, `st_connecting` +
  `d_connecting {r}`, …, `h_idle` / `d_idle` when off). Compatible: the
  system-proxy ones (`h_stdStarting`, `h_on` + `d_stdOn {r} · {ms}`,
  `h_stdFail` + `fr_port`). `{r}` is the line label. Endpoint keys are internal
  and never shown: without a label `d_on` / `d_stdOn` become just the latency
  ("38 ms"), `d_connecting` is empty, and `d_reconnecting` falls back to
  `d_connecting {r}` (no old label) or empty (no new label). An empty
  `ConnectionDetail` means no " · " after the node name.
  While the standard core has failed (`Snapshot.Standard` is `Failed`) the off
  detail in both methods is `d_idleProxyFailed` instead of `d_idle`.
- **Switch in the header** (right side): `ConnectSwitch` (`SwitchVisual` Off /
  On / Indeterminate) + `ConnectSwitchEnabled`, `ToggleConnectCommand`; below
  the header one weakest-tone line `ConnectionMethodCaption` ("增强模式"
  / "兼容模式 · 设置系统代理"). Indeterminate while preparing,
  authorizing, connecting, reconnecting and disconnecting; disabled while
  preparing, authorizing and disconnecting (a tap while connecting cancels).
  After a cancelled prompt the VM re-raises `ConnectSwitch` so the control
  snaps back.
- **Install hint:** `ShowInstallHint` (Enhanced, service missing) +
  `InstallServiceCommand` (`installBtn`, UAC shield); `IsInstallingService`.
- **Current node:** combo over `Nodes.Items` with `CurrentNode` (two-way; each
  item has `Name`, `CountryCode`, `LatencyText`, `LatencyKind`).
- **Notices** (`Notices`, `ConnectionNotice`): `Title`, `Message`, `Tone`, and
  up to two buttons (`ActionText`/`Action`, `SecondaryActionText`/`SecondaryAction`):
  failed → `retry` (+ `useCompatible` when compatibility mode would avoid the
  failure), occupied → `takeOver` (none for another OS user), and
  `NoticeKind.ProxyFailed` (tone Error) while the standard core has failed:
  title `proxyFailedT`, message the localized error, `retry` →
  `RetryLocalProxyCommand` (refetches the profile). It can sit next to a
  failed notice and clears when the core is ready again. Bind every kind; pick
  the icon by `Kind` / `Tone` (ProxyFailed and Conflict use the error icon).
- **Conflict** (`NoticeKind.Conflict`, tone Error): a failed connect is one
  notice; when the crate names another proxy/VPN app competing for the network
  path (`ConnectionState.competitor`), that notice is the conflict notice
  instead of the failed one: title `conflictT`, message `conflictD {app}`
  ("Surge 正在接管本机网络…"), `retry` → `RetryConnectCommand`, and
  `useCompatible` → `UseCompatibleCommand` when `suggest_compatible` is set
  (NetworkPathContended, ConnectHealthCheckFailed, a cancelled / failed
  service install, and any enhanced failure while detection found another
  app's tunnel). The enhanced headline is then `h_pathContended`. "Taken
  over" is said only when an app is named: NetworkPathContended with
  `competitor` empty is an ordinary failure (headline `h_failed`, message
  `fr_timeout`), e.g. a reconnect that failed after the service restarted.
  The crate names a competitor only while enhanced mode is contended or failed
  on the path (`NetworkPathContended`, `ConnectHealthCheckFailed`, a
  `ConnectFailed` `HEALTH_*` step such as `HEALTH_ENTRANCE_FAILED`) and
  detection saw a tunnel signal (another app's fake-IP 198.18.0.0/15 tunnel,
  its tunnel holding the default or split-default route, or fake-IP DNS); it
  arrives a moment after the failure (detection runs then), so the notice
  switches from Failed to Conflict in place. The pre-connect check is the
  exception: when another app's tunnel already owns the route or DNS, the
  crate does not start TUN at all and the connection lands directly in
  `Error{NetworkPathContended, "PREFLIGHT_CONFLICT: …"}` with `competitor`,
  `suggest_compatible` and `retryable` set (a Conflict notice from the
  start). A mesh VPN (Tailscale, ZeroTier) without the default route never
  blocks.
- **Compatible-mode failure text:** `SystemProxyFailed` → `fr_port`;
  `SystemProxyUnavailable` (the desktop has no proxy settings, e.g. Linux
  without `gsettings` / `kwriteconfig`; not retryable) → `sysproxyUnavailable`,
  both as the detail line and the failed notice's message. In compatible mode `competitor` / `proxy_was_foreign` say whose OS
  proxy was replaced (restored on disconnect); that is not a failure and has
  no notice. `Client::detect_conflicts()` (the full `ConflictReport`) is not
  used by the view models.
- **Routing rules** (`NoticeKind.RulesUnavailable`, tone Warn, no buttons):
  while `ClientSnapshot.rule_sets_unavailable` is non-empty (signed in, not
  restricted, any connection state), title `rulesT`, message
  `rulesUnavailableD` ("部分分流规则暂未加载，相关流量暂时走代理。"). It
  comes after the other notices and clears by itself once the core loads the
  rule sets; `stale` rule sets still route and show nothing.
- **Traffic:** `ShowTraffic`, `UpRate`, `DownRate` ("8.6 MB/s", bytes).
- **Local proxy:** `CurrentNodeProxy` (`ProxyInfo`: `HttpDisplay`,
  `SocksDisplay`, `Username`, `MaskedPassword`, `PasswordDisplay` / `PasswordRevealed` (eye toggle `TogglePasswordRevealedCommand`, masked by default), `CopyUsernameCommand`, `CopyPasswordCommand`, `CopyHttpCommand`,
  `CopySocksCommand` copying the full URL; the ✓ is view-local) or
  `ProxyUnavailableText` (`proxyStarting`, or `proxyFailed {reason}` while the
  standard core has failed; `CurrentNodeProxy` is then null even if the node
  still lists a proxy, since nothing listens on the port).
- **Restricted** (replaces card, notices, traffic, proxy): `IsRestricted`,
  `Access`, `RestrictedTitle`, `RestrictedMessage`, `CanBuy` →
  `OpenPurchaseCommand` (`buy` ↗) + `RefreshAccessCommand` (shows "…" while
  `IsRefreshingAccess`), `CanSwitchTeam` → the account menu.
- **Configuration invalid:** `ConfigInvalid` (`cfgT` / `cfgD` warning).

### Nodes

`Nodes.ViewState` (Data / Loading / InvalidNoHistory / Restricted),
`Nodes.ConfigInvalid`, `Items` (flat, profile order; `Name`, `CountryCode`,
`Tier` / `HasTier`, `Region`, `Routes` "HKG-A → HKG-B → SZX-R" (a line
without a label is `routeN` by failover position, "线路 2"; a single unlabelled
line gives ""; never endpoint keys), `IsCurrent`,
`LatencyKind` / `LatencyText` / `LatencyTone` / `LatencyTooltip`, `Proxy`),
`SelectedItem` (two-way row focus), `SelectedNodeProxy` + `SelectedNodeProxyTitle`
(bottom card), `MethodIndex` (Ping / TCP / Connect), `TestAllCommand`,
`RefreshCommand`, `IsProbing`, `CanProbe`, `NodeCountText`, `TestingText`.
Per row: `SetCurrentCommand` (double-click; disabled for the current node),
`TestOneCommand`, `CopyHttpCommand`, `CopySocksCommand`. The hint text is the
static key `nodesHint`.

### Logs

`Logs.Files` (client / core × today / yesterday; the crate names files by UTC
date, so "today" and "yesterday" are UTC dates),
`SelectedFile`, `Lines` (appended; `LinesAppended` to scroll), `IsFollowing`,
`IsMissing`, `DisplayPath`, `OpenFolderCommand`, `ReloadCommand`. Call
`Start()` when the page shows and `Stop()` when it hides.

Parsed view (render this rather than raw `Lines`): `Entries` (every line parsed
by `LogParser`; a line matching no format continues the previous entry, e.g. a
panic's backtrace) and `VisibleEntries` (those passing the filter; updated in
place as lines arrive). A `LogEntry` has `TimeText` ("HH:mm:ss.fff", local),
`Severity` / `SeverityText` ("INFO", "WARN", …; empty when unparsed), `Tone`
(`StatusTone`: error `Bad`, warning `Caution`, else `Neutral`), `IsDim`
(debug/trace), `Source` (the Rust target, client log only), `Message`, `Fields`
(`LogField(Key, Value)`, values unquoted) / `HasFields` / `FieldsText`, and
`Raw`. Filter: `SeverityOptions` + `SelectedSeverity` (two-way; keys
`logAllLevels` … `logErrorOnly`) or `MinimumSeverity`, `SearchText` (placeholder
`logSearch`; message, source and fields, case-insensitive), `HasNoMatches`
(`logNoMatches`). `CopyVisibleCommand` (`copyShownLogs`) copies the shown
entries' original text.

### Settings

`Settings.ShowAccountSection` (signed in only), General: `AppearanceIndex`,
`LaunchAtLogin`, `LaunchAtLoginNeedsApproval` + `OpenLaunchAtLoginSettingsCommand`,
`AutoCheckUpdates`, `CheckForUpdatesCommand`, `ShowUpdates`, `VersionText`.
Account: `Main.AccountEmail`, `Main.Teams` / `Main.SelectedTeam`,
`Main.ExpiresText` / `Main.IsExpired`, `Main.SignOutCommand`. Advanced:
`ConnectionModes` (`ConnectionModeOption`: `Mode`, `Title`, `Description`) with
`SelectedConnectionMode` (two-way) or `SetConnectionModeCommand`,
`RoutingModes` (`RoutingModeOption`: `Mode`, `Title`, `Description`; group
title `routingMode`) with `SelectedRoutingMode` (two-way) or
`SetRoutingModeCommand` (rules / global, applied without a reconnect;
`Main.RoutingMode` is the current one),
`ApiBaseOverride` + `ApiPlaceholder`, `Main.ServiceStatusText`,
`Main.InstallServiceCommand`, `Main.UninstallServiceCommand`. Call `Refresh()`
when the page shows.

### Message center

`Inbox.State` (FirstLoad → 6 skeleton rows / Ready / Empty), `HasLoadError` +
`RetryCommand`, `HeaderText`, `CanMarkAllRead` + `MarkAllReadCommand`,
`Messages`, `LoadMoreCommand` (within 24 px of the bottom), `IsLoadingMore`,
`IsExhausted`. Row: `Title`, `Content`, `IsUnread`, `Tone` (bar / icon tint),
`Type` + `TypeLabel`, `MetaText`, `RelativeTime`, `HasLink` +
`OpenLinkCommand`, `MarkReadCommand`, `ShowDetailCommand`. Detail: `Detail`,
`IsDetailOpen`, `PositionText` ("1 / 46"), `CanPrevious` / `PreviousCommand`,
`CanNext` / `NextCommand` (loads the next page at the end), `BackCommand`,
`ToggleDetailReadCommand` + `DetailReadActionText`; the item's
`DetailTimeText`. Relative times refresh every 60 s. `IsDetailReadOnly`: the
detail is a clicked push without an inbox message (`Detail.IsReadOnly`,
`Detail.PushId`): show title, content, time and severity only, and hide the
position, previous / next and the read toggle.

The type icon and label come from the crate's `InboxMessage.Category`
(`MessageTypes.LabelKey`: `t_expiring` … `t_broadcast`, `t_other`).

**Live refresh.** When `Snapshot.UnreadNotifications` rises (a push, or the
crate's unread poll) while signed in and the list is loaded (`IsLoaded`),
App.Core re-fetches page 1 after a ~1 s debounce (a burst of increases makes
one request; if a load is running it waits for it) and merges it: new messages
are prepended, the read state of loaded ones is updated, later pages stay
loaded, and `Detail` (a message being read, or a read-only push) stays open.
If more than a page arrived, the list restarts at page 1, still keeping
`Detail`. A falling count (read here or mark-all-read) does not reload, and a
failed background fetch is only logged (no error bar). Platforms need no
change: it runs off the existing snapshot, with no new binding or command.

### Tray

`Tray` (`TrayMenu`: `Icon` Off / Busy / On / Error, `ToolTip`, `Items`). Items
are plain data (`TrayItem`: `Kind`, `Role`, `Text`, `Secondary`, `IsChecked`,
`IsEnabled`, `Command` + `CommandParameter`, `Children`, `Dot`, `BadgeDot`), so
Windows can rebuild its Win32 popup on open and Linux its DBusMenu. Signed in:
header, **Connect** (secondary: the state), current node submenu, the unread
line; restricted: its title + the unread line; signed out: `notSignedIn`; restoring the saved login at launch: empty (and the tray shows no signed-out row).
Always: open, settings (`SettingsMenuKey`: `settingsMenuWin` / `preferences`),
check for updates, quit.

## Rules the view models follow

- **Feedback.** Persistent states render in the content (restricted card,
  warnings, notices, install hint). Failures of user actions become a
  `IUserPrompts.ShowErrorAsync` dialog (`errorT`, or `switchFailT`,
  `installFailT`, `uninstallFailT`, `launchFailT`). Cancelled actions,
  failures the content already shows, and background failures are logged only.
  In particular a connection command (connect, retry, take over, set the
  mode, use compatibility mode) that fails while the backend's current
  snapshot is Failed / Occupied is logged only: its notice shows the reason.
  Decide from `Backend.Snapshot()`, not the last applied snapshot, since the
  command's error returns before that snapshot reaches the UI thread.
  There is no banner.
- **Install flow.** Connect in Enhanced without the service →
  `ConfirmAsync(InstallService)`; cancel leaves the switch off (not failed).
  Denied at the OS prompt → failed + `fr_auth` + Retry (+ Use Compatibility
  Mode). The standalone install (hint, Settings) never fails the connection.
- **Connection method.** Changing it while connected reconnects in the new
  method; "Use Compatibility Mode" sets it and connects.
- **Local proxy** is independent of the connection: one shared port (7890 when
  free), the user name picks the node, the password is per device.
- **Notifications.** OS notifications come from the push agent. The main app
  only refreshes a loaded list when the unread count rises (see Message center); `Inbox.ActivateNotification(pushId)`
  is the IPC entry for clicks. A notification carries only the push id (the
  push queue id, not an inbox message id), resolved through the crate's
  `shown_push` (what the agent recorded when the OS showed it): an absolute
  http(s) `deep_link` is opened (and `message_id`, if any, marked read);
  else a `message_id` opens the message center on that message; else, or
  when the server no longer has it, the push shows as a read-only detail
  (no inbox calls, nothing marked read); an unknown push id just opens the
  message center. No link from anywhere else is ever opened.

## Platform seams

| Seam | Notes |
|---|---|
| `PPVPN.Ffi.PlatformHooks` | Credential store (see below), `OpenUrl`, privileged-service install / uninstall / query (UAC / polkit). Called from background threads. |
| `IAppServices` | Version + build number, default API base, clipboard, open URL / folder, launch at login (+ blocked by the system, open its settings), appearance, updates (check, auto-check). |
| `ISettingsStore` | Appearance, API override, `BackgroundHintShown` (first-close notice), window placement per window. |
| `IUserPrompts` | `ConfirmAsync(PromptKind)` (texts: `PromptTexts.For(kind, strings)`) and `ShowErrorAsync(title, message)`. ContentDialog / AdwAlertDialog; show the main window first when hidden; one dialog at a time. |
| `ILocalizer` | `JsonLocalizer`. |
| `IAppLog` | The app's own log. |
| `TimeProvider` (optional) | The clock for the countdown, relative times and log dates. |
| `SynchronizationContext` on the UI thread | WinUI installs `DispatcherQueueSynchronizationContext`. On GTK, install one whose `Post` uses GLib idle-add at default priority (FIFO), before creating `MainViewModel`. |

### Credential store errors

`CredentialLoad` returns null only when nothing is saved (that signs the user
out). A store that can't be read right now throws, and the crate keeps the saved
login, shows the error on the login page and re-reads the store in the
background (every 30 s, after 10 min every 5 min) until it works:

- `PlatformException.Locked(message)`: the store is **locked** (keychain,
  keyring). Shown as `CredentialStoreLocked` ("unlock and you'll be signed in
  again", **Retry**). Throw it from `CredentialLoad` / `CredentialSave` /
  `CredentialDelete` whenever the platform says so; `message` is for logs.
- `PlatformException.Failed(message)`: anything else; shown as
  `CredentialStoreFailed`.

Because of the background retries, `CredentialLoad` must not show an unlock
prompt on every call: after the user dismisses one, keep throwing `Locked`
without prompting until the store is unlocked some other way. **Retry** calls
`RetryCredentialRestore()`, which reads once more right away; a platform that
wants that read to prompt again resets its own "dismissed" flag before.

| Platform | Store | Locked |
|---|---|---|
| Linux | Secret Service (libsecret) | A matching item exists but is locked (unlock prompt dismissed): throw `Locked("LINUX_KEYRING_LOCKED: …")`, not "no credential". |
| Windows | Credential Manager | Not lockable while the user is signed in to Windows; always `Failed`. |
| macOS | Keychain item (a file, never locked, in Debug / file-store builds) | Not wired yet (later): a keychain read that returns `errSecInteractionNotAllowed` (-25308, locked keychain, no UI allowed) should throw `PlatformError.Locked`; everything else stays `Failed`. |

## Strings

`Strings/strings.json` is the design handoff's `strings.json` (same keys, same
`{"zh": {…}, "en": {…}}` shape) plus `Error_<ErrorCode>` texts and keys the
design lacks (`checkNow`, `nodesHint`, `expiredOn`, `errorT`, `installFailT`,
`uninstallFailT`, `launchFailT`, `testingWith`, `currentNodeIs`,
`proxyStarting`, `proxyFailed`, `proxyFailedT`, `d_idleProxyFailed`, `routeN {n}`,
`rulesT`, `rulesUnavailableD`, and the connection mode: `connect`,
`captionEnhanced`, `captionCompatible`,
`connMethod`, `connMethodHint`, `methodEnhanced(D)`, `methodCompatible(D)`,
`useCompatible`). Sample data became placeholders: `expiredD {d}`,
`teamOffD {team}`, `version {v} {b}`, `switchFailD {reason}` (the localized
error). It is the single source for both apps, static labels included: Windows
reads it at runtime (no resw), Linux through `JsonLocalizer`.

```csharp
var strings = new JsonLocalizer(log);            // zh-* UI culture → zh, else en
strings.Get("loginTitle");
strings.Format("d_on", ("r", "HKG-A"), ("ms", "38 ms"));
```

Placeholders are named (`Placeholders.Fill`). A key missing from zh falls back
to en; one missing from both is returned as the key and logged once.
`StringCatalogTests` checks that every design key and every key the view
models use exists in both languages with the same placeholders, and that only
the platform-specific keys (`revealWin`, `winLaunchApprove`, `uac*`, …) name a
platform.

macOS keeps its own `.xcstrings`. `tools/check-xcstrings.py` reports where
they drift from strings.json (missing Chinese source texts, English
differences, `Errors.xcstrings` against `Error_*`); `--strict` exits 1 for CI.

## Crate API used

Snapshot: `connection_mode`, `connection` (`ConnectionState`: phase, reason,
retryable, can_take_over, suggest_compatible, competitor, `detail` with endpoint key /
label / previous key / latency; the standard core's path while off),
`service_installed`, `ProfileStatus.SubscriptionExpired { expired_at }`,
`Account.email`, `Replica.label`, `InboxMessage.category`,
`PushMessage.message_id`, `rule_sets_unavailable`. Calls: `connect`,
`disconnect`, `retry`, `set_connection_mode`, `enhanced_take_over`,
`service_install` (Cancelled on a dismissed prompt), `service_uninstall`,
`mark_notification_unread`, `shown_push` (`FakeClientBackend.RecordShownPush`
stands in for the agent). `FakeClientBackend` simulates all of them,
including compatible mode's `SystemProxyFailed` and, with
`FakeOptions.ConflictOnFirstConnect`, Surge's enhanced mode
(ConnectHealthCheckFailed with `competitor` "Surge"), and with
`FakeOptions.UnavailableRuleSets` rule sets that load after
`RuleSetsLoadAfter` (the routing-rules notice).

### Routing rule sets (core 0.5.0)

What the crate does (nothing for the apps to call):

- Every `apply-profile` pins rule-set downloads to the API the profile came
  from: `allowed_rule_set_hosts: ["<authority of ClientConfig.api_base>"]`
  (`api.example.com`; `host:port` for a port other than 443; no scheme or
  path). The standard core gets it directly; the enhanced core through the
  privileged service (Connect / UpdateProfile carry the list; service 0.4.0
  forwards it, older services ignore it and apply the bare profile).
- `GetStatus.rule_sets` of the core in use (the enhanced core's while it is
  on, otherwise the standard core's) is read every 5 s by the connection
  monitor; the ids in state `unavailable` become
  `ClientSnapshot.rule_sets_unavailable` (profile order, empty when the
  profile has none, cleared on sign-out). `RuleSetChanged` events fold into
  the same field (`Client::on_core_event`); the crate has no event stream
  yet, so the poll is what the apps see.
- A host mismatch (`RULE_SET_HOST_NOT_ALLOWED`) is a backend
  misconfiguration: the crate treats it, `RULE_SET_HOSTS_INVALID` and the
  rule-set validation codes like any other invalid profile
  (`ProfileInvalid`), not a retryable core failure.

What each platform binds or ports:

- **Windows (WinUI 3)** and **Linux (GTK)** run on App.Core: regenerate the
  bindings (`ClientSnapshot` gained `RuleSetsUnavailable`, a `string[]`, as
  its last field). Windows needs nothing else: the notice template picks the
  warning glyph by `Tone` and hides the empty buttons. Linux picks the icon
  by `Kind`: `RulesUnavailable` falls through to `dialog-error-symbolic`;
  map it to `dialog-warning-symbolic` (the card is already tone Warn).
  Optional: a `--fake-rule-sets` start option mapping to
  `FakeOptions.UnavailableRuleSets`.
- **macOS (Swift, ports AppLogic by hand)**: regenerate the Swift bindings
  (`ClientSnapshot.ruleSetsUnavailable: [String]`, last field), then in
  `Presentation.swift`, after the `proxyFailed` notice:

  ```swift
  if isSignedIn, restricted == nil, !snapshot.ruleSetsUnavailable.isEmpty {
      notices.append(Notice(
          id: "rulesUnavailable", tone: .warn, systemImage: "arrow.triangle.branch",
          title: tr("rulesT"), message: tr("rulesUnavailableD"),
          actionTitle: nil, action: nil))
  }
  ```

  Add
  `rulesT` ("分流规则" / "Routing Rules") and `rulesUnavailableD` (the zh /
  en texts in strings.json) to `Localizable.xcstrings`
  (`tools/check-xcstrings.py` reports them until then), update the
  `ClientSnapshot(…)` initializers in `PreviewBackend.swift` and
  `Tests/PPVPNAppLogicTests/Support.swift` (`ruleSetsUnavailable: []`), and
  add a preview scenario and a `PresentationTests` case: the notice with one
  unavailable id, none when the list is empty or the account is restricted.

### Competitor detection: what each platform binds or ports

- **Windows (WinUI 3)** and **Linux (GTK)** run on App.Core: regenerate the
  bindings (`ConnectionState` gained `Competitor` and `ProxyWasForeign`
  before `Detail`; `ConflictReport`, `Client.DetectConflicts`), and
  construct `ConnectionState` only through `ConnectionStates.Off with {…}`.
  The notice template needs nothing new: Windows picks the glyph by `Tone`
  (Error) and shows `SecondaryAction`; Linux picks the icon by `Kind`
  (Conflict falls through to `dialog-error-symbolic`; `network-vpn-symbolic`
  would fit). Optional: a `--fake-conflict` start option mapping to
  `FakeOptions.ConflictOnFirstConnect`, like `--fake-connect-fail`.
- **macOS (Swift, ports AppLogic by hand)**: regenerate the Swift bindings,
  then in `Presentation.swift`:
  1. `pathTakenOver` also holds when `connection.competitor` is set (and the
     phase is `.error`, not only `.contended`), so the enhanced headline is
     `h_pathContended` and the header image `arrow.triangle.branch`.
  2. The `failed` notice and the path-contended `occupied` notice become one
     conflict notice when `connection.competitor` is non-empty: id
     `conflict`, tone `.err`, title `tr("conflictT")`, message
     `tr("conflictD", ["app": competitor])`, action `.retry` when retryable,
     secondary `.useCompatible` when `suggestCompatible`.
  3. Without a competitor nothing changes.
  4. Compatible mode: `competitor` / `proxyWasForeign` while on get no notice.
  Update the `ConnectionState(…)` initializers in `PreviewBackend.swift` and
  `Tests/PPVPNAppLogicTests/Support.swift` (new `competitor: nil,
  proxyWasForeign: false` arguments), and add a preview scenario and a
  `PresentationTests` case for Surge (ConnectHealthCheckFailed +
  competitor "Surge"). `conflictT`, `conflictD`, `h_pathContended` are
  already in `Localizable.xcstrings`.

### Conflict gate, SystemProxyUnavailable: what each platform ports

- **Windows / Linux** (App.Core): regenerate the bindings (`ErrorCode` gained
  `SystemProxyUnavailable`, appended). Nothing else: the headline, notices and
  strings (`Error_SystemProxyUnavailable`, `sysproxyUnavailable`) come from
  App.Core. The pre-connect conflict check runs inside the crate (about 1.5 s
  at most, then the connect proceeds unchecked); on Windows detection reads
  adapters, routes and DNS through IP Helper (`GetAdaptersAddresses`,
  `GetIpForwardTable2`), no PowerShell, so it finishes in milliseconds.
- **macOS (Swift)**: regenerate the Swift bindings, then
  1. `Errors.xcstrings`: add `SystemProxyUnavailable` with the
     `sysproxyUnavailable` texts (`check-xcstrings.py` reports it missing).
  2. `Presentation.failureReason`: `case .systemProxyUnavailable:
     return tr("sysproxyUnavailable")` (before the `.systemProxyFailed` case);
     `.networkPathContended` with `connection.competitor` empty returns
     `tr("fr_timeout")`.
  3. `pathTakenOver` holds only when `connection.competitor` is non-empty
     (drop the `phase == .contended && reason.code == .networkPathContended`
     alternative). A `.contended` phase whose reason is neither ServiceBusy
     nor ServiceOwnedByAnotherUser is a failure, as in App.Core's `State()`:
     with a competitor the conflict notice, without one the ordinary failed
     notice (headline and title `h_failed`, message `fr_timeout`, retry,
     `useCompatible` when `suggestCompatible`), never "taken over" and never
     the `st_occupied` / `d_occupied` fallback it would otherwise reach.
  4. `PresentationTests`: NetworkPathContended without a competitor →
     `h_failed` / `fr_timeout`; SystemProxyUnavailable in compatible mode →
     `sysproxyUnavailable`.

## OS notifications (push agent)

The push agent posts them; the main app keeps the first-close notice
(`stillRunning`). The Windows mechanics below apply to both.

**Click contract (all platforms).** The notification's launch arguments are
just the push id: `id=<pushId>` (`PushMessage.id`). Pass nothing else, and
never a link: on a click the main app calls
`Inbox.ActivateNotification(pushId)`, which looks the push up with
`shown_push` in `<data_dir>/push-agent-shown.json` (written by the agent
after `on_push` returned true; 50 newest, 7 days) and decides from there.


**Windows** (unpackaged, self-contained). `AppNotificationManager.Register()`
does not work here: it fails with `0x8007007E` because the Windows App Runtime
Singleton package is not installed alongside a self-contained, unpackaged app.
Use classic toasts with an explicit AUMID instead:

- **AUMID.** Use `PeakPass.PPVPN` everywhere. Call
  `SetCurrentProcessExplicitAppUserModelID("PeakPass.PPVPN")` at startup, and
  create the notifier with `ToastNotificationManager.CreateToastNotifier("PeakPass.PPVPN")`.
- **Per-user AUMID registration.** Write
  `HKCU\Software\Classes\AppUserModelId\PeakPass.PPVPN` with `DisplayName`,
  `IconUri` (a PNG path) and `CustomActivator` (the CLSID of the app's COM
  activator). Register that CLSID per user under
  `HKCU\Software\Classes\CLSID\{clsid}\LocalServer32`, pointing to
  `ppvpn.exe`. The activator (`INotificationActivationCallback`) receives the
  toast `launch` arguments when the user clicks a toast after the app has
  exited. Clicks arrive through the COM activator even while the app is
  running (not `ToastNotification.Activated`), so route them to the running
  instance.
- **Start menu shortcut.** The NSIS installer runs
  `ppvpn.exe --set-shortcut-aumid` after creating the shortcut. That stamps
  `System.AppUserModel.ID = PeakPass.PPVPN` (and the activator CLSID) on the
  `.lnk`, so Windows attributes the toasts to the app and keeps them in the
  Action Center.
- **Payload.** Build `ToastGeneric` toast XML (title and body `<text>`
  elements) with `launch="id=<pushId>"`; `Critical`
  uses `scenario="urgent"` (Windows 11 22H2 and later). On activation,
  parse the push id, dispatch to the UI thread with the `DispatcherQueue`,
  and call `ActivateNotification(pushId)`. A cold start through the COM activator
  passes the same arguments, and `ActivateNotification` handles the Restoring
  state.

**Linux.** OS notifications come from the push agent (`/usr/lib/ppvpn/ppvpn-push-agent`,
started from `/etc/xdg/autostart`), not the main app. It calls
`org.freedesktop.Notifications.Notify` over D-Bus with the
`desktop-entry=com.peakpassvpn.ppvpn.desktop` hint (the desktop file is
`com.peakpassvpn.ppvpn.desktop.desktop`; so the notification carries the
app's name and icon), `urgency` 2 for Critical and 1 otherwise, and a single
`default` action. On `ActionInvoked` it runs
`/usr/lib/ppvpn/ppvpn --open-notification <pushId>`, passing the
`ActivationToken`, when there is one, in the `XDG_ACTIVATION_TOKEN` and
`DESKTOP_STARTUP_ID` environment variables (focus on Wayland / X11). The main
app handles the command line (`HandlesCommandLine`): the primary instance
receives it and calls `Inbox.ActivateNotification(pushId)`.

## Referencing it

Both apps use a `ProjectReference`. PPVPN.Client comes along transitively, and
so do its `runtimes/<rid>/native/*` libraries:

```xml
<!-- apps/windows/PPVPN.Windows/PPVPN.Windows.csproj -->
<ProjectReference Include="..\..\dotnet-shared\PPVPN.App.Core\PPVPN.App.Core.csproj" />
<!-- apps/linux/PPVPN.Linux/PPVPN.Linux.csproj -->
<ProjectReference Include="../../shared/PPVPN.App.Core/PPVPN.App.Core.csproj" />
```

Generate the bindings first (`../PPVPN.Client/README.md`).

## Tests

```sh
dotnet test apps/dotnet-shared/PPVPN.App.Core.Tests
```

`ScriptedBackend` tests push snapshots by hand for the derivations (titles,
switch states, notices, restricted states, tray, account); `FakeClientBackend`
tests run the flows (sign-in, install, compatibility mode, take-over, nodes,
inbox paging and detail navigation, logs). A `ManualTime` `TimeProvider` covers
the countdown and relative times. One smoke test runs `FfiClientBackend`
against the real native library when one is present for the machine.
