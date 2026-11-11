// Actual WinSparkle DLL + a separate foreground Win32 application. No VPN,
// installer or installed PPVPN process is opened or changed by this fixture.
#include "update_fullscreen.h"
#include <cstdio>
#include <cwchar>
#include <d3d11.h>
#include <dxgi.h>
#pragma comment(lib, "d3d11.lib")
#pragma comment(lib, "dxgi.lib")
#include <string>
#include <vector>
#include <stdexcept>
#include <thread>
#include <chrono>
#pragma comment(lib, "user32.lib")
#pragma comment(lib, "gdi32.lib")
#pragma comment(lib, "advapi32.lib")

static void require(bool ok, const char* message) {
    if (!ok) throw std::runtime_error(message);
    std::printf("PASS %s\n", message); std::fflush(stdout);
}
static void pump(unsigned ms) {
    auto end = GetTickCount64() + ms;
    do {
        MSG message;
        while (PeekMessageW(&message, nullptr, 0, 0, PM_REMOVE)) {
            TranslateMessage(&message); DispatchMessageW(&message);
        }
        Sleep(20);
    } while (GetTickCount64() < end);
}
static HMODULE sparkle;
static PROCESS_INFORMATION owned_child = {};
static void stop_owned_child() {
    if (!owned_child.hProcess) return;
    HWND window = FindWindowW(L"PPVPNOtherAppQA", nullptr);
    DWORD pid = 0;
    if (window) GetWindowThreadProcessId(window, &pid);
    if (pid == owned_child.dwProcessId) PostMessageW(window, WM_CLOSE, 0, 0);
    if (WaitForSingleObject(owned_child.hProcess, 5000) == WAIT_TIMEOUT)
        TerminateProcess(owned_child.hProcess, 1); // only our dedicated fixture
    CloseHandle(owned_child.hProcess);
    owned_child.hProcess = nullptr;
}
static HWND prompt;
static BOOL CALLBACK find_prompt(HWND hwnd, LPARAM) {
    DWORD pid; GetWindowThreadProcessId(hwnd, &pid);
    if (pid == GetCurrentProcessId() && IsWindowVisible(hwnd) &&
        reinterpret_cast<HMODULE>(GetClassLongPtrW(hwnd, GCLP_HMODULE)) == sparkle)
        prompt = hwnd;
    return TRUE;
}
static HWND visible_prompt() { prompt = nullptr; EnumWindows(find_prompt, 0); return prompt; }
static void screenshot(const wchar_t* path) {
    int x = GetSystemMetrics(SM_XVIRTUALSCREEN), y = GetSystemMetrics(SM_YVIRTUALSCREEN);
    int w = GetSystemMetrics(SM_CXVIRTUALSCREEN), h = GetSystemMetrics(SM_CYVIRTUALSCREEN);
    HDC screen = GetDC(nullptr), memory = CreateCompatibleDC(screen);
    HBITMAP bitmap = CreateCompatibleBitmap(screen, w, h);
    auto previous = SelectObject(memory, bitmap);
    require(BitBlt(memory, 0, 0, w, h, screen, x, y, SRCCOPY | CAPTUREBLT) != FALSE, "desktop screenshot captured");
    BITMAPINFO info = {}; info.bmiHeader.biSize = sizeof(BITMAPINFOHEADER);
    info.bmiHeader.biWidth = w; info.bmiHeader.biHeight = -h;
    info.bmiHeader.biPlanes = 1; info.bmiHeader.biBitCount = 32; info.bmiHeader.biCompression = BI_RGB;
    std::vector<unsigned char> pixels(static_cast<size_t>(w) * h * 4);
    SelectObject(memory, previous);
    require(GetDIBits(memory, bitmap, 0, h, pixels.data(), &info, DIB_RGB_COLORS) != 0, "desktop screenshot readable");
    BITMAPFILEHEADER header = {}; header.bfType = 0x4d42;
    header.bfOffBits = sizeof(header) + sizeof(BITMAPINFOHEADER);
    header.bfSize = header.bfOffBits + static_cast<DWORD>(pixels.size());
    FILE* file; _wfopen_s(&file, path, L"wb"); require(file != nullptr, "screenshot output opened");
    fwrite(&header, sizeof(header), 1, file); fwrite(&info.bmiHeader, sizeof(info.bmiHeader), 1, file);
    fwrite(pixels.data(), pixels.size(), 1, file); fclose(file);
    DeleteObject(bitmap); DeleteDC(memory); ReleaseDC(nullptr, screen);
}
// The independent test app can also enter real DXGI exclusive fullscreen.
// Unsupported adapters/sessions are reported as unavailable, never mocked.
static IDXGISwapChain* swap_chain = nullptr;
static ID3D11Device* device = nullptr;
static ID3D11DeviceContext* context = nullptr;
static ID3D11RenderTargetView* target = nullptr;
static void leave_d3d(HWND hwnd) {
    KillTimer(hwnd, 42);
    if (swap_chain) { swap_chain->SetFullscreenState(FALSE, nullptr); swap_chain->Release(); swap_chain = nullptr; }
    if (target) { target->Release(); target = nullptr; }
    if (context) { context->Release(); context = nullptr; }
    if (device) { device->Release(); device = nullptr; }
}
static bool enter_d3d(HWND hwnd) {
    leave_d3d(hwnd);
    DXGI_SWAP_CHAIN_DESC desc = {};
    desc.BufferDesc.Format = DXGI_FORMAT_R8G8B8A8_UNORM;
    desc.BufferDesc.RefreshRate.Numerator = 60; desc.BufferDesc.RefreshRate.Denominator = 1;
    desc.SampleDesc.Count = 1; desc.BufferUsage = DXGI_USAGE_RENDER_TARGET_OUTPUT;
    desc.BufferCount = 1; desc.OutputWindow = hwnd; desc.Windowed = TRUE;
    desc.SwapEffect = DXGI_SWAP_EFFECT_DISCARD;
    desc.Flags = DXGI_SWAP_CHAIN_FLAG_ALLOW_MODE_SWITCH;
    HRESULT result = D3D11CreateDeviceAndSwapChain(nullptr, D3D_DRIVER_TYPE_HARDWARE,
        nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &desc, &swap_chain, &device, nullptr, &context);
    if (FAILED(result)) result = D3D11CreateDeviceAndSwapChain(nullptr, D3D_DRIVER_TYPE_WARP,
        nullptr, 0, nullptr, 0, D3D11_SDK_VERSION, &desc, &swap_chain, &device, nullptr, &context);
    if (FAILED(result)) { leave_d3d(hwnd); return false; }
    SetForegroundWindow(hwnd);
    result = swap_chain->SetFullscreenState(TRUE, nullptr);
    BOOL exclusive = FALSE; swap_chain->GetFullscreenState(&exclusive, nullptr);
    if (FAILED(result) || !exclusive) { leave_d3d(hwnd); return false; }
    ID3D11Texture2D* buffer = nullptr;
    result = swap_chain->GetBuffer(0, __uuidof(ID3D11Texture2D), reinterpret_cast<void**>(&buffer));
    if (SUCCEEDED(result)) { result = device->CreateRenderTargetView(buffer, nullptr, &target); buffer->Release(); }
    if (FAILED(result)) { leave_d3d(hwnd); return false; }
    SetTimer(hwnd, 42, 33, nullptr);
    return true;
}
static LRESULT CALLBACK other_proc(HWND hwnd, UINT msg, WPARAM w, LPARAM l) {
    if (msg == WM_APP + 2) return enter_d3d(hwnd) ? 1 : 0;
    if (msg == WM_TIMER && w == 42 && target) {
        const float blue[] = {0.05f, 0.15f, 0.55f, 1.0f};
        context->ClearRenderTargetView(target, blue); swap_chain->Present(1, 0); return 0;
    }
    if (msg == WM_APP + 1) {
        leave_d3d(hwnd);
        MONITORINFO monitor = {sizeof(MONITORINFO)};
        GetMonitorInfoW(MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST), &monitor);
        auto rect = w ? monitor.rcMonitor : monitor.rcWork;
        SetWindowLongPtrW(hwnd, GWL_STYLE, w ? WS_POPUP : WS_OVERLAPPEDWINDOW);
        ShowWindow(hwnd, SW_RESTORE);
        SetWindowPos(hwnd, HWND_TOP, rect.left, rect.top, rect.right - rect.left,
                     rect.bottom - rect.top, SWP_FRAMECHANGED | SWP_SHOWWINDOW);
        if (!w) ShowWindow(hwnd, SW_MAXIMIZE);
        SetForegroundWindow(hwnd);
        return 0;
    }
    if (msg == WM_PAINT) {
        PAINTSTRUCT paint; auto dc = BeginPaint(hwnd, &paint);
        const wchar_t* text = L"OTHER APP — real fullscreen fixture (not PPVPN)";
        TextOutW(dc, 40, 40, text, static_cast<int>(wcslen(text))); EndPaint(hwnd, &paint); return 0;
    }
    if (msg == WM_CLOSE) { leave_d3d(hwnd); DestroyWindow(hwnd); PostQuitMessage(0); return 0; }
    return DefWindowProcW(hwnd, msg, w, l);
}
static int other_app() {
    WNDCLASSW wc = {}; wc.lpfnWndProc = other_proc; wc.hInstance = GetModuleHandleW(nullptr);
    wc.lpszClassName = L"PPVPNOtherAppQA"; wc.hbrBackground = reinterpret_cast<HBRUSH>(COLOR_WINDOW + 1);
    RegisterClassW(&wc);
    HWND hwnd = CreateWindowW(wc.lpszClassName, L"PPVPN QA Other App", WS_OVERLAPPEDWINDOW,
        100, 100, 600, 400, nullptr, nullptr, wc.hInstance, nullptr);
    ShowWindow(hwnd, SW_SHOW); SetForegroundWindow(hwnd);
    MSG msg; while (GetMessageW(&msg, nullptr, 0, 0) > 0) { TranslateMessage(&msg); DispatchMessageW(&msg); }
    return 0;
}
static void close_prompt() {
    if (auto hwnd = visible_prompt()) PostMessageW(hwnd, WM_CLOSE, 0, 0);
    pump(500);
}
using Void = void(__cdecl*)();
template<class T> static T function(const char* name) {
    auto address = GetProcAddress(sparkle, name);
    require(address != nullptr, name); return reinterpret_cast<T>(address);
}
int wmain(int argc, wchar_t** argv) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    if (argc > 1 && std::wstring(argv[1]) == L"--other-app") return other_app();
    if (argc == 3 && std::wstring(argv[1]) == L"--capture") { screenshot(argv[2]); return 0; }
    try {
        require(argc == 4, "arguments: DLL feedURL installed-build");
        DWORD session = 0; ProcessIdToSessionId(GetCurrentProcessId(), &session);
        auto desktop = OpenInputDesktop(0, FALSE, DESKTOP_READOBJECTS | DESKTOP_SWITCHDESKTOP);
        std::printf("SESSION process=%lu console=%lu inputDesktop=%d\n", session, WTSGetActiveConsoleSessionId(), desktop != nullptr);
        if (!desktop || session == 0 || session != WTSGetActiveConsoleSessionId()) {
            if (desktop) CloseDesktop(desktop);
            std::puts("BLOCKED: no active interactive console; component UI acceptance not run"); return 77;
        }
        CloseDesktop(desktop);
        // Geometry regression cases use the production detector predicate.
        RECT monitor = {-1920,0,0,1080};
        require(ppvpn_update::covers_monitor(monitor, monitor, false, WS_POPUP), "negative-coordinate secondary monitor");
        require(!ppvpn_update::covers_monitor(monitor, monitor, true, WS_OVERLAPPEDWINDOW), "ordinary maximized is not fullscreen");
        RECT spanning = {-1920,0,1920,1080};
        require(ppvpn_update::covers_monitor(spanning, monitor, false, WS_POPUP), "borderless spanning multiple monitors");
        wchar_t executable[MAX_PATH]; GetModuleFileNameW(nullptr, executable, MAX_PATH);
        std::wstring command = L"\"" + std::wstring(executable) + L"\" --other-app";
        STARTUPINFOW startup = {sizeof(startup)}; PROCESS_INFORMATION child = {};
        require(CreateProcessW(nullptr, command.data(), nullptr, nullptr, FALSE, 0, nullptr, nullptr, &startup, &child) != FALSE, "separate other-app process started");
        owned_child = child;
        CloseHandle(child.hThread);
        HWND other = nullptr;
        for (int i=0; i<100 && !other; ++i) { pump(50); other = FindWindowW(L"PPVPNOtherAppQA", nullptr); }
        require(other != nullptr, "other-app window created");
        AllowSetForegroundWindow(child.dwProcessId);
        SendMessageW(other, WM_APP + 1, 1, 0); pump(300);
        require(GetForegroundWindow() == other && ppvpn_update::foreground_fullscreen(), "real other-app foreground fullscreen verified");
        sparkle = LoadLibraryW(argv[1]); require(sparkle != nullptr, "built WinSparkle DLL loaded");
        auto init = function<Void>("win_sparkle_init"); auto cleanup = function<Void>("win_sparkle_cleanup");
        auto check = function<Void>("win_sparkle_check_update_without_ui");
        auto feed = function<void(__cdecl*)(const char*)>("win_sparkle_set_appcast_url");
        auto automatic = function<void(__cdecl*)(int)>("win_sparkle_set_automatic_check_for_updates");
        auto registry = function<void(__cdecl*)(const char*)>("win_sparkle_set_registry_path");
        auto details = function<void(__cdecl*)(const wchar_t*,const wchar_t*,const wchar_t*)>("win_sparkle_set_app_details");
        auto build = function<void(__cdecl*)(const wchar_t*)>("win_sparkle_set_app_build_version");
        std::string key = "Software\\PPVPN\\UpdatePromptQA\\" + std::to_string(GetCurrentProcessId());
        bool permission = std::wstring(argv[3]).find(L"permission") == 0;
        bool cancel_permission = std::wstring(argv[3]) == L"permission-cancel";
        bool d3d = std::wstring(argv[3]) == L"d3d";
        registry(key.c_str());
        if (permission) {
            const char ran[] = "1";
            RegSetKeyValueA(HKEY_CURRENT_USER, key.c_str(), "DidRunOnce", REG_SZ, ran, sizeof(ran));
        } else automatic(0);
        details(L"PPVPN QA", L"PPVPN Update QA", L"0.7.5"); build(permission || d3d ? L"0.7.5.1" : argv[3]);
        std::wstring widefeed(argv[2]); std::string url(widefeed.begin(), widefeed.end()); feed(url.c_str());
        init();
        if (!permission) {
            SendMessageW(other, WM_APP+1, 0, 0); pump(300);
            check(); pump(300);
            if (d3d) {
                if (!SendMessageW(other, WM_APP+2, 0, 0)) {
                    cleanup(); stop_owned_child();
                    std::puts("BLOCKED: adapter/session cannot enter DXGI exclusive fullscreen"); return 78;
                }
            } else SendMessageW(other, WM_APP+1, 1, 0);
            pump(1000);
            if (d3d) {
                QUERY_USER_NOTIFICATION_STATE notification;
                auto result = SHQueryUserNotificationState(&notification);
                std::printf("DXGI exclusive entered; SHQueryUserNotificationState hr=%ld state=%d\n", result, int(notification));
                if (FAILED(result) || notification != QUNS_RUNNING_D3D_FULL_SCREEN) {
                    cleanup(); stop_owned_child();
                    std::puts("BLOCKED: OS did not confirm exclusive D3D fullscreen notification state"); return 78;
                }
            }
        }
        require(GetForegroundWindow() == other && ppvpn_update::foreground_fullscreen(), "entered fullscreen while delayed check was in flight");
        // The feed deliberately delays 3 seconds. Fullscreen remains real for
        // the response AND the eventual native Show/Raise call.
        for (int i=0;i<70;++i) {
            pump(100); require(!visible_prompt(), "no update window shown while fullscreen");
            require(GetForegroundWindow() == other, "fullscreen foreground retained");
        }
        screenshot(L"fullscreen-no-prompt.bmp");
        if (cancel_permission) automatic(0);
        SendMessageW(other, WM_APP+1, 0, 0); pump(300);
        require(GetForegroundWindow() == other && !ppvpn_update::foreground_fullscreen(), "normal maximized foreground is allowed");
        for (int i=0;i<100 && !visible_prompt();++i) pump(100);
        bool newer = std::wstring(argv[3]) == L"0.7.5.1" || (permission && !cancel_permission) || d3d;
        require(newer ? visible_prompt() != nullptr : visible_prompt() == nullptr, newer ? "original native update dialog appears after fullscreen exits" : "target build does not prompt after relaunch");
        screenshot(L"after-fullscreen.bmp"); close_prompt();
        if (newer && !permission) {
            SendMessageW(other, WM_APP+1, 1, 0); pump(300);
            const char zero[] = "0";
            RegSetKeyValueA(HKEY_CURRENT_USER, key.c_str(), "LastCheckTime", REG_SZ, zero, sizeof(zero));
            automatic(1); pump(5500);
            require(!visible_prompt(), "scheduled fallback result remains deferred during fullscreen");
            automatic(0); // app chose silent updates / installation started
            SendMessageW(other, WM_APP+1, 0, 0);
            for (int i=0;i<50;++i) { pump(100); if (visible_prompt()) throw std::runtime_error("cancelled scheduled response displayed"); }
            require(!visible_prompt(), "disabled fallback discards in-flight scheduled UI after fullscreen exits");
        }
        if (!newer && !permission) {
            for (int i=0;i<15;++i) {
                check();
                for (int j=0;j<600;++j) { pump(100); if (visible_prompt()) throw std::runtime_error("installed target build prompted again"); }
                std::printf("MINUTE %d no repeated update prompt\n", i+1); std::fflush(stdout);
            }
            screenshot(L"installed-after-15min.bmp");
        }
        cleanup(); stop_owned_child();
        RegDeleteTreeA(HKEY_CURRENT_USER, key.c_str());
        std::puts("PASS real Windows updater component UI scenario; no installer or VPN was run");
        return 0;
    } catch (const std::exception& error) {
        std::fprintf(stderr,"FAIL %s\n",error.what());
        try { screenshot(L"failure-desktop.bmp"); } catch (...) {}
        close_prompt();
        if (sparkle) {
            auto cleanup = reinterpret_cast<Void>(GetProcAddress(sparkle, "win_sparkle_cleanup"));
            if (cleanup) cleanup();
        }
        stop_owned_child();
        return 1;
    }
}
