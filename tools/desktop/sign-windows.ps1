param(
  [Parameter(Mandatory = $true)]
  [string] $Path
)

$ErrorActionPreference = "Stop"

if (-not $env:WINDOWS_CERTIFICATE) {
  throw "WINDOWS_CERTIFICATE is required."
}
if (-not $env:WINDOWS_CERTIFICATE_PASSWORD) {
  throw "WINDOWS_CERTIFICATE_PASSWORD is required."
}
if (-not $env:WINDOWS_TIMESTAMP_URL) {
  throw "WINDOWS_TIMESTAMP_URL is required."
}
if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) {
  throw "Signing target does not exist: $Path"
}

$signTool = Get-Command "signtool.exe" -ErrorAction SilentlyContinue
if (-not $signTool) {
  $signTool = Get-ChildItem "${env:ProgramFiles(x86)}\Windows Kits\10\bin" `
    -Filter "signtool.exe" `
    -File `
    -Recurse `
    -ErrorAction SilentlyContinue |
    Where-Object { $_.DirectoryName -match '\\x64$' } |
    Sort-Object FullName -Descending |
    Select-Object -First 1
}
if (-not $signTool) {
  throw "signtool.exe was not found."
}
$signToolPath = if ($signTool -is [System.Management.Automation.CommandInfo]) {
  $signTool.Source
} else {
  $signTool.FullName
}

$temporaryDirectory = Join-Path ([System.IO.Path]::GetTempPath()) (
  "ppvpn-signing-" + [guid]::NewGuid().ToString("N")
)
$certificatePath = Join-Path $temporaryDirectory "certificate.pfx"

try {
  New-Item -ItemType Directory -Path $temporaryDirectory | Out-Null
  [System.IO.File]::WriteAllBytes(
    $certificatePath,
    [Convert]::FromBase64String($env:WINDOWS_CERTIFICATE)
  )
  & $signToolPath sign `
    /fd SHA256 `
    /tr $env:WINDOWS_TIMESTAMP_URL `
    /td SHA256 `
    /f $certificatePath `
    /p $env:WINDOWS_CERTIFICATE_PASSWORD `
    $Path
  if ($LASTEXITCODE -ne 0) {
    throw "signtool failed with exit code $LASTEXITCODE"
  }
  & $signToolPath verify /pa /all /v $Path
  if ($LASTEXITCODE -ne 0) {
    throw "signtool verification failed with exit code $LASTEXITCODE"
  }
} finally {
  if (Test-Path -LiteralPath $certificatePath) {
    [System.IO.File]::WriteAllBytes($certificatePath, [byte[]]::new(0))
  }
  Remove-Item -LiteralPath $temporaryDirectory -Recurse -Force -ErrorAction SilentlyContinue
}
