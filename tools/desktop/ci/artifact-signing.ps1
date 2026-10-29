# Authenticode through Azure Artifact Signing (#489), for release.yml's
# windows job after azure/login (OIDC):
#
#   artifact-signing.ps1 -Setup         the signing client (pinned, sha256-checked) and
#                                       its metadata; WINDOWS_SIGN_COMMAND (this script
#                                       with -Path) for apps/windows/scripts/sign-file.ps1
#   artifact-signing.ps1 -Path <file>   signs one file (SignTool + the client), timestamps
#                                       it and checks the signature
#
# Environment (-Setup): ARTIFACT_SIGNING_ENDPOINT (the account's region,
# https://<region>.codesigning.azure.net/), ARTIFACT_SIGNING_ACCOUNT,
# ARTIFACT_SIGNING_PROFILE. The client signs as the Azure CLI's login.
param(
  [switch] $Setup,
  [string] $Path
)

$ErrorActionPreference = "Stop"

# Microsoft.ArtifactSigning.Client: SignTool's plugin (Azure.CodeSigning.Dlib.dll).
$clientVersion = "1.0.146"
$clientSha256 = "7761e5d8f31e54b764da21d33cd8b0a2940e1b05526c7dfcb5e94b0431426f70"
# Artifact Signing's own timestamp authority: its certificates are valid
# three days, the timestamp keeps the signature valid after that.
$timestampUrl = "http://timestamp.acs.microsoft.com"

function Get-SignTool {
  $found = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" -Filter "signtool.exe" -File -Recurse -ErrorAction SilentlyContinue |
    Where-Object { $_.DirectoryName -match '\\10\.0\.(\d+)\.\d+\\x64$' } |
    Sort-Object { [version]($_.Directory.Parent.Name) } -Descending |
    Select-Object -First 1
  if (-not $found) { throw "signtool.exe (Windows SDK, x64) was not found." }
  # The client needs SignTool 10.0.22621.755 or later.
  if ([version]($found.Directory.Parent.Name) -lt [version]"10.0.22621.755") {
    throw "$($found.FullName) is older than the Artifact Signing client supports (10.0.22621.755)."
  }
  $found.FullName
}

if ($Setup) {
  foreach ($name in "ARTIFACT_SIGNING_ENDPOINT", "ARTIFACT_SIGNING_ACCOUNT", "ARTIFACT_SIGNING_PROFILE") {
    if (-not [Environment]::GetEnvironmentVariable($name)) { throw "$name is required." }
  }
  if ($env:ARTIFACT_SIGNING_ENDPOINT -notmatch '^https://[a-z0-9]+\.codesigning\.azure\.net/?$') {
    throw "ARTIFACT_SIGNING_ENDPOINT must be https://<region>.codesigning.azure.net/."
  }
  $dir = Join-Path $env:RUNNER_TEMP "artifact-signing"
  New-Item -ItemType Directory -Force -Path $dir | Out-Null
  $package = Join-Path $dir "client.zip"
  $ProgressPreference = "SilentlyContinue"
  Invoke-WebRequest "https://api.nuget.org/v3-flatcontainer/microsoft.artifactsigning.client/$clientVersion/microsoft.artifactsigning.client.$clientVersion.nupkg" -OutFile $package
  if ((Get-FileHash -Algorithm SHA256 -LiteralPath $package).Hash.ToLowerInvariant() -ne $clientSha256) {
    throw "Microsoft.ArtifactSigning.Client ${clientVersion}: not the pinned sha256"
  }
  Expand-Archive -LiteralPath $package -DestinationPath (Join-Path $dir "client") -Force
  $dlib = Join-Path $dir "client\bin\x64\Azure.CodeSigning.Dlib.dll"
  if (-not (Test-Path -LiteralPath $dlib)) { throw "$dlib is missing from the client package." }

  # Only the Azure CLI's login: the other credentials are not there, and
  # trying them (the managed identity's endpoint) costs time per file.
  $metadata = Join-Path $dir "metadata.json"
  [ordered]@{
    Endpoint = $env:ARTIFACT_SIGNING_ENDPOINT
    CodeSigningAccountName = $env:ARTIFACT_SIGNING_ACCOUNT
    CertificateProfileName = $env:ARTIFACT_SIGNING_PROFILE
    CorrelationId = "$env:GITHUB_REPOSITORY/$env:GITHUB_RUN_ID"
    ExcludeCredentials = @(
      "EnvironmentCredential", "WorkloadIdentityCredential", "ManagedIdentityCredential",
      "SharedTokenCacheCredential", "VisualStudioCredential", "VisualStudioCodeCredential",
      "AzurePowerShellCredential", "AzureDeveloperCliCredential", "InteractiveBrowserCredential"
    )
  } | ConvertTo-Json | Set-Content -Encoding ascii -LiteralPath $metadata

  $signTool = Get-SignTool
  Write-Host "Artifact Signing: $signTool, client $clientVersion, profile $env:ARTIFACT_SIGNING_PROFILE"
  Add-Content -Encoding utf8 -LiteralPath $env:GITHUB_ENV -Value @(
    "ARTIFACT_SIGNING_SIGNTOOL=$signTool",
    "ARTIFACT_SIGNING_DLIB=$dlib",
    "ARTIFACT_SIGNING_METADATA=$metadata",
    "WINDOWS_SIGN_COMMAND=powershell -NoProfile -ExecutionPolicy Bypass -File `"$PSCommandPath`" -Path %1"
  )
  return
}

if (-not $Path) { throw "usage: artifact-signing.ps1 -Setup | -Path <file>" }
if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { throw "Signing target does not exist: $Path" }
foreach ($name in "ARTIFACT_SIGNING_SIGNTOOL", "ARTIFACT_SIGNING_DLIB", "ARTIFACT_SIGNING_METADATA") {
  if (-not [Environment]::GetEnvironmentVariable($name)) { throw "$name is not set: run with -Setup first." }
}
& $env:ARTIFACT_SIGNING_SIGNTOOL sign /v /fd SHA256 /tr $timestampUrl /td SHA256 `
  /dlib $env:ARTIFACT_SIGNING_DLIB /dmdf $env:ARTIFACT_SIGNING_METADATA $Path
if ($LASTEXITCODE -ne 0) { throw "signtool sign failed with exit code $LASTEXITCODE for $Path" }
& $env:ARTIFACT_SIGNING_SIGNTOOL verify /pa /tw $Path
if ($LASTEXITCODE -ne 0) { throw "signtool verify failed with exit code $LASTEXITCODE for $Path" }
