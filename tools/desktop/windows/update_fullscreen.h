// Used inside WinSparkle's UI thread, immediately before showing its own UI.
#pragma once
#include <windows.h>
#include <dwmapi.h>
#pragma comment(lib, "dwmapi.lib")

namespace ppvpn_update {
// Physical monitor bounds, not work area: a normal maximized window must not
// suppress notifications. Supports negative coordinates and borderless games.
inline bool covers_monitor(const RECT& frame, const RECT& monitor,
                           bool maximized, LONG_PTR style) {
    if (maximized && (style & WS_CAPTION) == WS_CAPTION)
        return false;
    return frame.left <= monitor.left && frame.top <= monitor.top &&
           frame.right >= monitor.right && frame.bottom >= monitor.bottom;
}

inline bool foreground_fullscreen() {
    HWND foreground = GetForegroundWindow();
    if (!foreground || foreground == GetDesktopWindow() ||
        foreground == GetShellWindow() || IsIconic(foreground))
        return false;
    MONITORINFO monitor = {sizeof(MONITORINFO)};
    if (!GetMonitorInfoW(MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST), &monitor))
        return false;
    RECT frame = {};
    if (FAILED(DwmGetWindowAttribute(foreground, DWMWA_EXTENDED_FRAME_BOUNDS,
                                    &frame, sizeof(frame))) &&
        !GetWindowRect(foreground, &frame))
        return false;
    return covers_monitor(frame, monitor.rcMonitor, IsZoomed(foreground) != FALSE,
                          GetWindowLongPtrW(foreground, GWL_STYLE));
}
} // namespace ppvpn_update
