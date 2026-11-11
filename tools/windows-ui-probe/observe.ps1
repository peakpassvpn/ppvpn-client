param([Parameter(Mandatory=$true)][string]$OutputDir, [switch]$RequireInteractive)
# Read-only desktop capability observations. No launch, input, focus, install or service changes.
$ErrorActionPreference = 'Stop'
New-Item -ItemType Directory -Force -Path $OutputDir | Out-Null
Add-Type -AssemblyName System.Windows.Forms, System.Drawing
Add-Type -TypeDefinition @'
using System;
using System.Text;
using System.Runtime.InteropServices;
public static class PpvDesktopReadOnly {
  [StructLayout(LayoutKind.Sequential)] public struct Rect { public int Left,Top,Right,Bottom; }
  [StructLayout(LayoutKind.Sequential)] public struct Flags { public int Inherit,Reserved; public uint Value; }
  [DllImport("kernel32.dll")] public static extern uint GetCurrentThreadId();
  [DllImport("kernel32.dll")] public static extern uint WTSGetActiveConsoleSessionId();
  [DllImport("user32.dll")] public static extern IntPtr GetProcessWindowStation();
  [DllImport("user32.dll")] public static extern IntPtr GetThreadDesktop(uint id);
  [DllImport("user32.dll",SetLastError=true)] public static extern IntPtr OpenInputDesktop(uint flags,bool inherit,uint access);
  [DllImport("user32.dll",SetLastError=true)] public static extern bool CloseDesktop(IntPtr h);
  [DllImport("user32.dll",EntryPoint="GetUserObjectInformationW",CharSet=CharSet.Unicode,SetLastError=true)]
  public static extern bool ObjectName(IntPtr h,int index,StringBuilder value,uint length,out uint needed);
  [DllImport("user32.dll",EntryPoint="GetUserObjectInformationW",SetLastError=true)]
  public static extern bool ObjectFlags(IntPtr h,int index,out Flags value,uint length,out uint needed);
  [DllImport("user32.dll")] public static extern IntPtr GetForegroundWindow();
  [DllImport("user32.dll")] public static extern uint GetWindowThreadProcessId(IntPtr h,out uint pid);
  [DllImport("user32.dll",SetLastError=true)] public static extern bool GetWindowRect(IntPtr h,out Rect rect);
  [DllImport("user32.dll")] public static extern int GetSystemMetrics(int index);
  [DllImport("dwmapi.dll")] public static extern int DwmIsCompositionEnabled(out bool enabled);
  [DllImport("dwmapi.dll")] public static extern int DwmGetWindowAttribute(IntPtr h,uint attr,out Rect rect,int size);
  [DllImport("user32.dll")] public static extern bool SetForegroundWindow(IntPtr h);
  [DllImport("user32.dll")] public static extern bool AllowSetForegroundWindow(uint pid);
  [DllImport("user32.dll")] public static extern bool IsZoomed(IntPtr h);
  [DllImport("user32.dll",EntryPoint="GetWindowLongPtrW")] public static extern IntPtr GetWindowLongPtr(IntPtr h,int index);
  public static string Name(IntPtr h) {
    if(h==IntPtr.Zero)return null;
    uint needed; var text=new StringBuilder(512);
    return ObjectName(h,2,text,1024,out needed)?text.ToString():null;
  }
}
'@
$station=[PpvDesktopReadOnly]::GetProcessWindowStation()
$threadDesktop=[PpvDesktopReadOnly]::GetThreadDesktop([PpvDesktopReadOnly]::GetCurrentThreadId())
$inputDesktopHandle=[PpvDesktopReadOnly]::OpenInputDesktop(0,$false,1)
$inputDesktopHandleError=if($inputDesktopHandle -eq [IntPtr]::Zero){[Runtime.InteropServices.Marshal]::GetLastWin32Error()}else{0}
$flags=New-Object PpvDesktopReadOnly+Flags
[uint32]$needed=0
$flagsOk=[PpvDesktopReadOnly]::ObjectFlags($station,1,[ref]$flags,12,[ref]$needed)
$foreground=[PpvDesktopReadOnly]::GetForegroundWindow()
[uint32]$foregroundPid=0
$foregroundThread=[PpvDesktopReadOnly]::GetWindowThreadProcessId($foreground,[ref]$foregroundPid)
$rect=New-Object PpvDesktopReadOnly+Rect
$rectOk=[PpvDesktopReadOnly]::GetWindowRect($foreground,[ref]$rect)
$dwmRect=New-Object PpvDesktopReadOnly+Rect
$dwmRectHr=[PpvDesktopReadOnly]::DwmGetWindowAttribute($foreground,9,[ref]$dwmRect,16)
$composed=$false
$dwmHr=[PpvDesktopReadOnly]::DwmIsCompositionEnabled([ref]$composed)
$screens=@([Windows.Forms.Screen]::AllScreens | ForEach-Object {
  @{device=$_.DeviceName;primary=$_.Primary;bounds=@{x=$_.Bounds.X;y=$_.Bounds.Y;width=$_.Bounds.Width;height=$_.Bounds.Height}}
})
$capture=@{success=$false}
$bitmap=$null;$graphics=$null
try {
  $b=[Windows.Forms.SystemInformation]::VirtualScreen
  $bitmap=New-Object Drawing.Bitmap $b.Width,$b.Height
  $graphics=[Drawing.Graphics]::FromImage($bitmap)
  $graphics.CopyFromScreen($b.Location,[Drawing.Point]::Empty,$b.Size)
  $image=Join-Path $OutputDir 'desktop.png'
  $bitmap.Save($image,[Drawing.Imaging.ImageFormat]::Png)
  $capture=@{success=$true;path=$image;sha256=(Get-FileHash $image -Algorithm SHA256).Hash.ToLowerInvariant();width=$b.Width;height=$b.Height}
} catch {
  $capture=@{success=$false;exception_type=$_.Exception.GetType().FullName;hresult=$_.Exception.HResult}
} finally {
  if($null -ne $graphics){$graphics.Dispose()};if($null -ne $bitmap){$bitmap.Dispose()}
}
$session=[Diagnostics.Process]::GetCurrentProcess().SessionId
$inputDesktopHandleName=[PpvDesktopReadOnly]::Name($inputDesktopHandle)
$threadName=[PpvDesktopReadOnly]::Name($threadDesktop)
$visible=$flagsOk -and (($flags.Value -band 1) -ne 0)
$candidate=$session -gt 0 -and $visible -and $inputDesktopHandle -ne [IntPtr]::Zero -and $inputDesktopHandleName -eq $threadName -and $foreground -ne [IntPtr]::Zero -and $screens.Count -gt 0 -and $capture.success
$report=@{
  schema=1;observed_utc=[DateTime]::UtcNow.ToString('o');observations_only=$true
  process_session_id=$session;active_console_session_id=[PpvDesktopReadOnly]::WTSGetActiveConsoleSessionId()
  remote_session_metric=[PpvDesktopReadOnly]::GetSystemMetrics(0x1000)
  window_station=@{name=[PpvDesktopReadOnly]::Name($station);flags_read_ok=$flagsOk;flags=$flags.Value;visible=$visible}
  thread_desktop_name=$threadName;input_desktop_name=$inputDesktopHandleName;input_desktop_open_error=$inputDesktopHandleError
  foreground=@{hwnd=$foreground.ToInt64();pid=$foregroundPid;thread_id=$foregroundThread;rect_read_ok=$rectOk;rect=$rect;dwm_rect_hresult=$dwmRectHr;dwm_rect=$dwmRect}
  dwm=@{hresult=$dwmHr;composition_enabled=$composed};monitors=$screens;screenshot=$capture
  observed_desktop_access_candidate=$candidate
  acceptance='Capability observations only. No update/fullscreen/15-minute/restart behavior was exercised.'
}
if($inputDesktopHandle -ne [IntPtr]::Zero){$report.input_desktop_close_ok=[PpvDesktopReadOnly]::CloseDesktop($inputDesktopHandle)}
[IO.File]::WriteAllText((Join-Path $OutputDir 'desktop-session.json'), ($report | ConvertTo-Json -Depth 10), (New-Object System.Text.UTF8Encoding $false))
if($RequireInteractive -and -not $candidate){exit 2}
