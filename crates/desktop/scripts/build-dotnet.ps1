<#
.SYNOPSIS
  Build ppvpn-client for Windows and regenerate the C# bindings used by
  apps/dotnet-shared/PPVPN.Client (WinUI 3 app).

.DESCRIPTION
  Usage: scripts/build-dotnet.ps1 [-Target x86_64-pc-windows-msvc] [-Release]

  Writes:
    apps/dotnet-shared/PPVPN.Client/Generated/ppvpn_client.cs
    apps/dotnet-shared/PPVPN.Client/runtimes/win-x64/native/ppvpn_client.dll

  Requires the MSVC toolchain (Visual Studio Build Tools, "Desktop development
  with C++") and uniffi-bindgen-cs v0.11.0+v0.31.0 on PATH, or set
  $env:UNIFFI_BINDGEN_CS to the generator executable.

  Linux targets: use scripts/build-dotnet.sh.
#>
[CmdletBinding()]
param(
    [string]$Target = "x86_64-pc-windows-msvc",
    [switch]$Release
)

Set-StrictMode -Version Latest
$ErrorActionPreference = "Stop"

$BindgenCsTag = "v0.11.0+v0.31.0"
$BindgenCsVersion = $BindgenCsTag.Substring(1)
$BindgenCsInstall = "cargo install --git https://github.com/NordSecurity/uniffi-bindgen-cs --tag $BindgenCsTag uniffi-bindgen-cs"

switch ($Target) {
    "x86_64-pc-windows-msvc" { $Rid = "win-x64"; $Lib = "ppvpn_client.dll" }
    default {
        Write-Error "unsupported target triple: $Target (supported: x86_64-pc-windows-msvc; Linux targets use scripts/build-dotnet.sh)"
    }
}
$BuildProfile = if ($Release) { "release" } else { "debug" }

# Native commands do not throw on failure; check the exit code explicitly.
function Invoke-Native {
    param([Parameter(Mandatory)][string]$Exe, [string[]]$Arguments = @())
    & $Exe @Arguments
    if ($LASTEXITCODE -ne 0) {
        throw "$Exe $($Arguments -join ' ') failed with exit code $LASTEXITCODE"
    }
}

$CrateDir = (Resolve-Path (Join-Path $PSScriptRoot "..")).Path
$RepoDir = (Resolve-Path (Join-Path $CrateDir "../..")).Path
$ProjectDir = Join-Path $RepoDir "apps/dotnet-shared/PPVPN.Client"
$Bindgen = if ($env:UNIFFI_BINDGEN_CS) { $env:UNIFFI_BINDGEN_CS } else { "uniffi-bindgen-cs" }

# Check the generator before spending time on the build. The generated code
# must match uniffi 0.31 exactly, so a mismatched version is an error.
if (-not (Get-Command $Bindgen -ErrorAction SilentlyContinue)) {
    Write-Host "error: $Bindgen not found. Install it with:" -ForegroundColor Red
    Write-Host "  $BindgenCsInstall"
    exit 1
}
$FoundVersion = ((& $Bindgen --version 2>$null) -join " ").Trim() -split "\s+" | Select-Object -Last 1
if ($FoundVersion -ne $BindgenCsVersion) {
    Write-Host "error: $Bindgen is version '$FoundVersion', need $BindgenCsVersion. Reinstall with:" -ForegroundColor Red
    Write-Host "  $BindgenCsInstall --force"
    exit 1
}

Push-Location $CrateDir
try {
    if (Get-Command rustup -ErrorAction SilentlyContinue) {
        Invoke-Native rustup @("target", "add", $Target)
    }
    $CargoArgs = @("build", "--locked", "--lib", "--target", $Target)
    if ($Release) { $CargoArgs += "--release" }
    Invoke-Native cargo $CargoArgs

    $Metadata = (& cargo metadata --format-version 1 --no-deps) | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0) { throw "cargo metadata failed" }
    $TargetDir = $Metadata.target_directory
    $BuiltLib = Join-Path $TargetDir "$Target/$BuildProfile/$Lib"
    if (-not (Test-Path $BuiltLib)) {
        throw "expected build output not found: $BuiltLib"
    }

    # Bindings are generated from the compiled library so they always match it.
    $GenDir = Join-Path $TargetDir "dotnet-bindings/$Target"
    if (Test-Path $GenDir) { Remove-Item -Recurse -Force $GenDir }
    New-Item -ItemType Directory -Force -Path $GenDir | Out-Null
    Invoke-Native $Bindgen @(
        "--library", $BuiltLib, "--crate", "ppvpn_client",
        "--config", (Join-Path $CrateDir "uniffi.toml"), "--no-format", "--out-dir", $GenDir
    )
    $GeneratedCs = Join-Path $GenDir "ppvpn_client.cs"
    if (-not (Test-Path $GeneratedCs)) {
        Get-ChildItem $GenDir | Out-Host
        throw "generator did not produce ppvpn_client.cs in $GenDir"
    }

    $OutGenerated = Join-Path $ProjectDir "Generated"
    $OutNative = Join-Path $ProjectDir "runtimes/$Rid/native"
    New-Item -ItemType Directory -Force -Path $OutGenerated, $OutNative | Out-Null
    Get-ChildItem -Path $OutGenerated -Filter *.cs | Remove-Item -Force
    Get-ChildItem -Path $OutNative -File | Remove-Item -Force
    Copy-Item $GeneratedCs (Join-Path $OutGenerated "ppvpn_client.cs")
    Copy-Item $BuiltLib (Join-Path $OutNative $Lib)
    # Keep the PDB next to the DLL for native stack traces in debug builds.
    $Pdb = [System.IO.Path]::ChangeExtension($BuiltLib, ".pdb")
    if (Test-Path $Pdb) { Copy-Item $Pdb $OutNative }

    Write-Host "C# bindings: $(Join-Path $OutGenerated 'ppvpn_client.cs')"
    Write-Host "Native lib:  $(Join-Path $OutNative $Lib) ($BuildProfile, $Target)"
}
finally {
    Pop-Location
}
