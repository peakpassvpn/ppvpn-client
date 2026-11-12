# Diagnostic only: reuse a normal, already-built A installer on a disposable CI host.
$ErrorActionPreference='Stop'
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class PpvpnStartupExitCode{[DllImport("kernel32.dll",SetLastError=true)]public static extern bool GetExitCodeProcess(IntPtr process,out uint code);}'
function Get-OwnedExitCode([IntPtr]$Handle){
  [uint32]$code=259
  if($Handle -eq [IntPtr]::Zero -or -not [PpvpnStartupExitCode]::GetExitCodeProcess($Handle,[ref]$code) -or $code -eq 259){throw 'Owned process status unknown/still running'}
  return $code
}
if(-not $env:GITHUB_ACTIONS){throw 'Startup installer diagnostic is CI-only'}
$dir=Join-Path $env:ProgramFiles 'PPVPN'
if((Get-Service ppvpn_service -ErrorAction SilentlyContinue) -or (Get-Process ppvpn -ErrorAction SilentlyContinue) -or (Test-Path $dir)){throw 'Refusing an existing user app/service'}
$repo=(Resolve-Path "$PSScriptRoot/../../..").Path
$out=Join-Path $repo 'build/startup-focus-qa'
New-Item -ItemType Directory -Force $out|Out-Null
$installer=Join-Path $repo 'build/reused/update-installers/A/PPVPN-0.7.5-windows-x64-setup.exe'
if(-not (Test-Path $installer)){throw 'Original A installer missing'}
@{observer_source=(git rev-parse HEAD);artifact_run=$env:PPVPN_SOURCE_RUN;installer_sha256=(Get-FileHash $installer).Hash;scope='startup diagnostic only; no A-to-B/15-minute acceptance'}|ConvertTo-Json|Set-Content "$out/source.json"
try {
  $install=Start-Process $installer -ArgumentList '/S' -PassThru
  $installHandle=$install.Handle
  if(-not $install.WaitForExit(180000) -or (Get-OwnedExitCode $installHandle) -ne 0){throw 'Original A installation failed'}
  $data=Join-Path $env:LOCALAPPDATA 'PPVPN'
  New-Item -ItemType Directory -Force $data|Out-Null
  [IO.File]::WriteAllText((Join-Path $data 'silent-update.json'),'{"auto_update":false}')
  [IO.File]::WriteAllText((Join-Path $data 'app-settings.json'),'{"Language":"en","AutoCheckUpdates":false}')
  Push-Location $out
  try {
    $helper=Join-Path $repo 'build/reused/update-qa/update-prompt-qa.exe'
    $observer=Start-Process "$repo/build/startup-focus-qa/client-upgrade-qa.exe" -ArgumentList ('"'+$dir+'/ppvpn.exe" "'+$helper+'" 0.7.5.60000 0.7.5.60001 --startup-only') -PassThru -RedirectStandardOutput 'raw.txt' -RedirectStandardError 'errors.txt'
    $observerHandle=$observer.Handle
    $deadline=[DateTime]::UtcNow.AddSeconds(50)
    while(-not $observer.HasExited -and [DateTime]::UtcNow -lt $deadline){
      Get-CimInstance Win32_Process -Filter "Name='ppvpn.exe'"|Where-Object {$_.ExecutablePath -eq "$dir\ppvpn.exe"}|Select-Object ProcessId,ParentProcessId,CommandLine,CreationDate|ConvertTo-Json -Compress|Add-Content actual-argv.jsonl
      Start-Sleep -Milliseconds 100
      $observer.Refresh()
    }
    if(-not $observer.HasExited){Stop-Process -Id $observer.Id -Force;throw 'Diagnostic observer timeout; not acceptance'}
    Get-Content raw.txt
    if(Test-Path errors.txt){Get-Content errors.txt}
    if((Get-OwnedExitCode $observerHandle) -ne 0){throw 'Actual startup diagnostic failed; inspect foreground and startup phase evidence'}
  }finally{Pop-Location}
}finally{
  if(Test-Path "$dir/uninstall.exe"){
    $uninstall=Start-Process "$dir/uninstall.exe" -ArgumentList '/S /PURGE' -PassThru
    if(-not $uninstall.WaitForExit(180000)){throw 'Owned normal uninstaller timeout'}
    # NSIS copies itself to a temporary child; parent exit is not cleanup completion.
    $deadline=[DateTime]::UtcNow.AddSeconds(60)
    do {
      $remaining=@(Get-Process ppvpn -ErrorAction SilentlyContinue)
      $service=Get-Service ppvpn_service -ErrorAction SilentlyContinue
      if(-not $remaining.Count -and -not $service){break}
      Start-Sleep -Milliseconds 250
    }while([DateTime]::UtcNow -lt $deadline)
    @{app_processes=@($remaining|Select-Object Id,Path);service_remaining=[bool]$service;directory_remaining=(Test-Path $dir)}|ConvertTo-Json -Depth 4|Set-Content "$out/cleanup.json"
    if($remaining.Count -or $service){throw 'Owned diagnostic app/service remained after normal uninstallation'}
  }
}
