// Drives the installed client's EXISTING WinSparkle UI, never an alternative updater.
#include "update_fullscreen.h"
#include <tlhelp32.h>
#include <cstdio>
#include <cstdlib>
#include <cerrno>
#include <ctime>
#include <cwchar>
#include <string>
#include <vector>
#include <stdexcept>
#pragma comment(lib,"user32.lib")
#pragma comment(lib,"advapi32.lib")
#pragma comment(lib,"version.lib")

static PROCESS_INFORMATION other = {}, app = {};
static std::wstring capture_helper;
static void ensure(bool value, const char* text) {
    if(!value) throw std::runtime_error(text);
    std::printf("PASS %s\n", text); std::fflush(stdout);
}
static PROCESS_INFORMATION launch(const std::wstring& path, const std::wstring& args) {
    STARTUPINFOW startup={sizeof(startup)}; PROCESS_INFORMATION process={};
    std::wstring command=L"\""+path+L"\" "+args;
    ensure(CreateProcessW(path.c_str(), command.data(),nullptr,nullptr,FALSE,0,nullptr,nullptr,&startup,&process)!=FALSE,"real process launched");
    CloseHandle(process.hThread); return process;
}
struct WindowSearch { DWORD pid; bool update_only; HWND found; };
static BOOL CALLBACK window_callback(HWND window, LPARAM param) {
    auto& search=*reinterpret_cast<WindowSearch*>(param);
    DWORD pid=0; GetWindowThreadProcessId(window,&pid);
    if(pid!=search.pid || !IsWindowVisible(window)) return TRUE;
    RECT frame={};
    if(!GetWindowRect(window,&frame) || frame.right<=frame.left || frame.bottom<=frame.top) return TRUE;
    wchar_t name[256]; GetClassNameW(window,name,256);
    // Native wxDialogs use the standard #32770 dialog class on Windows.
    // Zero-area PseudoConsoleWindow infrastructure is not an application UI.
    if(!search.update_only || wcsncmp(name,L"wx",2)==0 || wcscmp(name,L"#32770")==0) search.found=window;
    return TRUE;
}
static HWND visible(DWORD pid,bool update_only=true) {
    WindowSearch search={pid,update_only,nullptr}; EnumWindows(window_callback,reinterpret_cast<LPARAM>(&search));return search.found;
}
static ULONGLONG filetime_value(FILETIME value) {
    return (static_cast<ULONGLONG>(value.dwHighDateTime)<<32)|value.dwLowDateTime;
}
static ULONGLONG process_creation(HANDLE process) {
    FILETIME created={},exited={},kernel={},user={};
    return GetProcessTimes(process,&created,&exited,&kernel,&user)?filetime_value(created):0;
}
static std::wstring normalized_image_path(const std::wstring& path) {
    std::wstring native=path;
    for(auto& character:native)if(character==L'/')character=L'\\';
    DWORD size=GetFullPathNameW(native.c_str(),0,nullptr,nullptr);
    if(!size || size>32768)return {};
    std::vector<wchar_t> full(size);
    DWORD written=GetFullPathNameW(native.c_str(),size,full.data(),nullptr);
    if(!written || written>=size)return {};
    return std::wstring(full.data(),written);
}
static void role(const char* name, DWORD pid) {
    auto process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,pid);
    ensure(process!=nullptr,"original client process identity handle opened");
    auto created=process_creation(process);wchar_t image[32768]={};DWORD size=32768;
    bool queried=QueryFullProcessImageNameW(process,0,image,&size)!=FALSE;
    CloseHandle(process);
    auto full=queried?normalized_image_path(image):std::wstring{};
    ensure(created!=0 && !full.empty(),"original client start time and absolute image path captured");
    FILE* identity=nullptr;fopen_s(&identity,"client-image-identities.txt","a");
    if(identity){std::fwprintf(identity,L"pid=%lu start_filetime=%llu image=%ls\n",pid,created,full.c_str());fclose(identity);}
    FILE* file=nullptr;fopen_s(&file,"client-processes.jsonl","a");
    if(file){std::fprintf(file,"{\"stage\":\"%s\",\"pid\":%lu,\"tick\":%llu,\"start_filetime\":%llu}\n",name,pid,GetTickCount64(),created);fclose(file);}
}
static void sample(const char* phase,DWORD old_pid,DWORD new_pid,HWND expected_fullscreen=nullptr) {
    DWORD foreground_pid=0;auto foreground=GetForegroundWindow();GetWindowThreadProcessId(foreground,&foreground_pid);
    auto old_update=visible(old_pid),new_update=new_pid?visible(new_pid):nullptr;
    auto new_window=new_pid?visible(new_pid,false):nullptr;
    FILE* file=nullptr;fopen_s(&file,"window-samples.csv","a");
    wchar_t class_name[256]={};GetClassNameW(foreground,class_name,256);
    char class_utf8[768]={};WideCharToMultiByte(CP_UTF8,0,class_name,-1,class_utf8,sizeof(class_utf8),nullptr,nullptr);
    RECT frame={};GetWindowRect(foreground,&frame);
    if(file){std::fprintf(file,"%s,%llu,%lu,%lu,%lu,%p,%p,%p,%p,%s,%d,%ld,%ld,%ld,%ld\n",phase,GetTickCount64(),foreground_pid,old_pid,new_pid,old_update,new_update,new_window,foreground,class_utf8,IsWindowVisible(foreground),frame.left,frame.top,frame.right,frame.bottom);fclose(file);}
    if(expected_fullscreen && foreground!=expected_fullscreen) throw std::runtime_error(std::string("foreground changed during ")+phase);
    if(expected_fullscreen && new_window) throw std::runtime_error("new B window appeared during fullscreen installation transition");
}
static HWND install_button=nullptr;
static BOOL CALLBACK button_callback(HWND window,LPARAM) {
    wchar_t text[256],name[256];GetWindowTextW(window,text,256);GetClassNameW(window,name,256);
    if(_wcsicmp(name,L"Button")==0 && wcsstr(text,L"Install update") && IsWindowEnabled(window) && IsWindowVisible(window)) install_button=window;
    return TRUE;
}
// Query the EXISTING primary-instance pipe. Close without sending any bytes:
// instance.rs ignores an empty message, so this does not activate/show/quit.
static DWORD primary_instance_pid() {
    wchar_t unexpected[2]={};
    if(GetEnvironmentVariableW(L"PPVPN_FLUTTER_HOME",unexpected,2))return 0;
    DWORD size=GetEnvironmentVariableW(L"LOCALAPPDATA",nullptr,0);
    if(!size || size>32768)return 0;
    std::vector<wchar_t> base(size);
    DWORD length=GetEnvironmentVariableW(L"LOCALAPPDATA",base.data(),size);
    if(!length || length>=size)return 0;
    std::wstring data(base.data(),length);data+=L"\\PPVPN";
    for(auto& character:data)if(character==L'\\')character=L'/';
    const auto name=std::wstring(L"\\\\.\\pipe\\ppvpn-")+data;
    auto pipe=CreateFileW(name.c_str(),GENERIC_WRITE,0,nullptr,OPEN_EXISTING,0,nullptr);
    if(pipe==INVALID_HANDLE_VALUE)return 0;
    ULONG owner=0;bool queried=GetNamedPipeServerProcessId(pipe,&owner)!=FALSE;
    CloseHandle(pipe);return queried?owner:0;
}
static void scheduler_snapshot(const char* stage,const char* expected_last=nullptr) {
    const char* key="Software\\PPVPN\\WinSparkle";
    char last[64]={},interval[64]={},machine_interval[64]={},enabled[64]={};
    DWORD last_size=sizeof(last),interval_size=sizeof(interval),machine_interval_size=sizeof(machine_interval),enabled_size=sizeof(enabled);
    const auto last_status=RegGetValueA(HKEY_CURRENT_USER,key,"LastCheckTime",RRF_RT_REG_SZ,nullptr,last,&last_size);
    const auto interval_status=RegGetValueA(HKEY_CURRENT_USER,key,"UpdateInterval",RRF_RT_REG_SZ,nullptr,interval,&interval_size);
    const auto machine_interval_status=RegGetValueA(HKEY_LOCAL_MACHINE,key,"UpdateInterval",RRF_RT_REG_SZ,nullptr,machine_interval,&machine_interval_size);
    const auto enabled_status=RegGetValueA(HKEY_CURRENT_USER,key,"CheckForUpdates",RRF_RT_REG_SZ,nullptr,enabled,&enabled_size);
    ensure(last_status==ERROR_SUCCESS && enabled_status==ERROR_SUCCESS,"owned scheduler last-check and automatic-check registry state captured");
    long effective_interval=86400;const char* interval_source="pinned_0.9.2_default_both_registry_values_missing";
    const char* selected_interval=nullptr;
    if(interval_status==ERROR_SUCCESS){selected_interval=interval;interval_source="actual_HKCU_registry_value";}
    else{
        ensure(interval_status==ERROR_FILE_NOT_FOUND,"user interval lookup is known missing");
        if(machine_interval_status==ERROR_SUCCESS){selected_interval=machine_interval;interval_source="actual_HKLM_registry_value";}
        else{ensure(machine_interval_status==ERROR_FILE_NOT_FOUND || machine_interval_status==ERROR_PATH_NOT_FOUND,"machine interval lookup is known missing; exact pinned default applies");}
    }
    if(selected_interval){
        char* end=nullptr;errno=0;effective_interval=std::strtol(selected_interval,&end,10);
        ensure(errno==0 && effective_interval>0 && end && *end==0 && std::to_string(effective_interval)==selected_interval,"configured effective scheduler interval is known");
    }
    if(expected_last)ensure(std::string(last)==expected_last,"B scheduled-check reset read back as zero");
    FILE* file=nullptr;const auto path=std::string(stage)+"-scheduler-state.txt";fopen_s(&file,path.c_str(),"w");
    ensure(file!=nullptr,"owned scheduler evidence file opened");
    std::fprintf(file,"last_status=%ld\nLastCheckTime=%s\ninterval_status=%ld\nconfigured_HKCU_UpdateInterval=%s\nmachine_interval_status=%ld\nconfigured_HKLM_UpdateInterval=%s\neffective_interval=%ld\ninterval_source=%s\nautomatic_status=%ld\nCheckForUpdates=%s\nobserved_app_pid=%lu\nobserver_pid=%lu\nobserved_unix_seconds=%lld\nobserved_tick=%llu\n",last_status,last,interval_status,interval,machine_interval_status,machine_interval,effective_interval,interval_source,enabled_status,enabled,app.dwProcessId,GetCurrentProcessId(),static_cast<long long>(std::time(nullptr)),GetTickCount64());fclose(file);
}
static DWORD find_app(const std::wstring& path,DWORD exclude=0,ULONGLONG created_after=0,ULONGLONG* created_out=nullptr) {
    const DWORD primary=created_after?primary_instance_pid():0;
    if(created_after && !primary)return 0;
    auto expected=normalized_image_path(path);
    if(expected.empty())return 0;
    auto snapshot=CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS,0);PROCESSENTRY32W entry={sizeof(entry)};DWORD found=0;
    if(Process32FirstW(snapshot,&entry)) do {
        if(entry.th32ProcessID==exclude || (created_after && entry.th32ProcessID!=primary) || _wcsicmp(entry.szExeFile,L"ppvpn.exe")) continue;
        auto process=OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION,FALSE,entry.th32ProcessID);
        if(process) {wchar_t name[32768];DWORD size=32768;
            if(QueryFullProcessImageNameW(process,0,name,&size)){
                auto actual=normalized_image_path(name);auto created=process_creation(process);
                if(!actual.empty() && _wcsicmp(actual.c_str(),expected.c_str())==0 && created && created>=created_after){
                    found=entry.th32ProcessID;if(created_out)*created_out=created;
                }
            }
            CloseHandle(process);
        }
    }while(Process32NextW(snapshot,&entry));
    CloseHandle(snapshot);return found;
}
static std::wstring file_version(const wchar_t* path) {
    DWORD ignored=0,size=GetFileVersionInfoSizeW(path,&ignored);std::vector<unsigned char> data(size);
    ensure(size && GetFileVersionInfoW(path,0,size,data.data()),"installed PE version readable");
    VS_FIXEDFILEINFO* value=nullptr;UINT length=0;
    ensure(VerQueryValueW(data.data(),L"\\",reinterpret_cast<void**>(&value),&length)!=FALSE,"installed PE fixed version readable");
    return std::to_wstring(HIWORD(value->dwFileVersionMS))+L"."+std::to_wstring(LOWORD(value->dwFileVersionMS))+L"."+std::to_wstring(HIWORD(value->dwFileVersionLS))+L"."+std::to_wstring(LOWORD(value->dwFileVersionLS));
}
static void quit_app(DWORD pid) {
    auto event=OpenEventW(EVENT_MODIFY_STATE,FALSE,L"Local\\PPVPN.Desktop.Quit");ensure(event!=nullptr,"existing installer quit event available");
    SetEvent(event);CloseHandle(event);
    auto process=OpenProcess(SYNCHRONIZE,FALSE,pid);ensure(process!=nullptr,"client exit handle opened");
    bool exited=WaitForSingleObject(process,15000)==WAIT_OBJECT_0;CloseHandle(process);ensure(exited,"client exited through existing quit event");
}
static void close_other() {
    if(!other.hProcess)return;
    if(auto window=FindWindowW(L"PPVPNOtherAppQA",nullptr)) PostMessageW(window,WM_CLOSE,0,0);
    if(WaitForSingleObject(other.hProcess,5000)==WAIT_TIMEOUT)TerminateProcess(other.hProcess,1);
    CloseHandle(other.hProcess);other.hProcess=nullptr;
}
static void snapshot(const wchar_t* file) {
    std::wstring image(file);
    auto at=image.rfind(L".txt");if(at!=std::wstring::npos)image.replace(at,4,L".bmp");
    auto capture=launch(capture_helper,L"--capture \""+image+L"\"");
    ensure(WaitForSingleObject(capture.hProcess,10000)==WAIT_OBJECT_0,"actual desktop capture finished");
    DWORD code=1;GetExitCodeProcess(capture.hProcess,&code);CloseHandle(capture.hProcess);
    ensure(code==0,"actual desktop capture succeeded");
    FILE* output;_wfopen_s(&output,file,L"w");
    if(output){std::fwprintf(output,L"foreground=%p installed_pid=%lu utc_tick=%llu\n",GetForegroundWindow(),app.dwProcessId,GetTickCount64());fclose(output);}
}
int wmain(int argc,wchar_t** argv) {
    SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2);
    try {
        const bool startup_only=argc==6 && wcscmp(argv[5],L"--startup-only")==0;
        ensure(argc==5 || startup_only,"arguments: installed client, existing helper, A version, B version [--startup-only]");
        capture_helper=argv[2];
        DWORD session=0;ProcessIdToSessionId(GetCurrentProcessId(),&session);
        ensure(session>0 && session==WTSGetActiveConsoleSessionId(),"actual active console session");
        ensure(file_version(argv[1])==argv[3],"installed A build matches fixture");
        ensure(!find_app(argv[1]),"no client already running on disposable test host");
        other=launch(argv[2],L"--other-app");
        HWND foreground=nullptr;for(int i=0;i<100 && !foreground;++i){Sleep(50);foreground=FindWindowW(L"PPVPNOtherAppQA",nullptr);}
        ensure(foreground!=nullptr,"independent foreground app created");AllowSetForegroundWindow(other.dwProcessId);
        SendMessageW(foreground,WM_APP+1,1,0);Sleep(500);
        ensure(GetForegroundWindow()==foreground && ppvpn_update::foreground_fullscreen(),"other app actually fullscreen before client scheduled check");
        if(startup_only){
            wchar_t timing[MAX_PATH]={};GetFullPathNameW(L"startup-timing.txt",MAX_PATH,timing,nullptr);
            SetEnvironmentVariableW(L"PPVPN_FLUTTER_TIMING",timing);
            FILETIME time={};GetSystemTimeAsFileTime(&time);ULARGE_INTEGER value={};value.LowPart=time.dwLowDateTime;value.HighPart=time.dwHighDateTime;
            auto epoch=std::to_wstring(value.QuadPart/10000ULL-11644473600000ULL);
            SetEnvironmentVariableW(L"PPVPN_FLUTTER_T0_MS",epoch.c_str());
        }
        app=launch(argv[1],L"--background");
        DWORD old_pid=app.dwProcessId;role("A",old_pid);
        const auto full_start=GetTickCount64();ULONGLONG response_tick=0;
        if(startup_only){
            std::printf("DIAGNOSTIC startup-only actual client pid=%lu launch_argument=--background tick=%llu; no appcast/install acceptance\n",old_pid,full_start);std::fflush(stdout);
            bool interrupted=false,captured=false;
            const bool restore_once=GetEnvironmentVariableW(L"PPVPN_QA_RESTORE_GAME_ONCE",nullptr,0)>0;
            while(GetTickCount64()<full_start+12000){
                sample("A-startup-diagnostic",old_pid,0);
                if(GetForegroundWindow()!=foreground){
                    interrupted=true;
                    if(!captured){
                        captured=true;
                        if(restore_once){
                            // Diagnostic only: one NEW fixture user intention,
                            // to distinguish later layout activation from the initial native focus.
                            std::printf("DIAGNOSTIC restoring actual other-app fullscreen once tick=%llu\n",GetTickCount64());std::fflush(stdout);
                            SendMessageW(foreground,WM_APP+1,1,0);
                            sample("A-startup-after-one-game-restore",old_pid,0);
                            ensure(GetForegroundWindow()==foreground,"one actual fixture game restore completed");
                        }else snapshot(L"startup-focus-change.txt");
                    }
                }
                Sleep(20);
            }
            snapshot(L"startup-final.txt");
            SendMessageW(foreground,WM_APP+1,0,0);Sleep(300);
            auto explicit_open=launch(argv[1],L"");
            ensure(WaitForSingleObject(explicit_open.hProcess,15000)==WAIT_OBJECT_0,"explicit user open reused existing app process");
            CloseHandle(explicit_open.hProcess);
            bool user_front=false;
            for(int i=0;i<150 && !user_front;++i){
                DWORD front_pid=0;GetWindowThreadProcessId(GetForegroundWindow(),&front_pid);
                user_front=front_pid==old_pid && visible(old_pid,false)!=nullptr;
                sample("A-explicit-user-open",old_pid,0);Sleep(100);
            }
            snapshot(L"explicit-user-open.txt");
            ensure(user_front,"original client visible and foreground after explicit user open");
            auto front=GetForegroundWindow();DWORD thread=GetWindowThreadProcessId(front,nullptr);
            GUITHREADINFO input={sizeof(input)};
            ensure(GetGUIThreadInfo(thread,&input)!=FALSE && input.hwndFocus!=nullptr &&
                   GetAncestor(input.hwndFocus,GA_ROOT)==front && IsWindowEnabled(input.hwndFocus),
                   "original client keyboard focus enabled after explicit user open");
            quit_app(old_pid);close_other();
            ensure(!interrupted,"background startup preserved independent fullscreen foreground throughout twelve seconds");
            return 0;
        }
        while(GetTickCount64()<full_start+30000){
            sample("A-initial-fullscreen",old_pid,0,foreground);
            if(visible(old_pid))throw std::runtime_error("client A prompted during fullscreen");
            FILE* marker=nullptr;fopen_s(&marker,"appcast-A-written.txt","r");
            if(marker){unsigned long response_pid=0;unsigned long long tick=0;
                if(fscanf_s(marker,"%lu %llu",&response_pid,&tick)==2 && response_pid==old_pid && tick>=full_start)response_tick=tick;
                fclose(marker);
            }
            if(response_tick && GetTickCount64()>=full_start+7000 && GetTickCount64()>=response_tick+2000)break;
            Sleep(100);
        }
        ensure(response_tick!=0,"actual A appcast response completed during fullscreen observation");
        sample("A-initial-fullscreen",old_pid,0,foreground);
        snapshot(L"client-A-fullscreen-state.txt");
        SendMessageW(foreground,WM_APP+1,0,0);Sleep(500);
        ensure(!ppvpn_update::foreground_fullscreen(),"other app returned to ordinary maximization");
        HWND update=nullptr;for(int i=0;i<150 && !update;++i){Sleep(100);update=visible(old_pid);}
        ensure(update!=nullptr,"existing client WinSparkle update dialog shown after fullscreen");
        EnumChildWindows(update,button_callback,0);ensure(install_button!=nullptr,"original Install update button found");
        const auto download_button=install_button;
        PostMessageW(download_button,BM_CLICK,0,0);
        // The standard scheduled-update UI has two original user actions:
        // download, then the distinct Ready-to-install button. Do not bypass
        // either by launching the installer directly or changing updater mode.
        const auto download_deadline=GetTickCount64()+180000;
        HWND ready_button=nullptr;
        while(GetTickCount64()<download_deadline && !ready_button){
            sample("A-downloading",old_pid,0);
            install_button=nullptr;
            if(auto dialog=visible(old_pid))EnumChildWindows(dialog,button_callback,0);
            if(install_button && install_button!=download_button)ready_button=install_button;
            Sleep(50);
        }
        ensure(ready_button!=nullptr,"original Ready-to-install button found after actual download");
        // The actual A appcast and signed download are complete, but the
        // original final Install action has NOT run: B cannot launch yet.
        // Exercise B's existing automatic checker as actually due. A just
        // wrote the shared LastCheckTime; keep the required B feed assertion.
        const char* check_key="Software\\PPVPN\\WinSparkle";
        scheduler_snapshot("B-before-due-reset");
        const char due_check[]="0";
        ensure(RegSetKeyValueA(HKEY_CURRENT_USER,check_key,"LastCheckTime",REG_SZ,due_check,sizeof(due_check))==ERROR_SUCCESS,
               "original B automatic check scheduled due before normal installer relaunch");
        FILETIME reset_at={};GetSystemTimeAsFileTime(&reset_at);
        const auto b_check_reset=filetime_value(reset_at);
        scheduler_snapshot("B-after-due-reset","0");
        FILE* schedule=nullptr;fopen_s(&schedule,"B-check-reset-order.txt","w");
        if(schedule){std::fprintf(schedule,"reset_filetime=%llu\nreset_tick=%llu\n",b_check_reset,GetTickCount64());fclose(schedule);}
        FILETIME requested={};GetSystemTimeAsFileTime(&requested);
        const auto install_requested=filetime_value(requested);
        FILE* install_order=nullptr;fopen_s(&install_order,"B-check-reset-order.txt","a");
        if(install_order){std::fprintf(install_order,"install_request_filetime=%llu\nA_pid=%lu\n",install_requested,old_pid);fclose(install_order);}
        PostMessageW(ready_button,BM_CLICK,0,0);
        SendMessageW(foreground,WM_APP+1,1,0);Sleep(300);
        ensure(GetForegroundWindow()==foreground,"other app foreground during real installation");
        const auto exit_deadline=GetTickCount64()+180000;
        while(WaitForSingleObject(app.hProcess,0)==WAIT_TIMEOUT && GetTickCount64()<exit_deadline){
            sample("A-installing",old_pid,0,foreground);Sleep(50);
        }
        ensure(WaitForSingleObject(app.hProcess,0)==WAIT_OBJECT_0,"A exited for the real installer");
        CloseHandle(app.hProcess);app.hProcess=nullptr;
        DWORD next=0;ULONGLONG next_created=0;
        for(int i=0;i<3600 && !next;++i){
            next=find_app(argv[1],old_pid,install_requested,&next_created);sample("A-to-B-relaunch",old_pid,next,foreground);Sleep(50);
        }
        ensure(next && next!=old_pid,"installer actually reopened a distinct B client process");
        ensure(next_created>=b_check_reset,"actual B process created after its scheduled-check reset");
        ensure(file_version(argv[1])==argv[4],"installed file actually replaced by target B build");
        app.hProcess=OpenProcess(SYNCHRONIZE|PROCESS_QUERY_LIMITED_INFORMATION,FALSE,next);
        ensure(app.hProcess!=nullptr && next_created!=0 && process_creation(app.hProcess)==next_created,
               "primary-instance B process handle and exact creation identity retained");
        app.dwProcessId=next;role("B",next);
        FILE* primary_record=nullptr;fopen_s(&primary_record,"B-primary-instance.txt","w");
        if(primary_record){std::fprintf(primary_record,"pid=%lu start_filetime=%llu source=existing_primary_pipe_no_payload\n",next,next_created);fclose(primary_record);}
        std::printf("TRANSITION old_pid=%lu new_pid=%lu\n",old_pid,next);std::fflush(stdout);
        for(int i=0;i<50;++i){sample("B-fullscreen",old_pid,next,foreground);Sleep(100);}
        snapshot(L"client-B-fullscreen-state.txt");
        scheduler_snapshot("B-after-native-startup");
        SendMessageW(foreground,WM_APP+1,0,0);Sleep(500);
        for(int i=0;i<100 && !visible(next,false);++i)Sleep(100);
        ensure(visible(next,false)!=nullptr,"original client window resumes after fullscreen exits");
        const ULONGLONG started=GetTickCount64();
        for(int minute=1;minute<=15;++minute){
            auto until=started+static_cast<ULONGLONG>(minute)*60000;
            while(GetTickCount64()<until){if(WaitForSingleObject(app.hProcess,0)!=WAIT_TIMEOUT)throw std::runtime_error("original B process exited during observation");sample("B-15min",old_pid,next);if(visible(next))throw std::runtime_error("B repeated an update-available dialog");Sleep(100);}
            std::printf("REAL_MINUTE %d B PID=%lu no update-available dialog\n",minute,next);std::fflush(stdout);
        }
        snapshot(L"client-B-after-15min-state.txt");quit_app(next);CloseHandle(app.hProcess);app.hProcess=nullptr;
        const char zero[]="0";RegSetKeyValueA(HKEY_CURRENT_USER,"Software\\PPVPN\\WinSparkle","LastCheckTime",REG_SZ,zero,sizeof(zero));
        app=launch(argv[1],L"--background");role("restarted-B",app.dwProcessId);ensure(app.dwProcessId!=next,"actual B process restarted");
        for(int i=0;i<150;++i){sample("B-restarted",0,app.dwProcessId);Sleep(100);if(visible(app.dwProcessId))throw std::runtime_error("B repeated update prompt after real process restart");}
        ensure(file_version(argv[1])==argv[4],"B remains installed after next startup check");
        snapshot(L"client-B-restart-state.txt");quit_app(app.dwProcessId);CloseHandle(app.hProcess);app.hProcess=nullptr;
        close_other();std::puts("PASS actual existing client A-to-B WinSparkle/NSIS fallback; not silent-service installation coverage");return 0;
    }catch(const std::exception& error){
        std::fprintf(stderr,"FAIL %s\n",error.what());
        try { if(!capture_helper.empty())snapshot(L"failure-desktop-state.txt"); } catch(...) {}
        close_other();
        if(app.hProcess)CloseHandle(app.hProcess);
        return 1;
    }
}
