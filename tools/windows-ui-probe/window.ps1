param([Parameter(Mandatory=$true)][string]$CommandDir)
$ErrorActionPreference='Stop'
Add-Type -AssemblyName PresentationFramework, PresentationCore, WindowsBase, System.Windows.Forms
Add-Type @'
using System; using System.Runtime.InteropServices;
public static class PpvProbeWindowNative {
 [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr window);
}
'@
function Save-Json($Name,$Value){[IO.File]::WriteAllText((Join-Path $CommandDir $Name),($Value|ConvertTo-Json -Depth 8),(New-Object System.Text.UTF8Encoding $false))}
$script:app=New-Object System.Windows.Application
$script:window=New-Object System.Windows.Window
$script:window.Title='Independent desktop capability probe - not PPVPN'
$script:window.Width=800;$script:window.Height=500
$script:window.Background=[System.Windows.Media.BrushConverter]::new().ConvertFromString('#2463EB')
$script:text=New-Object System.Windows.Controls.TextBlock
$script:text.Foreground=[System.Windows.Media.Brushes]::White
$script:text.FontSize=30;$script:text.TextWrapping='Wrap';$script:text.Margin=40;$script:text.VerticalAlignment='Center'
$script:window.Content=$script:text
$script:lastCommand='';$script:deadline=[DateTime]::UtcNow.AddSeconds(120)
$script:timer=New-Object System.Windows.Threading.DispatcherTimer
$script:timer.Interval=[TimeSpan]::FromMilliseconds(200)
$script:timer.Add_Tick({
 try {
  if([DateTime]::UtcNow -gt $script:deadline){$script:window.Close();return}
  $path=Join-Path $CommandDir 'command.json'
  if(-not (Test-Path -LiteralPath $path)){return}
  try{$c=Get-Content -LiteralPath $path -Raw|ConvertFrom-Json}catch{return}
  if($c.id -eq $script:lastCommand){return}
  if($c.mode -notin @('normal','maximized','fullscreen','exit-fullscreen','close')){throw 'Invalid owned-window mode'}
  $script:lastCommand=$c.id
  if($c.mode -eq 'close'){$script:window.Close();return}
  $screens=[System.Windows.Forms.Screen]::AllScreens
  if($c.monitor -lt 0 -or $c.monitor -ge $screens.Count){throw 'Requested monitor does not exist'}
  $bounds=$screens[$c.monitor].Bounds
  $script:window.WindowState='Normal';$script:window.Topmost=$false
  if($c.mode -eq 'fullscreen') {
   $script:window.WindowStyle='None';$script:window.ResizeMode='NoResize'
   $script:window.Left=$bounds.X;$script:window.Top=$bounds.Y
   $script:window.Width=$bounds.Width;$script:window.Height=$bounds.Height;$script:window.Topmost=$true
  } else {
   $script:window.WindowStyle='SingleBorderWindow';$script:window.ResizeMode='CanResize'
   $script:window.Width=[Math]::Min(800,$bounds.Width-80);$script:window.Height=[Math]::Min(500,$bounds.Height-80)
   $script:window.Left=$bounds.X+40;$script:window.Top=$bounds.Y+40
   if($c.mode -eq 'maximized'){$script:window.WindowState='Maximized'}
  }
  $script:text.Text="REAL OTHER-PROCESS WINDOW`n$($c.mode)`nMonitor $($c.monitor) - PID $PID`n$([DateTime]::UtcNow.ToString('o'))"
  $script:window.UpdateLayout();$script:window.Activate()|Out-Null
  $handle=([System.Windows.Interop.WindowInteropHelper]::new($script:window)).Handle
  $focused=[PpvProbeWindowNative]::SetForegroundWindow($handle)
  Save-Json 'ack.json' @{id=$c.id;mode=$c.mode;monitor=$c.monitor;pid=$PID;hwnd=$handle.ToInt64();activation_requested=$focused;utc=[DateTime]::UtcNow.ToString('o')}
 }catch{
  Save-Json 'helper-error.json' @{type=$_.Exception.GetType().FullName;message=$_.Exception.Message;utc=[DateTime]::UtcNow.ToString('o')}
  $script:window.Close()
 }
})
$script:window.Add_Loaded({
 $handle=([System.Windows.Interop.WindowInteropHelper]::new($script:window)).Handle
 Save-Json 'ready.json' @{pid=$PID;hwnd=$handle.ToInt64();utc=[DateTime]::UtcNow.ToString('o')}
 $script:timer.Start()
})
$script:window.Add_Closed({$script:timer.Stop();$script:app.Shutdown()})
[void]$script:app.Run($script:window)
