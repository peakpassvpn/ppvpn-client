# Existing updater fullscreen fix and QA

The client still uses WinSparkle 0.9.2 and its original dialogs, downloads,
verification and installer handling. The pinned source patch adds a gate to the
existing UI thread before `Show` / `Raise` and the automatic-check permission
modal. It delays those windows while the foreground application's frame covers
its physical monitor. It does not hide an already activated window or substitute
an in-app message. Ordinary captioned maximized windows are excluded. A
missing/transitioning foreground window or unreadable monitor/frame defers the
prompt for another check. The explicit `QUNS_RUNNING_D3D_FULL_SCREEN` state
also defers it; quiet time/DND are not added as new product policy.

The public 0.9.2 API has no cancellable pre-display callback. Checking only when
starting a request, or using `check_update_without_ui`, leaves a race when another
application enters fullscreen while the request is pending. The source patch is
built by the upstream MSBuild path at an exact commit and submodule revisions;
packaging must not fall back to the unpatched NuGet DLL. The NuGet package still
supplies its existing feed-signing tool. Review the patch and source-build impact
before merge; nothing here publishes a release.

The Rust bridge loads/configures WinSparkle but waits for the initial app-side
silent/fallback decision before initialization. The patched library retains a
paused periodic worker, so later enabling fallback still works. A scheduled
result also checks that decision again at display time. Manual checks keep their
existing UI. No new update scheduler, downloader or installation service is added.

Dispatch the existing `ci.yml` on the fix branch. Its update notification QA job
builds the real DLL and a separate foreground Win32 test application, uses a
controlled delayed loopback feed, and records desktop screenshots. It checks
full screen, normal maximization, permission UI and cancellation of a deferred
scheduled result. It reports a missing/locked/noninteractive desktop as blocked,
not passed. The local HTTP fixture is never used by the product feed and installs
nothing. Signature verification code is not patched or disabled.

The target-build process observes fifteen minutes of repeated checks in a real
DLL process. This is **component UI coverage**, not a real client A→B upgrade.
The Rust completion tests separately launch actual child OS processes to read a
persisted completion acknowledgement and force failed saves. Neither replaces
full-client installation, service restart/relaunch and stale-response acceptance
listed in `docs/desktop/qa-checklist.md`. Those results must include exact source,
OS/session, installed versions/builds, raw logs, screenshots and cleanup evidence.
The shared Rust acknowledgement and serialized check-policy changes also run in the normal macOS CI job;
the Sparkle implementation remains unchanged.
