<#
.SYNOPSIS
  Builds the PPVPN Windows installer (NSIS) from the Flutter app (#401).

.DESCRIPTION
  The Flutter counterpart of apps/windows/scripts/package.ps1, with the same
  installer (apps/windows/installer/ppvpn.nsi), the same install directory,
  service, shortcuts and uninstall entry, and the same artifacts:
  dist/windows/PPVPN-<version>-windows-x64-setup.exe (+ .sha256) and
  dist/windows/release-meta-windows-x64.json. An upgrade from the native app
  removes its files through the native installer's install-files.txt.

  Runs on Windows (Windows PowerShell 5.1 or PowerShell 7) after
  apps/flutter/scripts/ci-build.sh, whose release bundle it packages, with
  Rust (MSVC) and NSIS 3.08+:

    1. the version: PPVPN_VERSION and PPVPN_BUILD_NUMBER as ci-build.sh reads them (default:
       the workspace's version, build 0); the bundle's ppvpn.exe must carry it
    2. builds ppvpn-service, -install, -uninstall from crates/service, as package.ps1 does
    3. builds the push agent (apps/flutter/push-agent, release) -> ppvpn-push-agent.exe
    4. stages the bundle (ppvpn.exe, flutter_windows.dll, ppvpn_flutter.dll, the plugins' DLLs,
       data\), the agent, the service binaries, wintun.dll (pinned release, sha256-checked),
       WinSparkle.dll (the updater, rust/src/update.rs: the WinSparkle package the native app
       references), Assets\AppLogo.png (the notifications' icon, the native app's) and LICENSE.txt
    5. optional Authenticode signing (apps/windows/scripts/sign-file.ps1, the same environment)
    6. install-files.txt, makensis (solid LZMA, or zlib with -FastCompression)
    7. EdDSA signature for the update feed (winsparkle-tool, as the native app's feed is
       WinSparkle's) + release-meta-windows-x64.json

  It builds and signs only; it never uploads.

  release.yml builds the service in a job of its own, beside the app's (#459):
  -ServiceOnly builds just step 2 into -OutDir (default build\windows-service),
  and the packaging job passes that directory as -ServiceDir and the app job's
  push agent as -PushAgent, so that it compiles nothing.

  Environment:
    PPVPN_VERSION, PPVPN_BUILD_NUMBER                version and build number (see above)
    PPVPN_RELEASE_CHANNEL                            dev | stable | empty (no channel in release-meta)
    PPVPN_UPDATE_PRIVATE_KEY_FILE / PPVPN_UPDATE_PRIVATE_KEY / SPARKLE_PRIVATE_KEY
                                                     EdDSA private key (file / base64 contents)
    PPVPN_UPDATE_PUBLIC_KEY / PPVPN_SPARKLE_PUBLIC_KEY  checks the signature when set
    PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT              0/1 for the service build; defaults to 1
                                                     unless Authenticode signing is configured
                                                     (required with -ServiceOnly, which cannot tell)
    PPVPN_WINDOWS_PUBLISHER_SHA256 (+ _NEXT_)        publisher pins, required when signed
    WINDOWS_SIGN_COMMAND or WINDOWS_CERTIFICATE (+ _PASSWORD, WINDOWS_TIMESTAMP_URL)

.EXAMPLE
  powershell -ExecutionPolicy Bypass -File apps\flutter\scripts\package-windows.ps1
#>
[CmdletBinding()]
param(
  # The release bundle of ci-build.sh.
  [string] $Bundle,
  # EdDSA private key file for the update signature (else the environment, see above).
  [string] $UpdateKeyFile,
  [string] $OutDir,
  [string] $Makensis,
  [ValidateSet("", "dev", "stable")]
  [string] $Channel = "",
  # Dev loops: zlib, non-solid (much faster makensis, bigger installer).
  [switch] $FastCompression,
  # Build only the service helpers (step 2) into -OutDir, for -ServiceDir.
  [switch] $ServiceOnly,
  # The service helpers of a -ServiceOnly run, instead of building them.
  [string] $ServiceDir,
  # ppvpn-push-agent.exe built with the bundle, instead of building it.
  [string] $PushAgent
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

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$flutterDir = Join-Path $repo "apps\flutter"
$windowsDir = Join-Path $repo "apps\windows"
# The native app's installer script and assets, shared until the native app goes.
$nsi = Join-Path $windowsDir "installer\ppvpn.nsi"
$signScript = Join-Path $windowsDir "scripts\sign-file.ps1"
$target = "x86_64-pc-windows-msvc"
if (-not $Bundle) { $Bundle = Join-Path $flutterDir "build\windows\x64\runner\Release" }
if (-not $OutDir) { $OutDir = Join-Path $repo $(if ($ServiceOnly) { "build\windows-service" } else { "dist\windows" }) }
if ($ServiceOnly -and ($ServiceDir -or $PushAgent)) { throw "-ServiceOnly builds the service; it takes no -ServiceDir or -PushAgent." }
$work = Join-Path $repo "staging\windows-flutter"
$stage = Join-Path $work "app"

if (-not $Channel -and $env:PPVPN_RELEASE_CHANNEL) { $Channel = $env:PPVPN_RELEASE_CHANNEL }
if ($Channel -notin @("", "dev", "stable")) { throw "Channel must be dev or stable, not '$Channel'." }
if (-not $ServiceOnly) {
  if (-not (Test-Path -LiteralPath (Join-Path $Bundle "ppvpn.exe"))) {
    throw "no ppvpn.exe in $Bundle; build it with apps/flutter/scripts/ci-build.sh"
  }
  $Bundle = (Resolve-Path -LiteralPath $Bundle).Path
}

# --- 1. version ------------------------------------------------------------------
$version = $env:PPVPN_VERSION
if (-not $version) {
  $cargoToml = Get-Content -Raw (Join-Path $repo "Cargo.toml")
  if ($cargoToml -notmatch '(?ms)^\[workspace\.package\][^\[]*?^version = "([^"]+)"') {
    throw "no [workspace.package] version in Cargo.toml"
  }
  $version = $Matches[1]
}
$build = if ($env:PPVPN_BUILD_NUMBER) { $env:PPVPN_BUILD_NUMBER } else { "0" }
if ($version -notmatch '^\d+\.\d+\.\d+$') { throw "version must be x.y.z, not '$version'" }
if ($build -notmatch '^\d+$' -or [int]$build -gt 65535) { throw "build number must be 0-65535, not '$build'" }
$buildVersion = "$version.$build"
function Get-FileVersion([string] $Path) {
  $info = (Get-Item -LiteralPath $Path).VersionInfo
  "$($info.FileMajorPart).$($info.FileMinorPart).$($info.FileBuildPart).$($info.FilePrivatePart)"
}
# flutter build --build-name/--build-number (ci-build.sh) -> the version resource.
if (-not $ServiceOnly) {
  $exeVersion = Get-FileVersion (Join-Path $Bundle "ppvpn.exe")
  if ($exeVersion -ne $buildVersion) {
    throw "the bundle's ppvpn.exe is $exeVersion, not ${buildVersion}: build it with the same PPVPN_VERSION and PPVPN_BUILD_NUMBER"
  }
}
Write-Host "PPVPN $version (build $buildVersion), Flutter"
$installerName = "PPVPN-$version-windows-x64-setup.exe"

# --- signing configuration (as package.ps1) -----------------------------------------
$signing = [bool]($env:WINDOWS_SIGN_COMMAND -or $env:WINDOWS_CERTIFICATE)
function Invoke-Sign([string] $Path) {
  if (-not $signing) { return }
  Invoke-Native "powershell" @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", $signScript, $Path)
}

if (-not $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT) {
  # Without the signing configuration a service-only build cannot tell.
  if ($ServiceOnly) { throw "-ServiceOnly needs PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT (0 for a signed release)." }
  $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT = if ($signing) { "0" } else { "1" }
}
if ($env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT -notin @("0", "1")) {
  throw "PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT must be 0 or 1."
}
if ($env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT -eq "0") {
  if (-not $signing -and -not $ServiceOnly) {
    throw "PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT=0 needs Authenticode signing, or the service rejects ppvpn.exe."
  }
  if ($env:PPVPN_WINDOWS_PUBLISHER_SHA256 -notmatch '^[0-9a-fA-F]{64}$') {
    throw "Signed builds require PPVPN_WINDOWS_PUBLISHER_SHA256 (64 hex characters)."
  }
} else {
  Write-Warning "Unsigned build: ppvpn-service accepts the unsigned ppvpn.exe from its install directory."
}

# --- 2. service helpers (as package.ps1) --------------------------------------------
# What the service was built for, kept beside -ServiceOnly's binaries: the packaging
# run must sign as the service expects (an unsigned ppvpn.exe is refused otherwise).
$serviceBins = @("ppvpn-service", "ppvpn-service-install", "ppvpn-service-uninstall")
$serviceRecord = "allow-unsigned-client.txt"
if ($ServiceDir) {
  $serviceOut = (Resolve-Path -LiteralPath $ServiceDir).Path
  $built = (Get-Content -Raw -LiteralPath (Join-Path $serviceOut $serviceRecord)).Trim()
  if ($built -ne $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT) {
    throw "the service in $ServiceDir was built with PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT=$built, this package with $env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT"
  }
  foreach ($bin in $serviceBins) {
    $binVersion = Get-FileVersion (Join-Path $serviceOut "$bin.exe")
    if ($binVersion -ne $buildVersion) { throw "$ServiceDir\$bin.exe is $binVersion, not $buildVersion" }
  }
} else {
  Write-Host "==> cargo build ppvpn-service ($target)"
  # Version resource of ppvpn-service*.exe (crates/service/build.rs), the same as ppvpn.exe.
  $env:PPVPN_BUILD_VERSION = $buildVersion
  Push-Location $repo
  try {
    Invoke-Native "cargo" @("build", "--locked", "-p", "ppvpn-service", "--profile", "service", "--target", $target)
  } finally {
    Pop-Location
  }
  $serviceOut = Join-Path $repo "target\$target\service"
}
if ($ServiceOnly) {
  New-Item -ItemType Directory -Force -Path $OutDir | Out-Null
  foreach ($bin in $serviceBins) {
    Copy-Item -LiteralPath (Join-Path $serviceOut "$bin.exe") -Destination (Join-Path $OutDir "$bin.exe")
  }
  Write-Utf8NoBom (Join-Path $OutDir $serviceRecord) "$env:PPVPN_WINDOWS_ALLOW_UNSIGNED_CLIENT`n"
  Write-Host "service helpers: $OutDir"
  return
}

# --- 3. the push agent -------------------------------------------------------------
if ($PushAgent) {
  $agentExe = (Resolve-Path -LiteralPath $PushAgent).Path
} else {
  # The host's release build with the bridge, the command ci-build.sh runs: alone, its
  # features (the windows crate's) differ from the bridge's, and everything that shares
  # them would be built again.
  Write-Host "==> cargo build ppvpn-flutter ppvpn-push-agent (release)"
  Push-Location $repo
  try {
    Invoke-Native "cargo" @("build", "--release", "--locked", "-p", "ppvpn-flutter", "-p", "ppvpn-push-agent")
  } finally {
    Pop-Location
  }
  $agentExe = Join-Path $repo "target\release\ppvpn-push-agent.exe"
}

# --- 4. stage ------------------------------------------------------------------------
Remove-Item -Recurse -Force $work -ErrorAction SilentlyContinue
New-Item -ItemType Directory -Force -Path $stage | Out-Null
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

Write-Host "==> staging $Bundle"
Copy-Item -Path (Join-Path $Bundle "*") -Destination $stage -Recurse
# Not -Include: Windows PowerShell ignores it with -LiteralPath and lists every file.
Get-ChildItem -LiteralPath $stage -Recurse -File |
  Where-Object { $_.Extension -in @(".pdb", ".lib", ".exp", ".ilk") } | Remove-Item -Force
foreach ($required in @("ppvpn.exe", "flutter_windows.dll", "ppvpn_flutter.dll", "data\icudtl.dat", "data\app.so", "data\flutter_assets")) {
  if (-not (Test-Path -LiteralPath (Join-Path $stage $required))) { throw "the bundle lacks $required" }
}
Copy-Item -LiteralPath $agentExe -Destination (Join-Path $stage "ppvpn-push-agent.exe")
foreach ($bin in $serviceBins) {
  Copy-Item -LiteralPath (Join-Path $serviceOut "$bin.exe") -Destination (Join-Path $stage "$bin.exe")
}
# The icon the notifications show (the app's and the push agent's registration,
# rust/src/aumid.rs): the native app's, at its path.
New-Item -ItemType Directory -Force -Path (Join-Path $stage "Assets") | Out-Null
Copy-Item -LiteralPath (Join-Path $windowsDir "PPVPN.Windows\Assets\AppLogo.png") -Destination (Join-Path $stage "Assets\AppLogo.png")

# The Wintun driver DLL beside the service, as package.ps1 stages it.
$wintunVersion = "0.14.1"
$wintunSha256 = "07c256185d6ee3652e09fa55c0b673e2624b565e02c4b9091c79ca7d2f24ef51"
$wintunZip = Join-Path $work "wintun-$wintunVersion.zip"
Invoke-WebRequest "https://www.wintun.net/builds/wintun-$wintunVersion.zip" -OutFile $wintunZip
if ((Get-Sha256 $wintunZip) -ne $wintunSha256) { throw "wintun-$wintunVersion.zip: not the pinned sha256" }
$wintunDir = Join-Path $work "wintun"
Expand-Archive -LiteralPath $wintunZip -DestinationPath $wintunDir -Force
Copy-Item -LiteralPath (Join-Path $wintunDir "wintun\bin\amd64\wintun.dll") -Destination (Join-Path $stage "wintun.dll")
Write-Host "wintun.dll $wintunVersion staged"

# WinSparkle of the package the native app references: its DLL for the updater
# (rust/src/update.rs loads it from beside ppvpn.exe), its winsparkle-tool.exe for the
# feed's signature (step 7).
$csproj = Join-Path $windowsDir "PPVPN.Windows\PPVPN.Windows.csproj"
$winSparkleVersion = ([xml](Get-Content -Raw $csproj)).SelectSingleNode("//PackageReference[@Include='WinSparkle']").GetAttribute("Version")
$winSparklePackage = Join-Path $work "winsparkle.$winSparkleVersion.zip"
Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/winsparkle/$winSparkleVersion/winsparkle.$winSparkleVersion.nupkg" -OutFile $winSparklePackage
$winSparkleDir = Join-Path $work "winsparkle"
Expand-Archive -LiteralPath $winSparklePackage -DestinationPath $winSparkleDir -Force
$winSparkleDll = Join-Path $winSparkleDir "runtimes\win-x64\native\WinSparkle.dll"
if (-not (Test-Path -LiteralPath $winSparkleDll)) { throw "WinSparkle.dll (win-x64) not found in WinSparkle $winSparkleVersion" }
Copy-Item -LiteralPath $winSparkleDll -Destination (Join-Path $stage "WinSparkle.dll")
Write-Host "WinSparkle.dll $winSparkleVersion staged"

# --- 5. Authenticode (optional) --------------------------------------------------------
# Ours: the app, the bridge, the plugins built with it, the agent, the service.
$ownBinaries = @("ppvpn.exe", "ppvpn_flutter.dll", "ppvpn-push-agent.exe", "ppvpn-service.exe",
  "ppvpn-service-install.exe", "ppvpn-service-uninstall.exe") +
  @(Get-ChildItem -LiteralPath $stage -File -Filter *_plugin.dll | ForEach-Object { $_.Name })
foreach ($name in $ownBinaries) { Invoke-Sign (Join-Path $stage $name) }

# The GPL text, as package.ps1 (CRLF, for the installer's text box and Notepad).
$license = Join-Path $repo "LICENSE"
Write-Utf8NoBom (Join-Path $stage "LICENSE.txt") (((Get-Content -Raw -LiteralPath $license) -replace "`r?`n", "`r`n"))

# --- 6. install-files.txt (the next upgrade removes what it lists), makensis ------------
$stageRoot = (Resolve-Path $stage).Path.TrimEnd('\') + '\'
$files = Get-ChildItem -LiteralPath $stage -Recurse -File | ForEach-Object { $_.FullName.Substring($stageRoot.Length) }
$dirs = Get-ChildItem -LiteralPath $stage -Recurse -Directory |
  ForEach-Object { $_.FullName.Substring($stageRoot.Length) + '\' } |
  Sort-Object -Property @{ Expression = { ($_ -split '\\').Count }; Descending = $true }, @{ Expression = { $_ } }
$lines = @($files) + @("install-files.txt") + @($dirs)
Write-Utf8NoBom (Join-Path $stage "install-files.txt") (($lines -join "`r`n") + "`r`n")
Write-Host "staged $(@($files).Count) files"

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
Invoke-Native $Makensis @("/V3", "/INPUTCHARSET", "UTF8", "/DDEFINES=$definesFile", $nsi)
if (-not (Test-Path -LiteralPath $installer)) { throw "makensis did not produce $installer" }

$installerItem = Get-Item -LiteralPath $installer
$sha256 = Get-Sha256 $installer
Write-Utf8NoBom "$installer.sha256" "$sha256  $installerName`n"
Write-Host "installer: $installer"
Write-Host "sha256:    $sha256"
Write-Host "length:    $($installerItem.Length)"

# --- 7. update signature + release metadata --------------------------------------------
$keyFile = $UpdateKeyFile
if (-not $keyFile -and $env:PPVPN_UPDATE_PRIVATE_KEY_FILE) { $keyFile = $env:PPVPN_UPDATE_PRIVATE_KEY_FILE }
$keyText = "$env:PPVPN_UPDATE_PRIVATE_KEY".Trim()
if (-not $keyText) { $keyText = "$env:SPARKLE_PRIVATE_KEY".Trim() }
$temporaryKey = $null
if (-not $keyFile -and $keyText) {
  $temporaryKey = Join-Path $work "update-key.tmp"
  Write-Utf8NoBom $temporaryKey $keyText
  $keyFile = $temporaryKey
}
$edSignature = $null
try {
  if ($keyFile) {
    # winsparkle-tool.exe of the WinSparkle package staged above: the installed apps,
    # native and this one, check the feed's signatures with WinSparkle.
    $tool = Join-Path $winSparkleDir "tools\winsparkle-tool.exe"
    if (-not (Test-Path -LiteralPath $tool)) { throw "winsparkle-tool.exe not found in WinSparkle $winSparkleVersion" }
    $edSignature = (& $tool sign --private-key-file $keyFile $installer | Out-String).Trim()
    if ($LASTEXITCODE -ne 0 -or $edSignature -notmatch '^[A-Za-z0-9+/]{86}==$') {
      throw "winsparkle-tool sign failed: $edSignature"
    }
    $verifyKey = "$env:PPVPN_UPDATE_PUBLIC_KEY".Trim()
    if (-not $verifyKey) { $verifyKey = "$env:PPVPN_SPARKLE_PUBLIC_KEY".Trim() }
    if ($verifyKey) {
      & $tool verify --public-key $verifyKey --signature $edSignature $installer
      if ($LASTEXITCODE -ne 0) { throw "The EdDSA signature does not verify with the public key; wrong private key?" }
    }
    Write-Host "appcast enclosure:"
    Write-Host "  sparkle:edSignature=`"$edSignature`" length=`"$($installerItem.Length)`""
  } else {
    Write-Warning "No EdDSA private key: release metadata has no ed_signature and cannot be published to the update feed."
  }
} finally {
  if ($temporaryKey) { Remove-Item -LiteralPath $temporaryKey -Force -ErrorAction SilentlyContinue }
}

# The same shape as package.ps1's.
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
Get-Content -Raw $metaFile | Write-Host
