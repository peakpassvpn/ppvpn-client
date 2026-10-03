# PPVPN for Linux

GTK 4 / libadwaita app (C# with Gir.Core) on the shared view models in
`apps/shared/PPVPN.App.Core` and the Rust client `crates/ppvpn-client`. x86_64 only.
Paths and commands here are relative to `desktop/`.
Packaged by PeakPass VPN LLC <support@peakpassvpn.com>.

Runs on Ubuntu 22.04+, Debian 12+ and Fedora 36+: glibc 2.35, GTK 4.6 and libadwaita
1.1 are the floor, so the UI uses only libadwaita 1.1 API (no ToolbarView, SwitchRow,
EntryRow, AlertDialog or Banner; see `UI/`).

## Install

Packages come from the PPVPN repository at `https://pkg.peakpassvpn.com/linux`, which
also delivers updates; the app only tells the user when a newer version is there.

Ubuntu / Debian:

```sh
wget -qO /tmp/ppvpn-archive-keyring.deb https://pkg.peakpassvpn.com/linux/ppvpn-archive-keyring.deb && sudo apt install /tmp/ppvpn-archive-keyring.deb && sudo apt update && sudo apt install ppvpn
```

Fedora:

```sh
sudo dnf install https://pkg.peakpassvpn.com/linux/ppvpn-release.rpm && sudo dnf install ppvpn
```

(`wget`, not `curl`: Ubuntu Desktop does not ship curl.)

`ppvpn-archive-keyring` / `ppvpn-release` install the repository's signing key and
source entry (`/etc/apt/sources.list.d/ppvpn.list`, `/etc/yum.repos.d/ppvpn.repo`). Testers
use the dev channel by replacing `stable` with `dev` in that file.

The repository, and the two setup packages (detached signatures next to them, `<url>.asc`),
are signed with this key; compare the fingerprint before trusting it:

```
PPVPN Package Signing <packages@peakpassvpn.com>
RSA 4096, expires 2031-09-28
6924 CE59 5D51 ED53 251B  8776 C48D 5132 620C 3C86
```

The public key is `packaging/ppvpn.asc` (also at `https://pkg.peakpassvpn.com/linux/ppvpn.asc`).
Without the setup package:

```sh
curl -fsSL https://pkg.peakpassvpn.com/linux/ppvpn.asc | sudo gpg --dearmor -o /usr/share/keyrings/ppvpn-archive-keyring.gpg
echo "deb [signed-by=/usr/share/keyrings/ppvpn-archive-keyring.gpg] https://pkg.peakpassvpn.com/linux/apt stable main" | sudo tee /etc/apt/sources.list.d/ppvpn.list
```

Removing `ppvpn` also removes the Enhanced Mode system service it installed. To drop the
repository as well, purge the setup package (`sudo apt purge ppvpn-archive-keyring`): a plain
`remove` keeps its source entry, as for every Debian configuration file.

## Layout on disk

| Path | |
|---|---|
| `/usr/lib/ppvpn/ppvpn` | the app (self-contained .NET apphost); `ppvpn-service` only accepts IPC from this path |
| `/usr/lib/ppvpn/{ppvpn-core,ppvpn-service,ppvpn-service-install,ppvpn-service-uninstall}` | copied to `/usr/lib/ppvpn-service` and run as the `ppvpn-service` systemd unit when Enhanced Mode is first turned on (pkexec) |
| `/usr/lib/ppvpn/package-format` | `deb` or `rpm`: which upgrade command the update notice shows |
| `/usr/lib/ppvpn/ppvpn-push-agent` | push agent (NativeAOT, `PPVPN.PushAgent/`): shows backend pushes as desktop notifications, also while the app is closed; one per user session, started from `/etc/xdg/autostart/com.peakpassvpn.ppvpn.push-agent.desktop` and by the app |
| `~/.local/share/ppvpn`, `~/.local/state/ppvpn/logs`, `~/.config/ppvpn` | per-user data, logs, settings (XDG) |
| `/var/log/ppvpn/{ppvpn-service,ppvpn-core}.log` | privileged service and Enhanced Mode core logs (root only; 5 MB per file, 3 files kept); kept when the service is uninstalled or reinstalled |

Credentials go to the Secret Service (GNOME Keyring, KWallet, …); only without one, to a
0600 file in the data directory. A saved login in a locked keyring whose unlock prompt was
dismissed is reported as `PlatformException.Locked` (`LINUX_KEYRING_LOCKED`), not as "signed
out": the login page says to unlock it, and the client signs in again once it is unlocked.

The push agent talks to `org.freedesktop.Notifications`. Without a notification server it
keeps pushes pending and shows them once one appears. A click runs
`ppvpn --open-notification <push id>`; the running app receives it through its command line.
Its log is `~/.local/state/ppvpn/logs/ppvpn-push-agent.<date>.log`.

## Develop

```sh
crates/ppvpn-client/scripts/build-dotnet.sh x86_64-unknown-linux-gnu   # bindings + native library
dotnet run --project apps/linux -- --preview                            # UI on FakeClientBackend
dotnet run --project apps/linux -- --preview=NoSubscription             # a FakeProfileScenario
dotnet test apps/linux/PPVPN.Linux.Tests
```

`--preview` exists in debug builds only. Builds use the production backend unless
`PPVPN_API_BASE` is set when building (`PPVPN_API_BASE=<dev backend> dotnet build`). Debug
builds are accepted by a debug `ppvpn-service` from any `apps/linux/…/ppvpn` path.

## Package

```sh
apps/linux/scripts/build-package.sh        # dist/linux/PPVPN-<version>-linux-x64.{deb,rpm} + release-meta
apps/linux/scripts/build-repo-packages.sh  # ppvpn-archive-keyring (deb), ppvpn-release (rpm)
apps/linux/scripts/smoke-test.sh deb|rpm   # start an installed package once under Xvfb
```

Releases publish through `.github/workflows/desktop-native-release.yml`: after the installers
go to R2, `scripts/ci/publish-linux-repo.sh` adds the deb and rpm to the signed repositories in
the packages bucket (apt `linux/apt`, suites `dev` and `stable`; dnf `linux/rpm/<channel>/x86_64`),
publishes the setup packages and writes `linux/<channel>/latest.json`. Packages are immutable;
indexes, signatures and `latest.json` move last. `PKG_STORE=dir:<path>` publishes into a local
directory instead (with a throwaway key) to try it out.

`build-package.sh` documents its inputs (version, build number, channel, API base, update
feed). The Rust code is linked against glibc 2.35 with cargo-zigbuild; the NativeAOT push
agent links against the build host's glibc (and needs clang and zlib1g-dev), so build on
Ubuntu 22.04 — the script refuses binaries that need a newer glibc. CI: `.github/workflows/desktop-native-linux.yml`, called by
the unified desktop release, which publishes the packages to the repository.
