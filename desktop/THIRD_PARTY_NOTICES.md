# Third-party notices (desktop)

Third-party components the desktop apps ship or link, beyond what the engine
links. For the engine see the root
[THIRD_PARTY_NOTICES.md](../THIRD_PARTY_NOTICES.md): the Go core incorporates
sing-box; the Rust core (feature `rust-core`, off by default) links sail and,
through btls, BoringSSL, at the commits pinned in the root `Cargo.toml`.

The desktop code itself is GPL-3.0-or-later, like the rest of the repository
([LICENSE](../LICENSE)). Versions below are the ones the tree pins; where the
tree pins none, none is given.

## Images

| Component | Used by | Licence | Upstream |
|---|---|---|---|
| flag-icons (4:3 country flags) | macOS (`apps/macos/PPVPN/Resources/Assets.xcassets/Flags`); the Linux and Windows apps take the same images from there | MIT; text in `apps/macos/PPVPN/Resources/flag-icons-LICENSE.txt` | https://github.com/lipis/flag-icons |

## macOS app

| Component | Version | Licence | Upstream |
|---|---|---|---|
| Sparkle (in-app updates) | 2.x, 2.6.0 or later (`apps/macos/project.yml`) | MIT; its bundled components carry their own notices, see upstream | https://github.com/sparkle-project/Sparkle |

## Windows app

| Component | Version | Licence | Upstream |
|---|---|---|---|
| WinSparkle (in-app updates) | 0.9.2 | MIT | https://github.com/vslavik/winsparkle |
| Microsoft.WindowsAppSDK (WinUI 3) | 2.5.1 | MIT (source repository); for the terms of the NuGet package see upstream | https://github.com/microsoft/WindowsAppSDK |
| Microsoft.Windows.SDK.BuildTools (build time only) | 10.0.28000.2705 | licence: see upstream | https://www.nuget.org/packages/Microsoft.Windows.SDK.BuildTools |
| H.NotifyIcon.WinUI (tray icon) | 2.3.2 | MIT | https://github.com/HavenDV/H.NotifyIcon |
| CommunityToolkit.Mvvm | 8.4.2 | MIT | https://github.com/CommunityToolkit/dotnet |

## Linux app

| Component | Version | Licence | Upstream |
|---|---|---|---|
| GirCore.Adw-1 (GTK 4 / libadwaita bindings, with the GirCore packages it depends on) | 0.8.1 | MIT | https://github.com/gircore/gir.core |
| Tmds.DBus.Protocol (app and push agent) | 0.95.1 | MIT | https://github.com/tmds/Tmds.DBus |
| CommunityToolkit.Mvvm (through `PPVPN.App.Core`) | 8.4.2 | MIT | https://github.com/CommunityToolkit/dotnet |

GTK 4 and libadwaita themselves are not shipped: the packages depend on the
distribution's libraries.

## .NET

The Windows and Linux apps are .NET 8 programs published self-contained (the
push agents with NativeAOT), so their packages carry the .NET runtime (MIT,
https://github.com/dotnet/runtime).

Test projects only (not shipped): xunit 2.9.2 and xunit.runner.visualstudio
2.8.2 (Apache-2.0, https://github.com/xunit/xunit), Microsoft.NET.Test.Sdk
17.11.1 (MIT, https://github.com/microsoft/vstest).

## Rust

| Component | Version | Licence | Upstream |
|---|---|---|---|
| uniffi (bindings runtime in `ppvpn-client`, and the Swift generator) | 0.31.0 | MPL-2.0 | https://github.com/mozilla/uniffi-rs |
| uniffi-bindgen-cs (generates the C# bindings in `apps/shared/PPVPN.Client`; build time, its output is compiled into the apps) | v0.11.0+v0.31.0 | MPL-2.0 | https://github.com/NordSecurity/uniffi-bindgen-cs |

`ppvpn-client` and `ppvpn-service` link further crates from crates.io (tokio,
reqwest with rustls, serde, tracing, log4rs, sysinfo and others). Their exact
versions are in the root `Cargo.lock`; each crate's licence is stated in its own manifest — see upstream.

PPVPN is not affiliated with or endorsed by any of the projects above.
