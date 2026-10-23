<#
.SYNOPSIS
  Installs, runs and uninstalls the Flutter app's Windows installer (ci.yml,
  job flutter, on the administrator runner).

.DESCRIPTION
  1. With -Native: installs the native app's installer first (silent), so
     the Flutter one runs as the upgrade from it.
  2. Installs -Installer silently and checks the install directory (the
     Flutter bundle, the push agent, the service binaries, wintun.dll; none
     of the native app's files left), the running service, the uninstall
     entry and the Start menu shortcut's AppUserModelID.
  3. Starts the installed app in the tray (--background), checks its
     notification registration, sets the installer's quit event and
     expects the app to exit by itself.
  4. Starts it again and uninstalls with /S /PURGE: the app, the service,
     the directory, the entries and the user's data are gone.
  Leaves no PPVPN installed, whatever fails.
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string] $Installer,
  [string] $Native
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3

# apps/windows/installer/ppvpn.nsi
$dir = Join-Path $env:ProgramFiles "PPVPN"
$exe = Join-Path $dir "ppvpn.exe"
$uninstallKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PPVPN"
$shortcut = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\PPVPN.lnk"
$service = "ppvpn_service"
$quitEvent = "Local\PPVPN.Desktop.Quit"
$aumid = "PeakPass.PPVPN"

function Install([string] $Path) {
  Write-Host "==> $Path /S"
  $process = Start-Process -FilePath $Path -ArgumentList "/S" -Wait -PassThru
  if ($process.ExitCode -ne 0) { throw "$Path /S exited with $($process.ExitCode)" }
}

function Wait-Until([string] $What, [int] $Seconds, [scriptblock] $Condition) {
  $deadline = (Get-Date).AddSeconds($Seconds)
  while (-not (& $Condition)) {
    if ((Get-Date) -gt $deadline) { throw "timed out after $Seconds s: $What" }
    Start-Sleep -Milliseconds 500
  }
}

function Test-QuitEvent {
  try {
    [System.Threading.EventWaitHandle]::OpenExisting($quitEvent).Dispose()
    return $true
  } catch {
    # Not there (yet).
    return $false
  }
}

function Start-App {
  Get-Process ppvpn -ErrorAction SilentlyContinue | Stop-Process -Force
  $process = Start-Process -FilePath $exe -ArgumentList "--background" -PassThru
  Wait-Until "the app's quit event" 60 { Test-QuitEvent }
  return $process
}

function Uninstall {
  $uninstaller = Join-Path $dir "uninstall.exe"
  if (-not (Test-Path -LiteralPath $uninstaller)) { return }
  Write-Host "==> $uninstaller /S /PURGE"
  # The uninstaller runs from a copy in %TEMP%: wait for what it removes, the
  # user's data (/PURGE) last.
  $process = Start-Process -FilePath $uninstaller -ArgumentList "/S", "/PURGE" -PassThru
  try {
    Wait-Until "the uninstaller" 120 {
      -not (Test-Path -LiteralPath $dir) -and -not (Test-Path $uninstallKey) -and
      -not (Test-Path -LiteralPath (Join-Path $env:LOCALAPPDATA "PPVPN"))
    }
  } catch {
    Show-UninstallState $process
    throw
  }
}

# What is left when the uninstaller times out (#425): its launcher's exit, which of the
# three conditions still holds and the files in each place, and what still runs (the
# app, the push agent, NSIS's copy of the uninstaller in %TEMP%, the service).
function Show-UninstallState($Process) {
  Write-Host "--- the uninstaller timed out; what is left:"
  $launcher = if ($Process.HasExited) { "exited with $($Process.ExitCode)" } else { "still running" }
  Write-Host "uninstall.exe: $launcher"
  Write-Host "uninstall key: $(if (Test-Path $uninstallKey) { 'present' } else { 'gone' })"
  foreach ($path in @($dir, (Join-Path $env:LOCALAPPDATA "PPVPN"))) {
    if (-not (Test-Path -LiteralPath $path)) { Write-Host "gone: $path"; continue }
    Write-Host "left: $path"
    Get-ChildItem -LiteralPath $path -Recurse -Force -ErrorAction SilentlyContinue |
      Select-Object -First 40 | ForEach-Object { Write-Host "  $($_.FullName)" }
  }
  Get-Process -ErrorAction SilentlyContinue |
    Where-Object { $_.ProcessName -like "ppvpn*" -or $_.ProcessName -like "Un_*" -or $_.ProcessName -like "Au_*" } |
    ForEach-Object {
      $path = try { $_.Path } catch { "" }
      Write-Host "running: $($_.ProcessName) (pid $($_.Id)) $path"
    }
  Get-Service $service -ErrorAction SilentlyContinue | ForEach-Object { Write-Host "service ${service}: $($_.Status)" }
}

try {
  # --- 1. the native app, to upgrade from ----------------------------------------------
  if ($Native) {
    Install $Native
    if (-not (Test-Path -LiteralPath (Join-Path $dir "WinSparkle.dll"))) { throw "the native install has no WinSparkle.dll" }
  }

  # --- 2. install --------------------------------------------------------------------
  Install $Installer
  foreach ($file in @("ppvpn.exe", "flutter_windows.dll", "ppvpn_flutter.dll", "data\icudtl.dat", "data\app.so",
      "data\flutter_assets", "Assets\AppLogo.png", "ppvpn-push-agent.exe", "ppvpn-service.exe",
      "ppvpn-service-install.exe", "ppvpn-service-uninstall.exe", "wintun.dll", "WinSparkle.dll",
      "install-files.txt", "uninstall.exe", "LICENSE.txt")) {
    if (-not (Test-Path -LiteralPath (Join-Path $dir $file))) { throw "not installed: $file" }
  }
  # The native app's own files went with its install-files.txt (WinSparkle.dll is
  # this app's too).
  foreach ($file in @("ppvpn.pri", "runtimes", "Microsoft.WindowsAppRuntime.dll")) {
    if (Test-Path -LiteralPath (Join-Path $dir $file)) { throw "left from the native app: $file" }
  }
  $listed = @(Get-Content -LiteralPath (Join-Path $dir "install-files.txt"))
  foreach ($file in Get-ChildItem -LiteralPath $dir -Recurse -File) {
    $relative = $file.FullName.Substring($dir.Length + 1)
    if ($relative -notin $listed -and $relative -ne "uninstall.exe" -and $relative -notlike "*.log") {
      throw "installed but not in install-files.txt: $relative"
    }
  }
  $state = (Get-Service $service).Status
  if ($state -ne "Running") { throw "$service is $state" }
  $imagePath = (Get-ItemProperty "HKLM:\SYSTEM\CurrentControlSet\Services\$service").ImagePath
  if ($imagePath.Trim('"') -ne (Join-Path $dir "ppvpn-service.exe")) { throw "$service runs $imagePath" }
  $entry = Get-ItemProperty $uninstallKey
  if ($entry.DisplayName -ne "PPVPN" -or -not $entry.DisplayVersion) { throw "uninstall entry: $($entry | Out-String)" }
  Write-Host "installed $($entry.DisplayVersion)"
  # ppvpn.exe --set-shortcut-aumid (rust/src/aumid.rs), run by the installer.
  $shell = New-Object -ComObject Shell.Application
  $link = $shell.Namespace((Split-Path -Parent $shortcut)).ParseName((Split-Path -Leaf $shortcut))
  if (-not $link) { throw "no Start menu shortcut at $shortcut" }
  $linkAumid = $link.ExtendedProperty("System.AppUserModel.ID")
  if ($linkAumid -ne $aumid) { throw "the shortcut's AppUserModelID is '$linkAumid', not $aumid" }

  # --- 3. the installer's quit event -----------------------------------------------------
  $app = Start-App
  $classes = "HKCU:\Software\Classes"
  $display = (Get-ItemProperty "$classes\AppUserModelId\$aumid").DisplayName
  $server = (Get-ItemProperty "$classes\CLSID\{FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE}\LocalServer32").'(default)'
  if ($display -ne "PPVPN" -or $server -ne "`"$exe`" --toast-activated") {
    throw "notification registration: '$display', '$server'"
  }
  $signal = [System.Threading.EventWaitHandle]::OpenExisting($quitEvent)
  [void]$signal.Set()
  $signal.Dispose()
  if (-not $app.WaitForExit(30000)) { throw "the app did not quit on $quitEvent" }
  Write-Host "the app quit on $quitEvent"

  # --- 4. uninstall, the app running ------------------------------------------------------
  [void](Start-App)
  Uninstall
  if (Get-Process ppvpn -ErrorAction SilentlyContinue) { throw "ppvpn.exe still runs after the uninstall" }
  if (Get-Service $service -ErrorAction SilentlyContinue) { throw "$service is still registered" }
  foreach ($path in @($shortcut, "$classes\AppUserModelId\$aumid", (Join-Path $env:LOCALAPPDATA "PPVPN"))) {
    if (Test-Path -LiteralPath $path) { throw "left after the uninstall: $path" }
  }
  Write-Host "installer smoke test passed"
} finally {
  try { Uninstall } catch { Write-Warning "cleanup: $_" }
  Get-Process ppvpn, ppvpn-push-agent -ErrorAction SilentlyContinue | Stop-Process -Force
}
