# Build the exact upstream version with the display-time fullscreen gate. Never
# fall back to the unpatched NuGet DLL: that would restore the focus race.
param([Parameter(Mandatory=$true)][string]$Work,
      [Parameter(Mandatory=$true)][string]$Output)
$ErrorActionPreference = 'Stop'
$source = Join-Path $Work 'winsparkle-source'
$revision = '9f43e81c1dfcdacd4e8f24a2503d332870ae6ab8' # upstream v0.9.2
if (-not (Test-Path (Join-Path $source '.git'))) {
  git clone --no-checkout https://github.com/vslavik/winsparkle.git $source
  if ($LASTEXITCODE) { throw 'WinSparkle clone failed' }
}
# A separate work directory belongs to this build; do not reuse an arbitrary checkout.
git -C $source checkout --detach $revision
if ($LASTEXITCODE) { throw 'WinSparkle pinned checkout failed' }
if ((git -C $source rev-parse HEAD).Trim() -ne $revision) { throw 'WinSparkle revision mismatch' }
git -C $source submodule update --init --recursive --depth 1
if ($LASTEXITCODE) { throw 'WinSparkle pinned submodules failed' }
$patch = Join-Path $PSScriptRoot 'winsparkle-0.9.2-fullscreen.patch'
$patchMark = Join-Path $source '.ppvpn-fullscreen-patch.sha256'
$patchHash = (Get-FileHash $patch -Algorithm SHA256).Hash
if (Test-Path $patchMark) {
  if ((Get-Content $patchMark -Raw).Trim() -ne $patchHash) { throw 'Use a fresh build work directory for a changed WinSparkle patch' }
} else {
  git -C $source apply --unidiff-zero --check $patch
  if ($LASTEXITCODE) { throw 'WinSparkle patch does not apply cleanly' }
  git -C $source apply --unidiff-zero $patch
  if ($LASTEXITCODE) { throw 'WinSparkle patch failed' }
  Set-Content $patchMark $patchHash
}
Copy-Item (Join-Path $PSScriptRoot 'update_fullscreen.h') (Join-Path $source 'src/ppvpn_update_fullscreen.h')
Push-Location $source
try {
  nuget restore WinSparkle.sln -NonInteractive
  if ($LASTEXITCODE) { throw 'WinSparkle NuGet restore failed' }
  # Same supported MSBuild path as upstream. Dwmapi supplies physical frame bounds.
  msbuild WinSparkle.vcxproj /m /t:Build /p:Configuration=Release /p:Platform=x64 /p:PlatformToolset=v143
  if ($LASTEXITCODE) { throw 'Patched WinSparkle build failed' }
  $dll = Join-Path $source 'x64/Release/WinSparkle.dll'
  if (-not (Test-Path $dll)) { throw 'Patched WinSparkle output missing' }
  New-Item -ItemType Directory -Force (Split-Path $Output) | Out-Null
  Copy-Item $dll $Output
  Write-Host "WinSparkle $revision + fullscreen gate: $((Get-FileHash $Output -Algorithm SHA256).Hash)"
} finally { Pop-Location }
