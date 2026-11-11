# Existing updater + existing installer. Run only on a clean disposable CI host.
$ErrorActionPreference='Stop'
if(-not $env:GITHUB_ACTIONS){throw 'This destructive installer fixture is CI-only; never run on a user VM'}
if((Get-Service ppvpn_service -ErrorAction SilentlyContinue) -or (Get-Process ppvpn -ErrorAction SilentlyContinue) -or (Test-Path "$env:ProgramFiles/PPVPN")){throw 'Refusing to replace an existing user app/service'}
$repo=(Resolve-Path "$PSScriptRoot/../../..").Path
$out=Join-Path $repo 'build/client-upgrade-qa'
New-Item -ItemType Directory -Force $out | Out-Null
$dir=Join-Path $env:ProgramFiles 'PPVPN'
$data=Join-Path $env:LOCALAPPDATA 'PPVPN'
$server=$null
try {
  $a=Join-Path $repo 'build/update-installers/A/PPVPN-0.7.5-windows-x64-setup.exe'
  $install=Start-Process $a -ArgumentList '/S' -Wait -PassThru
  if($install.ExitCode){throw 'Normal A installer failed'}
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
    & "$repo/build/update-qa/client-upgrade-qa.exe" "$dir/ppvpn.exe" "$repo/build/update-qa/update-prompt-qa.exe" '0.7.5.60000' '0.7.5.60001' 2>&1 | Tee-Object 'actual-client-upgrade.txt'
    if($LASTEXITCODE){throw 'Actual client upgrade fixture failed'}
  }finally{Pop-Location}
  $serviceAfter=Get-CimInstance Win32_Service -Filter "Name='ppvpn_service'"
  $serviceAfter | Select-Object Name,State,ProcessId,PathName | ConvertTo-Json | Set-Content "$out/service-B.json"
  if($serviceAfter.State -ne 'Running' -or $serviceAfter.ProcessId -eq $serviceBefore.ProcessId){throw 'Service did not actually restart during normal installation'}
  $installedBridge=Get-FileHash "$dir/ppvpn_flutter.dll" -Algorithm SHA256
  $builtBridge=Get-FileHash "$repo/build/update-bundles/B/ppvpn_flutter.dll" -Algorithm SHA256
  if($installedBridge.Hash -ne $builtBridge.Hash){throw 'Installed B bridge differs from normal B build'}
  $requests=@(Get-Content "$out/feed.jsonl" | ForEach-Object {$_|ConvertFrom-Json})
  if(@($requests|Where-Object {$_.path -like '*appcast*'}).Count -lt 3){throw 'Did not observe A, B and restarted-B actual feed responses'}
  if(@($requests|Where-Object {$_.path -like '*setup.exe'}).Count -lt 1){throw 'Real updater never downloaded B installer'}
  @{source=$env:GITHUB_SHA;os=(Get-CimInstance Win32_OperatingSystem|Select-Object Caption,Version,BuildNumber);scope='actual client A-to-B via existing WinSparkle fallback and NSIS; not silent InstallUpdate IPC coverage';service_restarted=$true;installed_B_bridge_sha256=$installedBridge.Hash;feed_responses=$requests;second_monitor='not guaranteed';commercial_game='not exercised'}|ConvertTo-Json -Depth 8|Set-Content "$out/result.json"
}finally{
  if($server -and -not $server.HasExited){Stop-Process -Id $server.Id}
  # This fixture refused an existing app/service at entry. Everything below was
  # installed by this test; retain logs before the normal uninstaller purges it.
  if(Test-Path "$data/logs"){Copy-Item "$data/logs" "$out/client-logs" -Recurse -Force}
  if(Test-Path "$dir/uninstall.exe"){
    $uninstall=Start-Process "$dir/uninstall.exe" -ArgumentList '/S /PURGE' -Wait -PassThru
    if($uninstall.ExitCode){Write-Error 'Normal test uninstaller failed'}
  }
  @{app_processes=@(Get-Process ppvpn -ErrorAction SilentlyContinue|Select-Object Id,Path);service_remaining=[bool](Get-Service ppvpn_service -ErrorAction SilentlyContinue);directory_remaining=(Test-Path $dir)}|ConvertTo-Json -Depth 4|Set-Content "$out/cleanup.json"
}
