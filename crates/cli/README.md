# ppvpn — PeakPass VPN command-line client

`ppvpn` logs in through the browser and runs a local HTTP/SOCKS5 proxy for your PeakPass VPN account. It
does not create a TUN device or change system network settings. See `docs/cli.md` in the source repository
for every command.

## Download

Each release (tag `vX.Y.Z`) of <https://github.com/peakpassvpn/ppvpn-client/releases> carries:

| File | For |
|---|---|
| `ppvpn-cli-<version>-linux-x86_64.tar.gz` | Linux on x86_64, any distribution (static binary) |
| `ppvpn-cli-<version>-linux-aarch64.tar.gz` | Linux on ARM64, any distribution (static binary) |
| `ppvpn-cli-<version>-macos-universal.tar.gz` | macOS on Apple silicon and Intel |
| `ppvpn-cli-<version>-<platform>.symbols.tar.gz` | the binary's symbols, only to read a crash report (not needed to run it) |
| `SHA256SUMS` | the SHA-256 of every file in the release |

## Verify

Check the archive against the release's `SHA256SUMS`, in the directory you downloaded both to:

```sh
sha256sum --ignore-missing -c SHA256SUMS          # Linux
shasum -a 256 --ignore-missing -c SHA256SUMS      # macOS
```

Every file in a release also has build provenance: GitHub's attestation that the repository's release
workflow built it from the release's commit. With the GitHub CLI:

```sh
gh attestation verify ppvpn-cli-<version>-<platform>.tar.gz --repo peakpassvpn/ppvpn-client \
  --signer-workflow peakpassvpn/ppvpn-client/.github/workflows/release.yml
```

## Install

```sh
tar -xzf ppvpn-cli-<version>-<platform>.tar.gz
install -m 755 ppvpn-cli-<version>-<platform>/ppvpn ~/.local/bin/ppvpn   # or /usr/local/bin (sudo)
ppvpn version
```

On macOS the binary is signed but not notarized. An archive downloaded with `curl` runs as is; one
downloaded with a browser is quarantined, and macOS refuses to open it. After verifying it, remove the
quarantine:

```sh
xattr -d com.apple.quarantine ~/.local/bin/ppvpn
```

To uninstall, run `ppvpn stop` and `ppvpn logout`, then remove the binary.

## Requirements

The device credential is kept only in the platform secret store:

- macOS: the login Keychain;
- Linux: a Secret Service provider (GNOME Keyring, KWallet, …) with an unlocked default collection.

Without one (for example on a server without a desktop session) `ppvpn login` fails with
`CREDENTIAL_STORE_UNAVAILABLE` (exit 8); there is no plain-file fallback.

## Use

```sh
ppvpn login     # opens the browser; confirm the code shown in the terminal
ppvpn start     # starts the local proxy in the background
ppvpn proxy     # shows the proxy endpoints
ppvpn stop
```

`ppvpn --help` lists every command; `--json` makes any command print one JSON value.
