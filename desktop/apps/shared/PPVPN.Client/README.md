# PPVPN.Client

.NET 8 bindings for the shared Rust crate `crates/ppvpn-client`, used by the
Windows (WinUI 3) and Linux (Gir.Core) apps via a `ProjectReference`.

`Generated/` and `runtimes/` are build outputs and are not committed. Regenerate
them after pulling changes to the crate:

```sh
# Linux (x64 / arm64)
crates/ppvpn-client/scripts/build-dotnet.sh x86_64-unknown-linux-gnu [--release]
crates/ppvpn-client/scripts/build-dotnet.sh aarch64-unknown-linux-gnu [--release]
```

```powershell
# Windows x64 (MSVC toolchain)
crates\ppvpn-client\scripts\build-dotnet.ps1 [-Release]
```

Each run builds the cdylib for that target, generates C# from the built library
(UniFFI library mode; `crates/ppvpn-client/uniffi.toml` sets public types and
the C# namespace) into `Generated/ppvpn_client.cs`, and copies the
native library to `runtimes/<rid>/native/` (`libppvpn_client.so` /
`ppvpn_client.dll`). Bindings and library must come from the same build: the
bindings verify API checksums against the library when first used.

The generator is pinned; the scripts refuse to run without it:

```sh
cargo install --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag v0.11.0+v0.31.0 uniffi-bindgen-cs
```

## Native library loading

The bindings import `"ppvpn_client"`. With a `ProjectReference`, .NET does not
probe `runtimes/<rid>/native/`, so the project copies that folder into the
consuming app's output/publish directory and `NativeLoader` registers a
`DllImport` resolver that loads `runtimes/<rid>/native/<lib>` for the running
OS/architecture, falling back to default probing. The resolver installs itself
when the assembly loads; calling `PPVPN.Ffi.NativeLoader.Install()` once at
startup is harmless and makes the dependency explicit. `NativeLoader.LoadedFrom`
reports which file was loaded.

A referencing app's build and `dotnet publish` (framework-dependent or
`--self-contained`, any `-r <rid>`) copy every `runtimes/<rid>/native/*` present
in this project to the app output. Normally that is only the RID built on the
machine; delete stale folders before packaging if you built several.

## Notes

- Linux: the `.so` requires the glibc version of the machine that built it
  (a build on Debian 13 / Ubuntu 24.04 needs GLIBC_2.39 and fails to load on
  Debian 12). Build release libraries on the oldest distro you support, or use
  `PPVPN_CARGO_BUILD="cargo zigbuild"` with a glibc-suffixed target.

- When copying a checkout from macOS to another machine as a tarball, create it
  with `COPYFILE_DISABLE=1 tar ...`; otherwise AppleDouble `._*` files end up
  next to the project and `dotnet` fails with "multiple project files".
