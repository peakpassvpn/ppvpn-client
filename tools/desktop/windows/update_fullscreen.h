// Used inside WinSparkle's UI thread, immediately before showing its own UI.
#pragma once
#include <windows.h>
#include <dwmapi.h>
#include <shellapi.h>
#pragma comment(lib, "dwmapi.lib")
#pragma comment(lib, "shell32.lib")

namespace ppvpn_update {
enum class ForegroundState { windowed, fullscreen, unknown };

// Physical monitor bounds, not work area: a normal maximized window must not
// suppress notifications. Supports negative coordinates and borderless games.
inline bool covers_monitor(const RECT& frame, const RECT& monitor,
                           bool maximized, LONG_PTR style) {
    if (maximized && (style & WS_CAPTION) == WS_CAPTION)
        return false;
    return frame.left <= monitor.left && frame.top <= monitor.top &&
           frame.right >= monitor.right && frame.bottom >= monitor.bottom;
}

inline ForegroundState foreground_state() {
    QUERY_USER_NOTIFICATION_STATE notification = QUNS_ACCEPTS_NOTIFICATIONS;
    HRESULT query = SHQueryUserNotificationState(&notification);
    // Only this explicit exclusive-fullscreen state adds a positive result.
    // Quiet time, DND and presentation settings are not new product policy.
    if (SUCCEEDED(query) && notification == QUNS_RUNNING_D3D_FULL_SCREEN)
        return ForegroundState::fullscreen;
    HWND foreground = GetForegroundWindow();
    // NULL is valid while activation changes, including entering a game.
    if (!foreground) return ForegroundState::unknown;
    if (foreground == GetDesktopWindow() || foreground == GetShellWindow())
        return ForegroundState::windowed;
    if (IsIconic(foreground)) return ForegroundState::unknown;
    MONITORINFO monitor = {sizeof(MONITORINFO)};
    if (!GetMonitorInfoW(MonitorFromWindow(foreground, MONITOR_DEFAULTTONEAREST), &monitor))
        return ForegroundState::unknown;
    RECT frame = {};
    if (FAILED(DwmGetWindowAttribute(foreground, DWMWA_EXTENDED_FRAME_BOUNDS,
                                    &frame, sizeof(frame))) &&
        !GetWindowRect(foreground, &frame))
        return ForegroundState::unknown;
    SetLastError(0);
    LONG_PTR style = GetWindowLongPtrW(foreground, GWL_STYLE);
    if (!style && GetLastError()) return ForegroundState::unknown;
    if (covers_monitor(frame, monitor.rcMonitor, IsZoomed(foreground) != FALSE, style))
        return ForegroundState::fullscreen;
    return SUCCEEDED(query) ? ForegroundState::windowed : ForegroundState::unknown;
}
inline bool should_defer_prompt() {
    return foreground_state() != ForegroundState::windowed;
}
inline bool foreground_fullscreen() {
    return foreground_state() == ForegroundState::fullscreen;
}
} // namespace ppvpn_update
