# PPVPN for Linux: packages

The deb and rpm of the Flutter app (`apps/flutter`, #401), under the name, layout and
repository the native GTK app had, so they upgrade it in place. x86_64 only. Paths and
commands here are relative to the repository root. Packaged by PeakPass Labs LLC
<support@peakpassvpn.com>.

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

The public key is `ppvpn.asc` here (also at `https://pkg.peakpassvpn.com/linux/ppvpn.asc`).
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
| `/usr/lib/ppvpn/ppvpn` | the Flutter runner, with `lib/` and `data/` beside it; `ppvpn-service` only accepts IPC from this path |
| `/usr/lib/ppvpn/{ppvpn-service,ppvpn-service-install,ppvpn-service-uninstall}` | the service is copied to `/usr/lib/ppvpn-service` and run as the `ppvpn-service` systemd unit when Enhanced Mode is first turned on (pkexec) |
| `/usr/lib/ppvpn/package-format` | `deb` or `rpm`: which upgrade command the update notice shows |
| `/usr/lib/ppvpn/ppvpn-push-agent` | push agent (`apps/flutter/push-agent`): shows backend pushes as desktop notifications, also while the app is closed; one per user session, started from `/etc/xdg/autostart/com.peakpassvpn.ppvpn.push-agent.desktop` and by the app |
| `~/.local/share/ppvpn`, `~/.local/state/ppvpn/logs`, `~/.config/ppvpn` | per-user data, logs, settings (XDG) |
| `/var/log/ppvpn/{ppvpn-service,ppvpn-core}.log` | privileged service and Enhanced Mode engine logs (root only; 5 MB per file, 3 files kept); kept when the service is uninstalled or reinstalled |

## Package

```sh
apps/flutter/scripts/package-linux.sh <bundle>   # dist/linux/PPVPN-<version>-linux-x64.{deb,rpm} + release-meta
apps/flutter/scripts/build-repo-packages.sh      # ppvpn-archive-keyring (deb), ppvpn-release (rpm)
```

`package-linux.sh` takes the release bundle of `apps/flutter/scripts/ci-build.sh` and this
directory's `nfpm.yaml`, maintainer scripts and desktop entries; `apps/flutter/README.md`
("Linux layout") has the details.

Releases publish through `.github/workflows/release.yml` (see `docs/desktop/ci.md` and
`docs/release.md`): the deb and the signed rpm are assets of the GitHub Release `vX.Y.Z`, and
`tools/desktop/ci/build-linux-repo.sh` rebuilds the signed repositories from the releases (apt
`linux/apt`, suite `stable`; dnf `linux/rpm/stable/x86_64`, pointing at the rpm asset), the setup
packages and `linux/stable/latest.json`. `tools/desktop/ci/upload-site.sh` uploads them to the R2
bucket behind https://pkg.peakpassvpn.com: packages first, indexes, signatures and `latest.json`
last. The dev suite is no longer published; `linux/dev/latest.json` is a copy of stable's.
