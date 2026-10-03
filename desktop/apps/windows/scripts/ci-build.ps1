<#
.SYNOPSIS
  Release build of the native Windows app, as run by the package-windows job of
  .github/workflows/desktop-package.yml. Also runs locally.

.DESCRIPTION
  1. verifies the vendored ppvpn-core against its manifest (scripts/verify-vendored-core.mjs)
  2. runs package.ps1 (which also builds the ppvpn-client crate and bindings with -Release)
     with the API base, the feed URL derived from it, the channel, version, build number
     and update keys

  Builds only; publishing is the release workflow's job.

  Parameters default to the environment the workflow sets:
    PPVPN_RELEASE_CHANNEL   dev | stable | empty (PR builds: no channel in release-meta)
    PPVPN_API_BASE          backend base URL (required): baked into the app as its backend
                            (PPVPN_API_BASE build property) and the update feed
                            <base>/api/v1/desktop/releases/windows-x64/appcast.xml
    PPVPN_BUILD_NUMBER      fourth part of FileVersion (sparkle:version), 0-65535
    PPVPN_VERSION           x.y.z overriding VersionPrefix
    PPVPN_SPARKLE_PUBLIC_KEY  base64 ed25519 public key baked into the app (shared with macOS)
    SPARKLE_PRIVATE_KEY     base64 private key; written to a temporary file, never printed
  Optional Authenticode: WINDOWS_CERTIFICATE (+ _PASSWORD, WINDOWS_TIMESTAMP_URL) or
  WINDOWS_SIGN_COMMAND, with PPVPN_WINDOWS_PUBLISHER_SHA256 (see package.ps1).

.EXAMPLE
  $env:PPVPN_SPARKLE_PUBLIC_KEY = "..."; $env:SPARKLE_PRIVATE_KEY = Get-Content key.txt
  .\ci-build.ps1 -Channel dev -ApiBase https://api.example.com -BuildNumber 42
#>
[CmdletBinding()]
param(
  [string] $Channel = $env:PPVPN_RELEASE_CHANNEL,
  [string] $ApiBase = $env:PPVPN_API_BASE,
  [string] $BuildNumber = $env:PPVPN_BUILD_NUMBER,
  [string] $Version = $env:PPVPN_VERSION,
  # Passed through to package.ps1 (default: the vendored core).
  [string] $CoreExe,
  [string] $CoreSha256
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3

function Invoke-Native {
  param([Parameter(Mandatory = $true)][string] $Exe, [string[]] $Arguments = @())
  $ErrorActionPreference = "Continue"
  & $Exe @Arguments
  if ($LASTEXITCODE -ne 0) { throw "$Exe failed with exit code $LASTEXITCODE" }
}

$repo = (Resolve-Path (Join-Path $PSScriptRoot "..\..\..")).Path
$inCi = [bool]$env:GITHUB_ACTIONS

if (-not $Channel) { $Channel = "" }
if ($Channel -notin @("", "dev", "stable")) { throw "Channel must be dev or stable, not '$Channel'." }
if (-not $ApiBase) { throw "ApiBase (or PPVPN_API_BASE) is required." }
$ApiBase = $ApiBase.Trim().TrimEnd("/")
if ($ApiBase -notmatch '^https?://[^/\s]+(/\S*)?$') { throw "ApiBase must be an http(s) URL, not '$ApiBase'." }
if ($inCi -and $ApiBase -notmatch '^https://') { throw "ApiBase must use https in CI." }
$feedUrl = "$ApiBase/api/v1/desktop/releases/windows-x64/appcast.xml"
if ($BuildNumber -and ($BuildNumber -notmatch '^\d+$' -or [int]$BuildNumber -gt 65535)) {
  throw "BuildNumber must be an integer between 0 and 65535."
}
if ($Version -and $Version -notmatch '^\d+\.\d+\.\d+$') { throw "Version must be x.y.z, not '$Version'." }
# The backend may be a secret of the release environment: say which kind, not its value.
$apiKind = if ($ApiBase -eq "https://www.peakpassvpn.com") { "production" } else { "custom" }
Write-Host "channel '$Channel', api $apiKind, version $(if ($Version) { $Version } else { '(Directory.Build.props)' }), build number $(if ($BuildNumber) { $BuildNumber } else { '(Directory.Build.props)' })"

# --- 1. vendored core ---------------------------------------------------------
if (-not $CoreExe) {
  $coreVersion = (Get-Content -Raw (Join-Path $repo "vendor\ppvpn-core\CURRENT")).Trim()
  $node = Get-Command node -ErrorAction SilentlyContinue
  if ($node) {
    Invoke-Native $node.Source @(
      (Join-Path $repo "scripts\verify-vendored-core.mjs"),
      "--vendor-dir", (Join-Path $repo "vendor\ppvpn-core\$coreVersion"),
      "--artifact", "windows-x86_64",
      "--expected-version", $coreVersion
    )
  } elseif ($inCi) {
    throw "node is required to verify the vendored core."
  } else {
    Write-Warning "node not found: skipping verify-vendored-core.mjs (package.ps1 still checks the sha256 from manifest.json)."
  }
}

# --- 2. update keys -----------------------------------------------------------
$publicKey = "$env:PPVPN_SPARKLE_PUBLIC_KEY".Trim()
$privateKey = "$env:SPARKLE_PRIVATE_KEY".Trim()
if (-not $publicKey -or -not $privateKey) {
  $message = "PPVPN_SPARKLE_PUBLIC_KEY and SPARKLE_PRIVATE_KEY are required for a publishable build."
  if ($Channel) { throw $message }
  Write-Warning "$message Building without them: updates are disabled in the app and release-meta has no ed_signature."
}

$keyFile = $null
try {
  $packageArgs = @("-NoProfile", "-ExecutionPolicy", "Bypass", "-File", (Join-Path $PSScriptRoot "package.ps1"),
    "-ApiBase", $ApiBase, "-FeedUrl", $feedUrl)
  if ($Channel) { $packageArgs += @("-Channel", $Channel) }
  if ($Version) { $packageArgs += @("-Version", $Version) }
  if ($publicKey) { $packageArgs += @("-PublicKey", $publicKey) }
  if ($privateKey) {
    $keyFile = Join-Path ([System.IO.Path]::GetTempPath()) ("ppvpn-update-" + [guid]::NewGuid().ToString("N") + ".key")
    [System.IO.File]::WriteAllText($keyFile, $privateKey, [System.Text.UTF8Encoding]::new($false))
    $packageArgs += @("-UpdateKeyFile", $keyFile)
  }
  if ($BuildNumber) { $packageArgs += @("-BuildNumber", $BuildNumber) }
  if ($CoreExe) { $packageArgs += @("-CoreExe", $CoreExe) }
  if ($CoreSha256) { $packageArgs += @("-CoreSha256", $CoreSha256) }
  Invoke-Native "powershell" $packageArgs
} finally {
  if ($keyFile -and (Test-Path -LiteralPath $keyFile)) {
    [System.IO.File]::WriteAllBytes($keyFile, [byte[]]::new(0))
    Remove-Item -LiteralPath $keyFile -Force
  }
}

$meta = Join-Path $repo "dist\windows\release-meta-windows-x64.json"
Get-Content -Raw $meta | Write-Host
