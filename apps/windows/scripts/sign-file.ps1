# Optional Authenticode signing for one file, driven by the same environment
# as the release workflow:
#   WINDOWS_SIGN_COMMAND  command line with %1 for the file (e.g. a cloud HSM tool), or
#   WINDOWS_CERTIFICATE (+ WINDOWS_CERTIFICATE_PASSWORD, WINDOWS_TIMESTAMP_URL)
#                         → scripts/sign-windows.ps1 (PFX + signtool)
# With neither set this is a no-op, so unsigned builds work unchanged.
# package.ps1 calls it for the staged binaries and hands it to makensis
# (!finalize / !uninstfinalize) for the installer and the uninstaller.
param(
  [Parameter(Mandatory = $true)]
  [string] $Path
)

$ErrorActionPreference = "Stop"

if ($env:WINDOWS_SIGN_COMMAND) {
  if (-not $env:WINDOWS_SIGN_COMMAND.Contains("%1")) {
    throw "WINDOWS_SIGN_COMMAND must contain the %1 file placeholder."
  }
  $command = $env:WINDOWS_SIGN_COMMAND.Replace("%1", '"' + $Path + '"')
  & cmd.exe /d /s /c $command
  if ($LASTEXITCODE -ne 0) { throw "WINDOWS_SIGN_COMMAND failed with exit code $LASTEXITCODE for $Path" }
} elseif ($env:WINDOWS_CERTIFICATE) {
  $repo = Resolve-Path (Join-Path $PSScriptRoot "..\..")
  & powershell -NoProfile -ExecutionPolicy Bypass -File (Join-Path $repo "tools\desktop\sign-windows.ps1") $Path
  if ($LASTEXITCODE -ne 0) { throw "sign-windows.ps1 failed with exit code $LASTEXITCODE for $Path" }
} else {
  Write-Host "not signing $Path (no WINDOWS_SIGN_COMMAND or WINDOWS_CERTIFICATE)"
}
