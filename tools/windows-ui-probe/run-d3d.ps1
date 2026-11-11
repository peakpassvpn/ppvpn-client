param([Parameter(Mandatory=$true)][string]$OutputDir)
$ErrorActionPreference='Stop'
$exe=Join-Path $OutputDir 'd3d-exclusive.exe'
$p=Start-Process -FilePath $exe -ArgumentList ('"'+$OutputDir+'"') -PassThru -RedirectStandardOutput (Join-Path $OutputDir 'd3d.stdout.log') -RedirectStandardError (Join-Path $OutputDir 'd3d.stderr.log')
$started=$p.StartTime.ToUniversalTime().Ticks
$forced=$false
if(-not $p.WaitForExit(60000)){
 $current=Get-Process -Id $p.Id -ErrorAction SilentlyContinue
 if($null -ne $current -and $current.StartTime.ToUniversalTime().Ticks -eq $started -and $current.Path -eq $exe){Stop-Process -Id $p.Id -Force;$forced=$true;[void]$p.WaitForExit(5000)}
}
$p.Refresh()
$r=@{head=$env:GITHUB_SHA;run_id=$env:GITHUB_RUN_ID;owned_pid=$p.Id;image=$exe;start_ticks=$started;session_id=$p.SessionId;forced_owned_termination=$forced;process_exit_observed=$p.HasExited;source_files=@{};exe_sha256=(Get-FileHash $exe -Algorithm SHA256).Hash.ToLowerInvariant()}
if($p.HasExited){$r.exit_code=$p.ExitCode}
Get-ChildItem $PSScriptRoot -File|ForEach-Object {$r.source_files[$_.Name]=(Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
[IO.File]::WriteAllText((Join-Path $OutputDir 'd3d-process.json'),($r|ConvertTo-Json -Depth 8),(New-Object Text.UTF8Encoding $false))
if(Test-Path (Join-Path $OutputDir 'd3d-result.json')){Get-Content (Join-Path $OutputDir 'd3d-result.json') -Raw}
if(-not $p.HasExited -or $forced -or $p.ExitCode -ne 0){exit 77}
