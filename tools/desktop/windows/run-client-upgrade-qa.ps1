# Existing updater + existing installer. Run only on a clean disposable CI host.
$ErrorActionPreference='Stop'
if(-not $env:GITHUB_ACTIONS){throw 'This destructive installer fixture is CI-only; never run on a user VM'}
if((Get-Service ppvpn_service -ErrorAction SilentlyContinue) -or (Get-Process ppvpn -ErrorAction SilentlyContinue) -or (Test-Path "$env:ProgramFiles/PPVPN")){throw 'Refusing to replace an existing user app/service'}
Add-Type -TypeDefinition 'using System;using System.Runtime.InteropServices;public static class PpvpnUpgradeExitCode{[DllImport("kernel32.dll",SetLastError=true)]public static extern bool GetExitCodeProcess(IntPtr process,out uint code);}'
function Get-OwnedExitCode([IntPtr]$Handle){
  [uint32]$code=259
  if($Handle -eq [IntPtr]::Zero -or -not [PpvpnUpgradeExitCode]::GetExitCodeProcess($Handle,[ref]$code) -or $code -eq 259){throw 'Owned process exit status unknown/still active; not acceptance'}
  return $code
}
$repo=(Resolve-Path "$PSScriptRoot/../../..").Path
$out=Join-Path $repo 'build/client-upgrade-qa'
New-Item -ItemType Directory -Force $out | Out-Null
$dir=Join-Path $env:ProgramFiles 'PPVPN'
$data=Join-Path $env:LOCALAPPDATA 'PPVPN'
$server=$null;$observer=$null;$observerStarted=$null
try {
  $a=Join-Path $repo 'build/update-installers/A/PPVPN-0.7.5-windows-x64-setup.exe'
  $source=(git -C $repo rev-parse HEAD).Trim()
  if($source -cne $env:GITHUB_SHA){throw 'Checkout HEAD differs from dispatched source'}
  $manifest=@{source=$source;dispatch_sha=$env:GITHUB_SHA;hashes=@{}}
  $files=@{observer='build/update-qa/client-upgrade-qa.exe';helper='build/update-qa/update-prompt-qa.exe';A_exe='build/update-bundles/A/ppvpn.exe';B_exe='build/update-bundles/B/ppvpn.exe';A_bridge='build/update-bundles/A/ppvpn_flutter.dll';B_bridge='build/update-bundles/B/ppvpn_flutter.dll'}
  foreach($entry in $files.GetEnumerator()){$manifest.hashes[$entry.Key]=(Get-FileHash (Join-Path $repo $entry.Value) -Algorithm SHA256).Hash}
  $manifest|ConvertTo-Json -Depth 8|Set-Content "$out/artifact-manifest.json"
  $install=Start-Process $a -ArgumentList '/S' -PassThru
  $installHandle=$install.Handle
  if(-not $install.WaitForExit(180000)){throw 'Normal A installer timed out'}
  if((Get-OwnedExitCode $installHandle) -ne 0){throw 'Normal A installer failed'}
  $manifest.installed_A=@{}
  foreach($file in @('ppvpn.exe','ppvpn_flutter.dll','WinSparkle.dll','ppvpn-service.exe')){$manifest.installed_A[$file]=(Get-FileHash (Join-Path $dir $file) -Algorithm SHA256).Hash}
  $serviceBefore=Get-CimInstance Win32_Service -Filter "Name='ppvpn_service'"
  $serviceBefore | Select-Object Name,State,ProcessId,PathName | ConvertTo-Json | Set-Content "$out/service-A.json"
  New-Item -ItemType Directory -Force $data | Out-Null
  [IO.File]::WriteAllText((Join-Path $data 'silent-update.json'),'{"auto_update":false}')
  [IO.File]::WriteAllText((Join-Path $data 'app-settings.json'),'{"Language":"en","AutoCheckUpdates":true}')
  New-Item 'HKCU:/Software/PPVPN/WinSparkle' -Force | Out-Null
  Set-ItemProperty 'HKCU:/Software/PPVPN/WinSparkle' -Name LastCheckTime -Value '0'
  $feed=Join-Path $PSScriptRoot 'client-upgrade-feed.py'
  $packages=Join-Path $repo 'build/update-installers/B'
  $arguments=@('"'+$feed+'"','--package-dir','"'+$packages+'"','--private-dir','"'+$env:PPVPN_QA_PRIVATE+'"','--log','"'+$out+'/feed.jsonl"')
  $server=Start-Process python -ArgumentList $arguments -PassThru -RedirectStandardOutput "$out/feed-stdout.txt" -RedirectStandardError "$out/feed-stderr.txt"
  Start-Sleep -Seconds 1
  if($server.HasExited){throw 'Controlled TLS feed did not start'}
  Push-Location $out
  try {
    $observer=Start-Process "$repo/build/update-qa/client-upgrade-qa.exe" -ArgumentList ('"'+$dir+'/ppvpn.exe" "'+$repo+'/build/update-qa/update-prompt-qa.exe" 0.7.5.60000 0.7.5.60001') -PassThru -RedirectStandardOutput 'actual-client-upgrade.txt' -RedirectStandardError 'actual-client-upgrade-errors.txt'
    $observerHandle=$observer.Handle
    $observerStarted=$observer.StartTime
    if(-not $observer.WaitForExit(30*60*1000)){
      Stop-Process -Id $observer.Id -Force
      throw 'Actual client observer timed out; acceptance is UNKNOWN/FAIL'
    }
    Get-Content 'actual-client-upgrade.txt'
    if((Get-OwnedExitCode $observerHandle) -ne 0){Get-Content 'actual-client-upgrade-errors.txt';throw 'Actual client upgrade fixture failed'}
  }finally{Pop-Location}
  $serviceAfter=Get-CimInstance Win32_Service -Filter "Name='ppvpn_service'"
  $serviceAfter | Select-Object Name,State,ProcessId,PathName | ConvertTo-Json | Set-Content "$out/service-B.json"
  if($serviceAfter.State -ne 'Running' -or $serviceAfter.ProcessId -eq $serviceBefore.ProcessId){throw 'Service did not actually restart during normal installation'}
  $manifest.installed_B=@{}
  foreach($file in @('ppvpn.exe','ppvpn_flutter.dll','WinSparkle.dll','ppvpn-service.exe')){$manifest.installed_B[$file]=(Get-FileHash (Join-Path $dir $file) -Algorithm SHA256).Hash}
  $manifest|ConvertTo-Json -Depth 8|Set-Content "$out/artifact-manifest.json"
  if($manifest.installed_B['ppvpn.exe'] -ne $manifest.hashes.B_exe){throw 'Installed B executable differs from the normal B build'}
  $installedBridge=Get-FileHash "$dir/ppvpn_flutter.dll" -Algorithm SHA256
  $builtBridge=Get-FileHash "$repo/build/update-bundles/B/ppvpn_flutter.dll" -Algorithm SHA256
  if($installedBridge.Hash -ne $builtBridge.Hash){throw 'Installed B bridge differs from normal B build'}
  $requests=@(Get-Content "$out/feed.jsonl" | ForEach-Object {$_|ConvertFrom-Json})
  $roles=@(Get-Content "$out/client-processes.jsonl"|ForEach-Object{$_|ConvertFrom-Json})
  foreach($stage in @('A','B','restarted-B')){
    $clientPid=($roles|Where-Object {$_.stage -eq $stage}|Select-Object -Last 1).pid
    if(-not $clientPid -or @($requests|Where-Object {$_.path -like '*appcast*' -and $_.client_pid -eq $clientPid -and $_.client_stage -eq $stage -and $_.outcome -eq 'written'}).Count -lt 1){throw "No actual appcast response bound to $stage process"}
  }
  $samples=@(Get-Content "$out/window-samples.csv"|ConvertFrom-Csv -Header phase,tick,foreground_pid,A_pid,B_pid,A_update,B_update,B_window)
  $initial=@($samples|Where-Object {$_.phase -eq 'A-initial-fullscreen'})
  if(-not $initial.Count){throw 'Initial fullscreen observation samples missing'}
  $first=[uint64]$initial[0].tick;$last=[uint64]$initial[-1].tick
  if(@($requests|Where-Object {$_.client_stage -eq 'A' -and $_.outcome -eq 'written' -and $_.path -like '*appcast*' -and [uint64]$_.finished_tick -ge $first -and [uint64]$_.finished_tick -le $last}).Count -lt 1){throw 'No actual A response completed inside the fullscreen sample interval'}
  if(@($requests|Where-Object {$_.path -like '*setup.exe'}).Count -lt 1){throw 'Real updater never downloaded B installer'}
  @{source=$env:GITHUB_SHA;os=(Get-CimInstance Win32_OperatingSystem|Select-Object Caption,Version,BuildNumber);scope='actual client A-to-B via existing WinSparkle fallback and NSIS; not silent InstallUpdate IPC coverage';service_restarted=$true;installed_B_bridge_sha256=$installedBridge.Hash;feed_responses=$requests;second_monitor='not guaranteed';commercial_game='not exercised'}|ConvertTo-Json -Depth 8|Set-Content "$out/result.json"
}finally{
  if($observer){
    $helper=Join-Path $repo 'build/update-qa/update-prompt-qa.exe'
    $ownedChildren=@(Get-CimInstance Win32_Process -Filter "ParentProcessId=$($observer.Id)"|Where-Object {$_.ExecutablePath -eq $helper -and $_.CreationDate -ge $observerStarted})
    $ownedChildren|Select-Object ProcessId,ParentProcessId,ExecutablePath,CreationDate|ConvertTo-Json|Set-Content "$out/owned-child-cleanup.json"
    foreach($child in $ownedChildren){Stop-Process -Id $child.ProcessId -Force -ErrorAction SilentlyContinue}
    if(-not $observer.HasExited){Stop-Process -Id $observer.Id -Force}
  }
  if($server -and -not $server.HasExited){Stop-Process -Id $server.Id}
  # This fixture refused an existing app/service at entry. Everything below was
  # installed by this test; retain logs before the normal uninstaller purges it.
  if(Test-Path "$data/logs"){Copy-Item "$data/logs" "$out/client-logs" -Recurse -Force}
  if(Test-Path "$dir/uninstall.exe"){
    $uninstall=Start-Process "$dir/uninstall.exe" -ArgumentList '/S /PURGE' -PassThru
    $uninstallHandle=$uninstall.Handle
    if(-not $uninstall.WaitForExit(180000)){throw 'Normal test uninstaller timed out'}
    if((Get-OwnedExitCode $uninstallHandle) -ne 0){throw 'Normal test uninstaller failed'}
  }
  # NSIS uninstallation executes from a temporary child. Parent exit alone
  # is not completion; the startup diagnostic confirmed this bounded wait.
  $cleanupDeadline=[DateTime]::UtcNow.AddSeconds(60)
  do {
    $cleanup=@{app_processes=@(Get-Process ppvpn -ErrorAction SilentlyContinue|Select-Object Id,Path);service_remaining=[bool](Get-Service ppvpn_service -ErrorAction SilentlyContinue);directory_remaining=(Test-Path $dir)}
    if(-not $cleanup.app_processes.Count -and -not $cleanup.service_remaining){break}
    Start-Sleep -Milliseconds 250
  }while([DateTime]::UtcNow -lt $cleanupDeadline)
  $cleanup|ConvertTo-Json -Depth 4|Set-Content "$out/cleanup.json"
  if($cleanup.app_processes.Count -or $cleanup.service_remaining){throw 'Owned test client/service remained after normal uninstallation'}
}
