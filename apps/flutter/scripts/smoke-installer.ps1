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
     entry and the Start menu shortcut's AppUserModelID. Then runs it again
     with WinSparkle.dll held open (sharing reads only, as a VM's guest agent
     held it): it must stop before changing anything, exit code 2, the
     service running again and the installed files as they were.
  3. Starts the installed app in the tray (--background), checks its
     notification registration, sets the installer's quit event and
     expects the app to exit by itself.
  4. Starts it again and uninstalls with /S /PURGE: the app, the service,
     the directory, the entries and the user's data are gone.
  Leaves no PPVPN installed, whatever fails.

  With -Native also stage A of the acceptance (#401), as the Linux runs do
  it: the native app's signed-in state, seeded in its own formats (the
  sign-in in the Credential Manager, app-settings.json, the node, the
  connection mode, the installation id, launch at login), must survive the
  upgrade to this installer, a rollback to the native one and the upgrade
  again, each app restoring it when started. There is no backend: the API
  override (the native app) and PPVPN_API_BASE (the Flutter app, which
  ignores the override in a release build, #519) point at a closed port,
  so "restored" is the client's offline restore (the runner has a
  network, and the real API would reject the fake sign-in). One PASS/FAIL line per check, in the job summary and in
  windows-stage-a.txt under $env:RUNNER_TEMP\flutter-out; any FAIL fails
  the script after the cleanup checks.
#>
[CmdletBinding()]
param(
  [Parameter(Mandatory = $true)][string] $Installer,
  [string] $Native
)

$ErrorActionPreference = "Stop"
Set-StrictMode -Version 3

# apps/flutter/packaging/windows/ppvpn.nsi
$dir = Join-Path $env:ProgramFiles "PPVPN"
$exe = Join-Path $dir "ppvpn.exe"
$uninstallKey = "HKLM:\Software\Microsoft\Windows\CurrentVersion\Uninstall\PPVPN"
$shortcut = Join-Path $env:ProgramData "Microsoft\Windows\Start Menu\Programs\PPVPN.lnk"
$service = "ppvpn_service"
$quitEvent = "Local\PPVPN.Desktop.Quit"
$aumid = "PeakPass.PPVPN"

# Stage A: the native app's files (apps/windows AppPaths, JsonSettingsStore,
# CredentialStore, LaunchAtLogin; crates/desktop selection.json and
# client-settings.json; crates/account's credential blob).
$data = Join-Path $env:LOCALAPPDATA "PPVPN"
$logs = Join-Path $data "logs"
$credentialTarget = "PPVPN/desktop.credentials"
$blob = '{"version":1,"active_refresh":"acceptance-not-a-real-refresh-token"}'
$installation = "5f0c2a8e-3b1d-4c6e-9a7f-2d4b6e8c0a13"
$apiOverride = "http://127.0.0.1:9"
# The same closed port for the Flutter app: a release build ignores the
# saved API override (#519), so it gets the backend from PPVPN_API_BASE,
# which every app started from here inherits (the native app reads only
# the override). Without it the Flutter app called the real API, which
# rejected the fake sign-in and dropped it (run of #520).
$env:PPVPN_API_BASE = $apiOverride
$runKey = "HKCU:\Software\Microsoft\Windows\CurrentVersion\Run"
$runCommand = "`"$exe`" --background"
$out = if ($env:RUNNER_TEMP) { Join-Path $env:RUNNER_TEMP "flutter-out" } else { Join-Path $env:TEMP "flutter-out" }
$results = Join-Path $out "windows-stage-a.txt"
$script:failures = 0

# The Credential Manager's generic credentials, as CredentialStore.cs uses them.
Add-Type -TypeDefinition @"
using System;
using System.ComponentModel;
using System.Runtime.InteropServices;
using FILETIME = System.Runtime.InteropServices.ComTypes.FILETIME;

public static class PpvpnCredential {
    [StructLayout(LayoutKind.Sequential, CharSet = CharSet.Unicode)]
    struct CREDENTIAL {
        public int Flags;
        public int Type;
        public string TargetName;
        public string Comment;
        public FILETIME LastWritten;
        public int CredentialBlobSize;
        public IntPtr CredentialBlob;
        public int Persist;
        public int AttributeCount;
        public IntPtr Attributes;
        public string TargetAlias;
        public string UserName;
    }

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredWriteW(ref CREDENTIAL credential, int flags);

    [DllImport("advapi32.dll", CharSet = CharSet.Unicode, SetLastError = true)]
    static extern bool CredReadW(string target, int type, int flags, out IntPtr credential);

    [DllImport("advapi32.dll")]
    static extern void CredFree(IntPtr buffer);

    // CRED_TYPE_GENERIC, CRED_PERSIST_LOCAL_MACHINE.
    public static void Write(string target, string user, byte[] blob) {
        var credential = new CREDENTIAL { Type = 1, TargetName = target, UserName = user, Persist = 2 };
        credential.CredentialBlobSize = blob.Length;
        credential.CredentialBlob = Marshal.AllocHGlobal(blob.Length);
        try {
            Marshal.Copy(blob, 0, credential.CredentialBlob, blob.Length);
            if (!CredWriteW(ref credential, 0)) throw new Win32Exception();
        } finally {
            Marshal.FreeHGlobal(credential.CredentialBlob);
        }
    }

    // Null when there is none.
    public static byte[] Read(string target) {
        IntPtr buffer;
        if (!CredReadW(target, 1, 0, out buffer)) return null;
        try {
            var credential = (CREDENTIAL)Marshal.PtrToStructure(buffer, typeof(CREDENTIAL));
            var blob = new byte[credential.CredentialBlobSize];
            if (blob.Length > 0) Marshal.Copy(credential.CredentialBlob, blob, 0, blob.Length);
            return blob;
        } finally {
            CredFree(buffer);
        }
    }
}
"@

function Note([string] $Kind, [string] $Text) {
  $line = "$Kind $Text"
  Write-Host $line
  New-Item -ItemType Directory -Force -Path $out | Out-Null
  Add-Content -LiteralPath $results -Value $line
}

# One stage A check: PASS when $Condition is true, else FAIL (and counted).
function Check([string] $What, [scriptblock] $Condition) {
  $ok = $false
  try { $ok = [bool](& $Condition) } catch { $What = "$What ($_)" }
  if ($ok) { Note "PASS" $What } else { Note "FAIL" $What; $script:failures++ }
}

function Write-File([string] $Name, [string] $Text) {
  # UTF-8 without a BOM: the Rust side's JSON reader takes no BOM.
  [System.IO.File]::WriteAllText((Join-Path $data $Name), $Text)
}

function Read-Json([string] $Name) {
  Get-Content -Raw -LiteralPath (Join-Path $data $Name) | ConvertFrom-Json
}

function Stop-Apps {
  Get-Process ppvpn, ppvpn-push-agent -ErrorAction SilentlyContinue | Stop-Process -Force
  Start-Sleep -Seconds 1
}

# A signed-in native install, in the native app's own formats.
function Initialize-State {
  New-Item -ItemType Directory -Force -Path $data | Out-Null
  Write-File "app-settings.json" (@(
      '{'
      '  "Appearance": "Dark",'
      "  `"ApiBaseOverride`": `"$apiOverride`","
      '  "AutoCheckUpdates": false,'
      '  "BackgroundHintShown": true,'
      '  "WindowPlacements": {}'
      '}') -join "`r`n")
  Write-File "selection.json" '{"node_id":"acceptance-node-2"}'
  Write-File "client-settings.json" '{"connection_mode":"compatible"}'
  # Created only when the device registers (online): seeded as a signed-in
  # install has it.
  Write-File "installation.json" "{`"installation_id`":`"$installation`"}"
  [PpvpnCredential]::Write($credentialTarget, "PPVPN", [System.Text.Encoding]::UTF8.GetBytes($blob))
  New-Item -Force -Path $runKey | Out-Null
  Set-ItemProperty -Path $runKey -Name "PPVPN" -Value $runCommand
}

# The seeded state, unchanged.
function Test-StateKept([string] $Label) {
  Check "${Label}: the sign-in is the seeded one" {
    $read = [PpvpnCredential]::Read($credentialTarget)
    $null -ne $read -and [System.Text.Encoding]::UTF8.GetString($read) -eq $blob
  }
  Check "${Label}: the node selection is kept" { (Read-Json "selection.json").node_id -eq "acceptance-node-2" }
  Check "${Label}: the connection mode is kept (compatible)" { (Read-Json "client-settings.json").connection_mode -eq "compatible" }
  Check "${Label}: the installation id is kept" { (Read-Json "installation.json").installation_id -eq $installation }
  Check "${Label}: app-settings.json keeps Appearance Dark, the API override and the native app's keys" {
    $settings = Read-Json "app-settings.json"
    $settings.Appearance -eq "Dark" -and $settings.ApiBaseOverride -eq $apiOverride -and
      $settings.AutoCheckUpdates -eq $false -and $settings.BackgroundHintShown -eq $true -and
      ($settings.PSObject.Properties.Name -contains "WindowPlacements")
  }
  Check "${Label}: the launch-at-login entry is kept and starts $exe --background" {
    (Get-ItemProperty -Path $runKey -Name "PPVPN").PPVPN -eq $runCommand
  }
}

# One start in the tray, 25 s, then the checks every start has.
function Test-Run([string] $Label, [string] $Kind) {
  Stop-Apps
  Get-ChildItem -LiteralPath $logs -Filter "*.log" -ErrorAction SilentlyContinue | Remove-Item -Force -ErrorAction SilentlyContinue
  $process = Start-Process -FilePath $exe -ArgumentList "--background" -PassThru
  Start-Sleep -Seconds 25
  Check "${Label}: the app was still running after 25 s" { -not $process.HasExited }
  $client = (Get-ChildItem -LiteralPath $logs -Filter "ppvpn-client.*.log" -ErrorAction SilentlyContinue |
      ForEach-Object { Get-Content -Raw -LiteralPath $_.FullName }) -join "`n"
  if ($client -match "session restore: offline, signed in with the saved login") {
    Note "PASS" "${Label}: restored the saved sign-in (offline)"
  } else {
    $restore = [regex]::Match($client, "session restore: [^\r\n]*").Value
    Note "FAIL" "${Label}: did not restore the saved sign-in: $restore"
    $script:failures++
  }
  $native = @(Get-ChildItem -LiteralPath $logs -Filter "ppvpn-windows.*.log" -ErrorAction SilentlyContinue).Count
  if ($Kind -eq "native") {
    Check "${Label}: the native app ran (its ppvpn-windows log)" { $native -gt 0 }
  } else {
    Check "${Label}: the Flutter app ran (no ppvpn-windows log)" { $native -eq 0 }
  }
  $saved = Join-Path $out $Label
  New-Item -ItemType Directory -Force -Path $saved | Out-Null
  Copy-Item -Path (Join-Path $logs "*") -Destination $saved -Recurse -ErrorAction SilentlyContinue
  Stop-Apps
}

function Test-NativeLayout([string] $Label) {
  Check "${Label}: the native app is installed (ppvpn.exe, ppvpn.pri, WinSparkle.dll)" {
    (Test-Path -LiteralPath $exe) -and (Test-Path -LiteralPath (Join-Path $dir "ppvpn.pri")) -and
      (Test-Path -LiteralPath (Join-Path $dir "WinSparkle.dll"))
  }
  Check "${Label}: no Flutter files are left" {
    -not (Test-Path -LiteralPath (Join-Path $dir "flutter_windows.dll")) -and
      -not (Test-Path -LiteralPath (Join-Path $dir "data\flutter_assets"))
  }
}

# The Flutter install (throws on the first miss, as before stage A).
function Test-FlutterInstall {
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
}

function Install([string] $Path) {
  Write-Host "==> $Path /S"
  $process = Start-Process -FilePath $Path -ArgumentList "/S" -Wait -PassThru
  if ($process.ExitCode -ne 0) {
    Show-FilesInUse
    throw "$Path /S exited with $($process.ExitCode)"
  }
}

# What a silent installer that stopped on files in use (exit code 2) reported.
function Show-FilesInUse {
  $report = Join-Path $env:TEMP "ppvpn-setup-files-in-use.txt"
  if (Test-Path -LiteralPath $report) {
    Write-Host "files in use, as the installer reported them:"
    Get-Content -LiteralPath $report | ForEach-Object { Write-Host "  $_" }
    Remove-Item -LiteralPath $report
  }
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
  # Whatever else still has a file of the install loaded (a crash report, a
  # scanner), and the app's crashes in the last minutes.
  Get-Process -ErrorAction SilentlyContinue | ForEach-Object {
    $process = $_
    $held = try { @($process.Modules | Where-Object { $_.FileName -like "$dir\*" }) } catch { @() }
    if ($held.Count -gt 0) {
      Write-Host "holds files: $($process.ProcessName) (pid $($process.Id)): $(($held | ForEach-Object { Split-Path -Leaf $_.FileName }) -join ', ')"
    }
  }
  Get-WinEvent -FilterHashtable @{ LogName = "Application"; Id = 1000, 1001, 1002; StartTime = (Get-Date).AddMinutes(-10) } `
    -ErrorAction SilentlyContinue |
    Where-Object { $_.Message -like "*ppvpn*" } |
    ForEach-Object { Write-Host "event $($_.Id) at $($_.TimeCreated): $(($_.Message -split "`n")[0..3] -join ' | ')" }
}

try {
  # --- 1. the native app, to upgrade from ----------------------------------------------
  # With stage A: signed in, its state seeded, started once.
  if ($Native) {
    Install $Native
    Test-NativeLayout "1 native"
    # Not with the app running, should the installer have started it.
    Stop-Apps
    Initialize-State
    Test-Run "1-native" "native"
    Test-StateKept "1-native"
  } else {
    Write-Warning "no -Native: stage A skipped"
  }

  # --- 2. install --------------------------------------------------------------------
  Install $Installer
  Test-FlutterInstall
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
  $held = Join-Path $dir "WinSparkle.dll"
  $handle = [System.IO.File]::Open($held, "Open", "Read", "Read")
  try {
    Write-Host "==> $Installer /S, $held held open"
    $process = Start-Process -FilePath $Installer -ArgumentList "/S" -Wait -PassThru
    if ($process.ExitCode -ne 2) { throw "with $held held open, $Installer /S exited with $($process.ExitCode), not 2" }
    # Restart Manager named this script's process as the holder.
    $report = Join-Path $env:TEMP "ppvpn-setup-files-in-use.txt"
    if (-not (Select-String -LiteralPath $report -SimpleMatch "WinSparkle.dll: " -Quiet) -or
        -not (Select-String -LiteralPath $report -SimpleMatch "(pid $PID" -Quiet)) {
      Show-FilesInUse
      throw "the installer's report does not name pid $PID holding WinSparkle.dll"
    }
    Show-FilesInUse
  } finally {
    $handle.Dispose()
  }
  Wait-Until "$service running again" 30 { (Get-Service $service).Status -eq "Running" }
  Test-FlutterInstall
  Write-Host "the installer stopped on a held file and left the install as it was"

  # --- stage A: the upgrade, a rollback, the upgrade again --------------------------------
  if ($Native) {
    Test-Run "2-flutter" "flutter"
    Test-StateKept "2-flutter"
    # The rollback: the native installer over this one, as a user reinstalls it.
    Install $Native
    Test-NativeLayout "3 native"
    Test-Run "3-native" "native"
    Test-StateKept "3-native"
    Install $Installer
    Test-FlutterInstall
    Test-Run "4-flutter" "flutter"
    Test-StateKept "4-flutter"
    Note "INFO" "stage A: $($script:failures) failed"
  }

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
  if ($script:failures -gt 0) { throw "stage A: $($script:failures) checks failed (see above)" }
  Write-Host "installer smoke test passed"
} finally {
  if ((Test-Path -LiteralPath $results) -and $env:GITHUB_STEP_SUMMARY) {
    @("### Windows stage A", '```') + @(Get-Content -LiteralPath $results) + @('```') |
      Add-Content -LiteralPath $env:GITHUB_STEP_SUMMARY
  }
  try { Uninstall } catch { Write-Warning "cleanup: $_" }
  Get-Process ppvpn, ppvpn-push-agent -ErrorAction SilentlyContinue | Stop-Process -Force
}
