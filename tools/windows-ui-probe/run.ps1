param([Parameter(Mandatory=$true)][string]$OutputDir)
$ErrorActionPreference='Stop'
New-Item -ItemType Directory -Force -Path $OutputDir|Out-Null
function Save-Json($Path,$Value){[IO.File]::WriteAllText($Path,($Value|ConvertTo-Json -Depth 14),(New-Object System.Text.UTF8Encoding $false))}
$report=@{schema=1;purpose='Actual Windows desktop capability only; no product/updater acceptance';head=$env:GITHUB_SHA;run_id=$env:GITHUB_RUN_ID;parent_pid=$PID;stages=@();cleanup=@{complete=$false}}
$child=$null;$childStarted=$null
try {
 $report.os=Get-CimInstance Win32_OperatingSystem | Select-Object Caption,Version,BuildNumber,OSArchitecture
 $report.runner_image=@{image_os=$env:ImageOS;image_version=$env:ImageVersion;architecture=$env:RUNNER_ARCH}
 & "$PSScriptRoot/observe.ps1" -OutputDir (Join-Path $OutputDir 'baseline')
 $baseline=Get-Content (Join-Path $OutputDir 'baseline/desktop-session.json') -Raw|ConvertFrom-Json
 $report.baseline=$baseline
 $control=Join-Path $OutputDir 'owned-window';New-Item -ItemType Directory -Force $control|Out-Null
 $helper=Join-Path $PSScriptRoot 'window.ps1';$report.helper_sha256=(Get-FileHash $helper -Algorithm SHA256).Hash.ToLowerInvariant()
 $exe=Join-Path $env:WINDIR 'System32/WindowsPowerShell/v1.0/powershell.exe'
 $helperArgs=@('-NoProfile','-NonInteractive','-ExecutionPolicy','Bypass','-STA','-File',('"'+$helper+'"'),'-CommandDir',('"'+$control+'"'))
 $child=Start-Process -FilePath $exe -ArgumentList $helperArgs -PassThru -RedirectStandardOutput (Join-Path $control 'stdout.log') -RedirectStandardError (Join-Path $control 'stderr.log')
 $childStarted=$child.StartTime.ToUniversalTime().Ticks
 $report.owned_process=@{pid=$child.Id;image=$child.Path;start_ticks=$childStarted;session_id=$child.SessionId}
 $until=[DateTime]::UtcNow.AddSeconds(20)
 while(-not (Test-Path (Join-Path $control 'ready.json')) -and [DateTime]::UtcNow -lt $until -and -not $child.HasExited){Start-Sleep -Milliseconds 200;$child.Refresh()}
 if(-not (Test-Path (Join-Path $control 'ready.json'))){throw 'Real helper window did not become ready'}
 $ready=Get-Content (Join-Path $control 'ready.json') -Raw|ConvertFrom-Json
 if($ready.pid -ne $child.Id -or $ready.hwnd -eq 0){throw 'Owned HWND/PID binding failed'}
 $handle=[IntPtr][long]$ready.hwnd
 [PpvDesktopReadOnly]::AllowSetForegroundWindow([uint32]$child.Id)|Out-Null
 $screens=[System.Windows.Forms.Screen]::AllScreens
 $plan=@(@{mode='normal';monitor=0},@{mode='maximized';monitor=0},@{mode='fullscreen';monitor=0},@{mode='exit-fullscreen';monitor=0})
 if($screens.Count -gt 1){$plan+=@(@{mode='fullscreen';monitor=1},@{mode='exit-fullscreen';monitor=1});$report.second_monitor='AVAILABLE_ACTUALLY_EXERCISED'}else{$report.second_monitor='UNAVAILABLE_NOT_EXERCISED'}
 foreach($stage in $plan){
  $id=[Guid]::NewGuid().ToString();Save-Json (Join-Path $control 'command.json') @{id=$id;mode=$stage.mode;monitor=$stage.monitor}
  $until=[DateTime]::UtcNow.AddSeconds(10);$ack=$null
  while([DateTime]::UtcNow -lt $until){
   try{$a=Get-Content (Join-Path $control 'ack.json') -Raw|ConvertFrom-Json;if($a.id -eq $id){$ack=$a;break}}catch{}
   if(Test-Path (Join-Path $control 'helper-error.json')){throw 'Owned helper reported an error'}
   Start-Sleep -Milliseconds 200
  }
  if($null -eq $ack){throw 'Owned window stage acknowledgement timed out'}
  [PpvDesktopReadOnly]::SetForegroundWindow($handle)|Out-Null;Start-Sleep -Seconds 2
  $samples=@()
  for($n=0;$n -lt 3;$n++){
   $fg=[PpvDesktopReadOnly]::GetForegroundWindow();[uint32]$owner=0
   $thread=[PpvDesktopReadOnly]::GetWindowThreadProcessId($fg,[ref]$owner)
   $rect=New-Object PpvDesktopReadOnly+Rect;$hr=[PpvDesktopReadOnly]::DwmGetWindowAttribute($handle,9,[ref]$rect,16)
   $styles=[PpvDesktopReadOnly]::GetWindowLongPtr($handle,-16).ToInt64()
   $samples+=@{foreground_hwnd=$fg.ToInt64();foreground_pid=$owner;foreground_thread=$thread;dwm_hresult=$hr;frame=$rect;style=$styles;caption_present=(($styles -band 0x00C00000) -eq 0x00C00000);zoomed=[PpvDesktopReadOnly]::IsZoomed($handle);utc=[DateTime]::UtcNow.ToString('o')}
   Start-Sleep -Milliseconds 250
  }
  $b=[System.Windows.Forms.SystemInformation]::VirtualScreen;$bitmap=New-Object System.Drawing.Bitmap $b.Width,$b.Height;$graphics=[System.Drawing.Graphics]::FromImage($bitmap)
  try{
   $graphics.CopyFromScreen($b.Location,[System.Drawing.Point]::Empty,$b.Size)
   $image=Join-Path $OutputDir ("$($stage.mode)-monitor$($stage.monitor).png");$bitmap.Save($image,[System.Drawing.Imaging.ImageFormat]::Png)
   $nonblack=0;$colors=New-Object 'System.Collections.Generic.HashSet[int]'
   for($y=0;$y -lt $b.Height;$y+=16){for($x=0;$x -lt $b.Width;$x+=16){$c=$bitmap.GetPixel($x,$y);[void]$colors.Add($c.ToArgb());if($c.R -gt 8 -or $c.G -gt 8 -or $c.B -gt 8){$nonblack++}}}
  }finally{$graphics.Dispose();$bitmap.Dispose()}
  $expected=$screens[$stage.monitor].Bounds;$last=$samples[-1];$r=$last.frame
  $fullGeometry=([Math]::Abs($r.Left-$expected.Left) -le 4 -and [Math]::Abs($r.Top-$expected.Top) -le 4 -and [Math]::Abs($r.Right-$expected.Right) -le 4 -and [Math]::Abs($r.Bottom-$expected.Bottom) -le 4)
  $ownedForeground=@($samples|Where-Object {$_.foreground_pid -ne $child.Id -or $_.foreground_hwnd -ne $handle.ToInt64()}).Count -eq 0
  $stateOk=if($stage.mode -eq 'fullscreen'){$fullGeometry -and -not $last.caption_present -and -not $last.zoomed}elseif($stage.mode -eq 'maximized'){$last.caption_present -and $last.zoomed}else{$last.caption_present -and -not $last.zoomed -and -not $fullGeometry}
  $row=@{mode=$stage.mode;monitor=$stage.monitor;ack=$ack;samples=$samples;monitor_bounds=@{left=$expected.Left;top=$expected.Top;right=$expected.Right;bottom=$expected.Bottom};working_area=$screens[$stage.monitor].WorkingArea;matches_full_monitor_geometry=$fullGeometry;actual_state_match=$stateOk;actual_owned_foreground=$ownedForeground;screenshot=@{path=$image;sha256=(Get-FileHash $image -Algorithm SHA256).Hash.ToLowerInvariant();nonblack_sample_pixels=$nonblack;unique_sample_colors=$colors.Count};passed=($stateOk -and $ownedForeground -and $last.dwm_hresult -eq 0 -and $nonblack -gt 0)}
  $report.stages+=,$row;Save-Json (Join-Path $OutputDir 'stages-progress.json') $report
 }
 $access=$baseline.process_session_id -gt 0 -and $baseline.window_station.visible -and $baseline.input_desktop_open_error -eq 0 -and $baseline.thread_desktop_name -eq $baseline.input_desktop_name
 $report.desktop_access_observed=$access;$report.capability_pass=$access -and @($report.stages|Where-Object {-not $_.passed}).Count -eq 0
}catch{
 $report.error=@{message=$_.Exception.Message;type=$_.Exception.GetType().FullName;hresult=$_.Exception.HResult};$report.capability_pass=$false
}finally{
 if($null -ne $child){
  $child.Refresh()
  if(-not $child.HasExited){
   try{Save-Json (Join-Path $control 'command.json') @{id=[Guid]::NewGuid().ToString();mode='close';monitor=0}}catch{}
   if(-not $child.WaitForExit(5000)){
    $same=Get-Process -Id $child.Id -ErrorAction SilentlyContinue
    if($null -ne $same -and $same.StartTime.ToUniversalTime().Ticks -eq $childStarted -and $same.Path -eq $exe){Stop-Process -Id $child.Id -Force;$report.cleanup.forced_owned_pid=$child.Id;[void]$child.WaitForExit(5000)}
   }
  }
  $child.Refresh();$report.cleanup.complete=$child.HasExited
  if($child.HasExited){$report.cleanup.exit_code=$child.ExitCode}
 }else{$report.cleanup.complete=$true}
 $report.product_or_updater_exercised=$false;$report.source_files=@{}
 Get-ChildItem $PSScriptRoot -Filter '*.ps1'|ForEach-Object {$report.source_files[$_.Name]=(Get-FileHash $_.FullName -Algorithm SHA256).Hash.ToLowerInvariant()}
 Save-Json (Join-Path $OutputDir 'result.json') $report
}
if(-not $report.capability_pass -or -not $report.cleanup.complete){exit 2}
