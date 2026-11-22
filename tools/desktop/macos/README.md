The macOS package retains Sparkle 2.10.1's standard updater controller, user driver,
windows, signature verification and installer. The fixed source is commit
`ab7d71291238df6e0492143b120c9b696a35f603`, also the version resolved by the existing
package build. `build-sparkle.sh` applies the checked patch and verifies the two
materialized source hashes before compiling the normal framework. Packaging then
uses its existing inside-out signing. The normal upstream `sign_update` tool is
still resolved from the same pinned package.

The small standard-driver hook delays manual result, permission, error and status
presentation before activation/show. Scheduled alerts retain the official gentle
reminder delegate. Pending retries are invalidated when the native update session
ends; a completed check also invalidates a pending checking window. The existing
installer progress app launches an opted-in PPVPN bundle without activation and
passes its update origin. The host guards its nib's initial order before Dart is
ready, and Dart resumes its existing window after fullscreen ends. Ordinary
launches keep their existing behavior. A missing patched framework disables the
updater instead of silently using an unguarded binary.

`run-update-prompt-qa.sh` builds the real framework and launches its standard UI
against a delayed loopback feed. A separate application enters actual foreground
fullscreen; the observer samples foreground ownership and the native update alert,
then exits fullscreen and requires that same native alert. It compiles the exact
presentation-policy region from the product source, without substituting a fake
driver. With a normal Flutter build passed as its argument, it also starts a copy
of that actual client with the existing updater's inactive launch configuration
and update-origin argument, samples its native first show, and requires its window
and focus after fullscreen exits. Raw samples, process identifiers, source and
binary hashes, screenshots and native exit status are retained. An unavailable
console is blocked, never passed.

These checks are component presentation and real-client startup checks. They do
not establish an actual macOS A-to-B install or a 15-minute post-install result.
The Windows normal-client installer fixture is separate. Neither fixture is a new
product updater or a production publication.
