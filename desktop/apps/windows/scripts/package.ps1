<#
.SYNOPSIS
  Builds the PPVPN Windows installer (NSIS) from the native WinUI app.

.DESCRIPTION
  Runs on Windows (Windows PowerShell 5.1 or PowerShell 7) with the .NET 8
  SDK, Rust (x86_64-pc-windows-msvc), MSVC, uniffi-bindgen-cs (see
  apps/shared/PPVPN.Client/README.md) and NSIS 3.08+.

    1. reads the version from apps/windows/Directory.Build.props
    2. builds ppvpn-service, -install, -uninstall from service/
    3. builds the ppvpn-client crate and its C# bindings (crates/ppvpn-client/scripts/build-dotnet.ps1 -Release),
       then dotnet publish (Release, win-x64, self-contained) -> ppvpn.exe + runtime + WinSparkle.dll
       + runtimes\win-x64\native\ppvpn_client.dll (other RIDs are pruned),
       and the push agent (NativeAOT publish of PPVPN.PushAgent) -> ppvpn-push-agent.exe next to it
    4. stages ppvpn-core.exe (vendored artifact, sha256-checked)
    5. optional Authenticode signing (scripts/sign-windows.ps1 environment)
    6. makensis -> dist/windows/PPVPN-<version>-windows-x64-setup.exe (+ .sha256); solid LZMA,
       or zlib with -FastCompression
    7. EdDSA signature for the update feed + dist/windows/release-meta-windows-x64.json

  It builds and signs only; it never uploads. Publishing to R2 is a separate job.

  Environment:
    PPVPN_API_BASE                                   backend base URL baked into the app (see README "Backend")
    PPVPN_UPDATE_FEED_URL, PPVPN_UPDATE_PUBLIC_KEY   baked into the app (see README "Updates")
    PPVPN_UPDATE_PRIVATE_KEY_FILE / PPVPN_UPDATE_PRIVATE_KEY
                                                     EdDSA private key (file / base64 contents)
    PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT              0/1 for the service build; defaults to 1
                                                     unless Authenticode signing is configured
    PPVPN_WINDOWS_PUBLISHER_SHA256 (+ _NEXT_)        publisher pins, required when signed
    WINDOWS_SIGN_COMMAND or WINDOWS_CERTIFICATE (+ _PASSWORD, WINDOWS_TIMESTAMP_URL)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File apps\windows\scripts\package.ps1
.EXAMPLE
  .\package.ps1 -CoreExe C:\drop\ppvpn-core-windows-amd64.exe -CoreSha256 7b42... -BuildNumber 12
#>
[CmdletBinding()]
param(
  # ppvpn-core.exe to ship. Default: vendor/ppvpn-core/<CURRENT>/build/ppvpn-core-windows-amd64.exe,
  # verified against that release's manifest.json.
  [string] $CoreExe,
  # Expected sha256 of -CoreExe. Without it, a windows-SHA256SUMS next to the file (or in its parent) is used.
  [string] $CoreSha256,
  # Overrides PpvpnBuildNumber (the 4th part of the file version WinSparkle compares).
  [string] $BuildNumber,
  # Backend base URL baked into the app (ClientConfig.ApiBase; else PPVPN_API_BASE, else the csproj default).
  [string] $ApiBase,
  # Override the update feed URL / public key baked into the app (else environment, else csproj defaults).
  [string] $FeedUrl,
  [string] $PublicKey,
  # EdDSA private key file for the update signature (else PPVPN_UPDATE_PRIVATE_KEY_FILE / PPVPN_UPDATE_PRIVATE_KEY).
  [string] $UpdateKeyFile,
  [string] $OutDir,
  [string] $Makensis,
  # Release channel recorded in release-meta (default: PPVPN_RELEASE_CHANNEL; omitted when empty, e.g. PR builds).
  [ValidateSet("", "dev", "stable")]
  [string] $Channel = "",
  # x.y.z overriding VersionPrefix from Directory.Build.props (release tag). No pre-release suffixes.
  [string] $Version,
  # Dev loops: zlib, non-solid (much faster makensis, bigger installer). CI keeps solid LZMA.
  [switch] $FastCompression
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3

function Invoke-Native {
  param([Parameter(Mandatory = $true)][string] $Exe, [string[]] $Arguments = @())
  # Windows PowerShell turns redirected native stderr (cargo progress) into
  # error records; only the exit code decides success here.
  $ErrorActionPreference = "Continue"
  & $Exe @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}

function Get-Sha256([string] $Path) {
  (Get-FileHash -Algorithm SHA256 -LiteralPath $Path).Hash.ToLowerInvariant()
}

function Write-Utf8NoBom([string] $Path, [string] $Text) {
  [System.IO.File]::WriteAllText($Path, $Text, [System.Text.UTF8Encoding]::new($false))
}

if (-not $Channel -and $env:PPVPN_RELEASE_CHANNEL) { $Channel = $env:PPVPN_RELEASE_CHANNEL }
if ($Channel -notin @("", "dev", "stable")) { throw "Channel must be dev or stable, not '$Channel'." }
if ($Version -and $Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must be x.y.z (no pre-release suffix), not '$Version'." }

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$windowsDir = Join-Path $repo "apps\windows"
$project = Join-Path $windowsDir "PPVPN.Windows\PPVPN.Windows.csproj"
$agentProject = Join-Path $windowsDir "PPVPN.PushAgent\PPVPN.PushAgent.csproj"
$nsi = Join-Path $windowsDir "installer\ppvpn.nsi"
$serviceDir = Join-Path $repo "service"
$target = "x86_64-pc-windows-msvc"
if (-not $OutDir) { $OutDir = Join-Path $repo "dist\windows" }
$work = Join-Path $repo "staging\windows-native"
$stage = Join-Path $work "app"

# --- 1. version --------------------------------------------------------------
$msbuildProps = @("-p:Configuration=Release", "-p:Platform=x64")
if ($BuildNumber) { $msbuildProps += "-p:PpvpnBuildNumber=$BuildNumber" }
if ($Version) { $msbuildProps += "-p:VersionPrefix=$Version" }
$json = & dotnet msbuild $project -nologo -getProperty:Version -getProperty:FileVersion @msbuildProps
if ($LASTEXITCODE -ne 0) { throw "dotnet msbuild -getProperty failed" }
$props = ($json | Out-String) | ConvertFrom-Json
$version = $props.Properties.Version
$buildVersion = $props.Properties.FileVersion
if ($buildVersion -notmatch '^\d+\.\d+\.\d+\.\d+$') { throw "FileVersion '$buildVersion' must be a four-part numeric version" }
Write-Host "PPVPN $version (build $buildVersion)"

$installerName = "PPVPN-$version-windows-x64-setup.exe"

# --- signing configuration ----------------------------------------------------
$signing = [bool]($env:WINDOWS_SIGN_COMMAND -or $env:WINDOWS_CERTIFICATE)
$signScript = Join-Path $PSScriptRoot "sign-file.ps1"
function Invoke-Sign([string] $Path) {
  if (-not $signing) { return }
  Invoke-Native "powershell" @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $signScript, $Path)
}

if (-not $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT) {
  $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT = if ($signing) { "0" } else { "1" }
}
if ($env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT -notin @("0", "1")) {
  throw "PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT must be 0 or 1."
}
if ($env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT -eq "0") {
  if (-not $signing) {
    throw "PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT=0 needs Authenticode signing, or the service rejects ppvpn.exe."
  }
  if ($env:PPVPN_WINDOWS_PUBLISHER_SHA256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw "Signed builds require PPVPN_WINDOWS_PUBLISHER_SHA256 (64 hex characters)."
  }
} else {
  Write-Warning "Unsigned build: ppvpn-service accepts the unsigned ppvpn.exe from its install directory."
}

# --- 2. service helpers -------------------------------------------------------
Write-Host "==> cargo build ppvpn-service ($target)"
# Version resource of ppvpn-service*.exe (service/build.rs), the same as ppvpn.exe.
$env:PPVPN_BUILD_VERSION = $buildVersion
Push-Location $serviceDir
try {
  Invoke-Native "cargo" @("build", "--locked", "--release", "--target", $target)
} finally {
  Pop-Location
}
$serviceOut = Join-Path $serviceDir "target\$target\release"

# --- 3. ppvpn-client + app ----------------------------------------------------
# The crate's release cdylib and matching bindings (apps/shared/PPVPN.Client/Generated and
# runtimes/win-x64/native); PPVPN.Windows references them through PPVPN.App.Core.
$clientBuild = Join-Path $repo "crates\ppvpn-client\scripts\build-dotnet.ps1"
Write-Host "==> $clientBuild -Release"
Invoke-Native "powershell" @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $clientBuild, "-Release")

Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

$publishArgs = @("publish", $project, "-nologo", "-c", "Release", "-r", "win-x64", "--self-contained", "-o", $stage) + $msbuildProps
if ($ApiBase) { $publishArgs += "-p:PPVPN_API_BASE=$ApiBase" }
if ($FeedUrl) { $publishArgs += "-p:PPVPN_UPDATE_FEED_URL=$FeedUrl" }
if ($PublicKey) { $publishArgs += "-p:PPVPN_UPDATE_PUBLIC_KEY=$PublicKey" }
Write-Host "==> dotnet publish"
Invoke-Native "dotnet" $publishArgs
Get-ChildItem -LiteralPath $stage -Recurse -Filter *.pdb | Remove-Item -Force
# PPVPN.Client copies every runtimes/<rid>/native it has (e.g. a linux-x64 build in the same
# checkout); ship only win-x64. NativeLoader loads runtimes\win-x64\native\ppvpn_client.dll.
$runtimes = Join-Path $stage "runtimes"
if (Test-Path -LiteralPath $runtimes) {
  Get-ChildItem -LiteralPath $runtimes -Directory | Where-Object { $_.Name -ne "win-x64" } | ForEach-Object {
    Write-Host "pruning runtimes\$($_.Name)"
    Remove-Item -Recurse -Force -LiteralPath $_.FullName
  }
}
# The Windows App SDK ships *.mui resources for ~90 UI languages (one folder each); the app is
# zh-CN / en-US only (SatelliteResourceLanguages covers .NET's own satellites).
$keepLanguages = @("zh-CN", "zh-Hans", "en-US", "en")
$pruned = 0
Get-ChildItem -LiteralPath $stage -Directory | Where-Object {
  $_.Name -match '^[a-z]{2,3}(-[A-Za-z]{2,4})*(-[A-Za-z0-9]{2,8})?$' -and $_.Name -notin $keepLanguages -and
  @(Get-ChildItem -LiteralPath $_.FullName -Recurse -File | Where-Object { $_.Extension -notin @(".mui", ".dll") }).Count -eq 0
} | ForEach-Object {
  Remove-Item -Recurse -Force -LiteralPath $_.FullName
  $pruned++
}
Write-Host "pruned $pruned resource-language folders (kept $($keepLanguages -join ', '))"
$clientDll = "runtimes\win-x64\native\ppvpn_client.dll"

# The push agent: NativeAOT (needs MSVC), one exe. It loads the same ppvpn_client.dll from
# runtimes\win-x64\native next to it, so only the exe is staged.
$agentOut = Join-Path $work "push-agent"
Write-Host "==> dotnet publish PPVPN.PushAgent (NativeAOT)"
Invoke-Native "dotnet" (@("publish", $agentProject, "-nologo", "-c", "Release", "-r", "win-x64", "-o", $agentOut) + $msbuildProps)
Copy-Item -LiteralPath (Join-Path $agentOut "ppvpn-push-agent.exe") -Destination (Join-Path $stage "ppvpn-push-agent.exe")
if ((Get-Sha256 (Join-Path $agentOut $clientDll)) -ne (Get-Sha256 (Join-Path $stage $clientDll))) {
  throw "the push agent was built against a different ppvpn_client.dll than the app"
}
Write-Host "ppvpn-push-agent.exe: $((Get-Item (Join-Path $stage "ppvpn-push-agent.exe")).Length) bytes"

foreach ($required in @("ppvpn.exe", "ppvpn-push-agent.exe", "WinSparkle.dll", "ppvpn.pri", $clientDll)) {
  if (-not (Test-Path (Join-Path $stage $required))) { throw "dotnet publish output lacks $required" }
}
Write-Host "ppvpn_client.dll: $((Get-Item (Join-Path $stage $clientDll)).Length) bytes"

foreach ($bin in @("ppvpn-service", "ppvpn-service-install", "ppvpn-service-uninstall")) {
  Copy-Item -LiteralPath (Join-Path $serviceOut "$bin.exe") -Destination (Join-Path $stage "$bin.exe")
}

# --- 4. core ------------------------------------------------------------------
if (-not $CoreExe) {
  $coreVersion = (Get-Content -Raw (Join-Path $repo "vendor\ppvpn-core\CURRENT")).Trim()
  $coreDir = Join-Path $repo "vendor\ppvpn-core\$coreVersion"
  $manifest = Get-Content -Raw (Join-Path $coreDir "manifest.json") | ConvertFrom-Json
  $artifact = $manifest.artifacts.'windows-x86_64'
  $CoreExe = Join-Path $coreDir $artifact.path
  if (-not $CoreSha256) { $CoreSha256 = $artifact.sha256 }
  Write-Host "ppvpn-core $coreVersion (vendored)"
}
if (-not (Test-Path -LiteralPath $CoreExe -PathType Leaf)) { throw "ppvpn-core not found: $CoreExe" }
if (-not $CoreSha256) {
  $coreItem = Get-Item -LiteralPath $CoreExe
  foreach ($sums in @((Join-Path $coreItem.DirectoryName "windows-SHA256SUMS"), (Join-Path $coreItem.Directory.Parent.FullName "windows-SHA256SUMS"))) {
    if (Test-Path -LiteralPath $sums) {
      $line = Get-Content -LiteralPath $sums | Where-Object { $_ -match ("[\\/ *]" + [regex]::Escape($coreItem.Name) + '$') } | Select-Object -First 1
      if ($line) { $CoreSha256 = ($line -split '\s+')[0]; break }
    }
  }
}
if (-not $CoreSha256) { throw "No expected sha256 for $CoreExe; pass -CoreSha256." }
$actualCoreSha = Get-Sha256 $CoreExe
if ($actualCoreSha -ne $CoreSha256.ToLowerInvariant()) {
  throw "ppvpn-core sha256 mismatch: expected $CoreSha256, got $actualCoreSha ($CoreExe)"
}
Write-Host "ppvpn-core sha256 ok: $actualCoreSha"
# The service starts exactly ppvpn-core.exe from its own directory (service/src/core.rs).
Copy-Item -LiteralPath $CoreExe -Destination (Join-Path $stage "ppvpn-core.exe")

# --- 5. Authenticode (optional) -----------------------------------------------
$ownBinaries = @("ppvpn.exe", "ppvpn-push-agent.exe", "ppvpn-core.exe", "ppvpn-service.exe", "ppvpn-service-install.exe", "ppvpn-service-uninstall.exe", $clientDll)
foreach ($name in $ownBinaries) { Invoke-Sign (Join-Path $stage $name) }

# --- install-files.txt: what this version installs (the next upgrade removes it) ---
$stageRoot = (Resolve-Path $stage).Path.TrimEnd('\') + '\'
$files = Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object { $_.FullName.Substring($stageRoot.Length) }
$dirs = Get-ChildItem -LiteralPath $stage -Recurse -Directory |
  ForEach-Object { $_.FullName.Substring($stageRoot.Length) + '\' } |
  Sort-Object -Property @{ Expression = { ($_ -split '\\').Count }; Descending = $true }, @{ Expression = { $_ } }
$lines = @($files) + @("install-files.txt") + @($dirs)
Write-Utf8NoBom (Join-Path $stage "install-files.txt") (($lines -join "`r`n") + "`r`n")

# --- 6. makensis --------------------------------------------------------------
if (-not $Makensis) {
  $cmd = Get-Command makensis.exe -ErrorAction SilentlyContinue
  if ($cmd) { $Makensis = $cmd.Source }
  else {
    $Makensis = @("${env:ProgramFiles(x86)}\NSIS\makensis.exe", "$env:ProgramFiles\NSIS\makensis.exe") |
      Where-Object { Test-Path $_ } | Select-Object -First 1
  }
}
if (-not $Makensis) { throw "makensis.exe not found (install NSIS 3.08+ or pass -Makensis)" }

$installer = Join-Path (Resolve-Path $OutDir).Path $installerName
Remove-Item -LiteralPath $installer -ErrorAction SilentlyContinue
$defines = @(
  "!define VERSION `"$version`"",
  "!define BUILD_VERSION `"$buildVersion`"",
  "!define STAGE_DIR `"$stage`"",
  "!define OUT_FILE `"$installer`""
)
if ($FastCompression) { $defines += "!define FAST_COMPRESSION" }
if ($signing) {
  # NSIS: single-quoted value, expanded inside a backtick-quoted !finalize.
  $defines += "!define SIGN_COMMAND 'powershell -NoProfile -ExecutionPolicy Bypass -File `"$signScript`" `"%1`"'"
}
$definesFile = Join-Path $work "defines.nsh"
[System.IO.File]::WriteAllText($definesFile, ($defines -join "`r`n") + "`r`n", [System.Text.UTF8Encoding]::new($true))
Write-Host "==> makensis ($(if ($FastCompression) { 'zlib' } else { 'solid lzma' }))"
$makensisTime = [Diagnostics.Stopwatch]::StartNew()
Invoke-Native $Makensis @("/V3", "/INPUTCHARSET", "UTF8", "/DDEFINES=$definesFile", $nsi)
Write-Host "makensis took $([int]$makensisTime.Elapsed.TotalSeconds) s"
if (-not (Test-Path -LiteralPath $installer)) { throw "makensis did not produce $installer" }

$installerItem = Get-Item -LiteralPath $installer
$sha256 = Get-Sha256 $installer
Write-Utf8NoBom "$installer.sha256" "$sha256  $installerName`n"
Write-Host "installer: $installer"
Write-Host "sha256:    $sha256"
Write-Host "length:    $($installerItem.Length)"

# --- 7. update signature + release metadata ------------------------------------
$keyFile = $UpdateKeyFile
if (-not $keyFile -and $env:PPVPN_UPDATE_PRIVATE_KEY_FILE) { $keyFile = $env:PPVPN_UPDATE_PRIVATE_KEY_FILE }
$temporaryKey = $null
if (-not $keyFile -and $env:PPVPN_UPDATE_PRIVATE_KEY) {
  $temporaryKey = Join-Path $work "update-key.tmp"
  Write-Utf8NoBom $temporaryKey $env:PPVPN_UPDATE_PRIVATE_KEY.Trim()
  $keyFile = $temporaryKey
}
$edSignature = $null
try {
  if ($keyFile) {
    # winsparkle-tool.exe from the WinSparkle package the app references.
    $winSparkleVersion = ([xml](Get-Content -Raw $project)).SelectSingleNode("//PackageReference[@Include='WinSparkle']").GetAttribute("Version")
    $packages = (& dotnet nuget locals global-packages --list | Out-String) -replace '(?s)^.*?global-packages:\s*', ''
    $tool = Join-Path $packages.Trim() "winsparkle\$winSparkleVersion\tools\winsparkle-tool.exe"
    if (-not (Test-Path -LiteralPath $tool)) { throw "winsparkle-tool.exe not found at $tool (restore the app first)" }
    $edSignature = (& $tool sign --private-key-file $keyFile $installer | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $edSignature -notmatch '^[A-Za-z0-9+/]{86}==$') {
      throw "winsparkle-tool sign failed: $edSignature"
    }
    $verifyKey = $PublicKey
    if (-not $verifyKey) { $verifyKey = $env:PPVPN_UPDATE_PUBLIC_KEY }
    if ($verifyKey) {
      & $tool verify --public-key $verifyKey --signature $edSignature $installer
      if ($LASTEXITCODE -ne 0) { throw "The EdDSA signature does not verify with the app's public key; wrong private key?" }
    }
    Write-Host ""
    Write-Host "appcast enclosure:"
    Write-Host "  sparkle:edSignature=`"$edSignature`" length=`"$($installerItem.Length)`""
  } else {
    Write-Warning "No EdDSA private key (PPVPN_UPDATE_PRIVATE_KEY_FILE / PPVPN_UPDATE_PRIVATE_KEY): release metadata has no ed_signature and cannot be published to the update feed."
  }
} finally {
  if ($temporaryKey) { Remove-Item -LiteralPath $temporaryKey -Force -ErrorAction SilentlyContinue }
}

$meta = [ordered]@{
  schema = 1
  platform = "windows-x64"
}
if ($Channel) { $meta.channel = $Channel }
$meta.version = $version
$meta.build = $buildVersion
$meta.file = $installerName
$meta.length = [long]$installerItem.Length
$meta.sha256 = $sha256
if ($edSignature) { $meta.ed_signature = $edSignature }
$meta.min_os = "10.0.17763"
$meta.installer_arguments = "/S"
$meta.published_at = [DateTime]::UtcNow.ToString("yyyy-MM-ddTHH:mm:ssZ", [Globalization.CultureInfo]::InvariantCulture)
$metaFile = Join-Path (Resolve-Path $OutDir).Path "release-meta-windows-x64.json"
Write-Utf8NoBom $metaFile (($meta | ConvertTo-Json) + "`n")
Write-Host "metadata:  $metaFile"
