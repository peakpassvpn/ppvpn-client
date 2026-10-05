; PPVPN for Windows — NSIS installer (per-machine, x64, zh-CN + en-US).
;
; Built by apps/windows/scripts/package.ps1, which stages the files and
; passes /DDEFINES=<generated defines.nsh> with:
;   VERSION        user-visible version (0.3.0)
;   BUILD_VERSION  four-part file version (0.3.0.12)
;   STAGE_DIR      directory whose contents are installed into $INSTDIR
;   OUT_FILE       installer path
;   SIGN_COMMAND   optional; Authenticode command with %1 for the file
;   FAST_COMPRESSION optional; zlib (non-solid) instead of solid LZMA, for dev loops
;
; Layout: everything in one directory, because ppvpn-service only accepts
; clients named ppvpn.exe that run from the service's own directory, and it
; starts ppvpn-core.exe from that directory too:
;   $PROGRAMFILES64\PPVPN\ppvpn.exe (+ .NET/Windows App SDK runtime, WinSparkle.dll)
;   ppvpn-push-agent.exe (NativeAOT; shares runtimes\win-x64\native\ppvpn_client.dll)
;   ppvpn-core.exe, ppvpn-service.exe, ppvpn-service-install.exe, ppvpn-service-uninstall.exe
;   install-files.txt (what this version installed; used to clean up on upgrade)
;   uninstall.exe
;
; Command line:
;   /S                silent (WinSparkle runs updates with /S); relaunches the
;                     app afterwards when it was running in this session
;                     (the push agent is relaunched whenever its Run value exists)
;   /DESKTOPSHORTCUT  (install) also create a desktop shortcut in silent mode
;   /PURGE            (uninstall) also remove per-user data in silent mode

Unicode true
ManifestDPIAware true
ManifestSupportedOS Win10
RequestExecutionLevel admin

!ifdef DEFINES
  !include "${DEFINES}"
!endif
; Release builds: solid LZMA (smallest). package.ps1 -FastCompression: zlib, several times faster.
!ifdef FAST_COMPRESSION
  SetCompressor zlib
!else
  SetCompressor /SOLID lzma
!endif
!macro REQUIRE_DEFINE NAME
  !ifndef ${NAME}
    !error "${NAME} is not defined; build with apps/windows/scripts/package.ps1"
  !endif
!macroend
!insertmacro REQUIRE_DEFINE VERSION
!insertmacro REQUIRE_DEFINE BUILD_VERSION
!insertmacro REQUIRE_DEFINE STAGE_DIR
!insertmacro REQUIRE_DEFINE OUT_FILE

!ifdef SIGN_COMMAND
  ; Optional Authenticode signing (scripts/sign-windows.ps1 or WINDOWS_SIGN_COMMAND).
  !finalize `${SIGN_COMMAND}`
  !uninstfinalize `${SIGN_COMMAND}`
!endif

!define APP_NAME "PPVPN"
!define PUBLISHER "PeakPass VPN LLC"
!define APP_EXE "ppvpn.exe"
!define CORE_EXE "ppvpn-core.exe"
; PPVPN.PushAgent (Program.cs) and PPVPN.Windows/Platform/PushAgentAutostart.cs
!define AGENT_EXE "ppvpn-push-agent.exe"
!define AGENT_QUIT_EVENT "Local\PPVPN.PushAgent.Quit"
!define AGENT_RUN_VALUE "PPVPNPushAgent"
!define SERVICE_NAME "ppvpn_service"
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PPVPN"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"
; Platform/LaunchAtLogin.cs
!define RUN_VALUE "PPVPN"
; Platform/AppUpdater.cs (QuitSignal)
!define QUIT_EVENT "Local\PPVPN.Desktop.Quit"
; Platform/CredentialStore.cs
!define CREDENTIAL_TARGET "PPVPN/desktop.credentials"
!define FAKE_CREDENTIAL_TARGET "PPVPN/desktop.credentials.fake"
; Platform/AppNotifications.cs (process AppUserModelID; also on the shortcuts)
!define AUMID "PeakPass.PPVPN"
!define TOAST_ACTIVATOR_CLSID "{FCD3C3FA-FCA6-4F2D-BC9E-BEA74D70EBBE}"
!define MANIFEST "install-files.txt"

Name "${APP_NAME}"
OutFile "${OUT_FILE}"
; Fixed: the install directory must stay admin-write-only because the
; privileged service trusts executables from it.
InstallDir "$PROGRAMFILES64\PPVPN"
BrandingText "${PUBLISHER}"
ShowInstDetails show
ShowUninstDetails show

!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "TextFunc.nsh"
!include "x64.nsh"
!include "WinVer.nsh"
!include "nsDialogs.nsh"

Var AppWasRunning     ; 1 when a ppvpn.exe in this session accepted the quit signal
Var CreateDesktop     ; 1 to create the desktop shortcut
Var DesktopCheckbox
Var RemoveData        ; uninstall: 1 to remove per-user data
Var DataCheckbox

; Relative paths resolve against this directory (makensis changes into it).
!define MUI_ICON "..\PPVPN.Windows\Assets\AppIcon.ico"
!define MUI_UNICON "..\PPVPN.Windows\Assets\AppIcon.ico"
!define MUI_ABORTWARNING
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_FUNCTION LaunchApp

!insertmacro MUI_PAGE_WELCOME
; GPL-3.0-or-later (LICENSE.txt, copied by package.ps1 from the repository's LICENSE). The GPL
; asks for no acceptance, so the page only informs: Next instead of "I Agree".
!define MUI_LICENSEPAGE_BUTTON "$(^NextBtn)"
!define MUI_LICENSEPAGE_TEXT_BOTTOM "$(LicenseBottom)"
!insertmacro MUI_PAGE_LICENSE "${STAGE_DIR}\LICENSE.txt"
Page custom TasksPage TasksPageLeave
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH

UninstPage custom un.DataPage un.DataPageLeave
!insertmacro MUI_UNPAGE_INSTFILES

; The first language is the fallback; NSIS picks the one matching the
; Windows UI language.
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"
!include "strings.nsh"
; The SimpChinese default (NSimSun) looks dated next to the app; use the
; Windows UI font for Chinese.
SetFont /LANG=${LANG_SIMPCHINESE} "Microsoft YaHei UI" 9

VIProductVersion "${BUILD_VERSION}"
VIFileVersion "${BUILD_VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductName" "${APP_NAME}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "CompanyName" "${PUBLISHER}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "LegalCopyright" "© ${PUBLISHER}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileDescription" "${APP_NAME} Setup"
VIAddVersionKey /LANG=${LANG_ENGLISH} "FileVersion" "${BUILD_VERSION}"
VIAddVersionKey /LANG=${LANG_ENGLISH} "ProductVersion" "${VERSION}"

; ───────────────────────────── shared helpers ─────────────────────────────

; Ask ppvpn.exe to quit through its normal path (backend Shutdown), wait for
; it, then terminate whatever is left (other sessions).
!macro DEFINE_STOP_APP UN
Function ${UN}StopApp
  StrCpy $AppWasRunning 0
  System::Call 'kernel32::OpenEventW(i 0x0002, i 0, w "${QUIT_EVENT}") p .r0'
  ${If} $0 P<> 0
    StrCpy $AppWasRunning 1
    DetailPrint "$(StoppingApp)"
    System::Call 'kernel32::SetEvent(p r0)'
    System::Call 'kernel32::CloseHandle(p r0)'
    ; Shutdown can take up to ~10 s (it disconnects enhanced mode).
    StrCpy $1 0
    ${Do}
      nsExec::Exec 'cmd.exe /d /c tasklist /nh /fi "imagename eq ${APP_EXE}" | find /i "${APP_EXE}"'
      Pop $0
      ${IfThen} $0 != 0 ${|} ${Break} ${|}
      IntOp $1 $1 + 1
      ${IfThen} $1 > 40 ${|} ${Break} ${|}
      Sleep 500
    ${Loop}
  ${EndIf}
  nsExec::Exec 'cmd.exe /d /c tasklist /nh /fi "imagename eq ${APP_EXE}" | find /i "${APP_EXE}"'
  Pop $0
  ${If} $0 == 0
    DetailPrint "$(TerminatingApp)"
    nsExec::Exec 'taskkill.exe /f /t /im ${APP_EXE}'
    Pop $0
    Sleep 1000
  ${EndIf}
FunctionEnd
!macroend
!insertmacro DEFINE_STOP_APP ""
!insertmacro DEFINE_STOP_APP "un."

; The push agent (ppvpn-push-agent.exe) holds ppvpn_client.dll open: ask it to
; quit through its event (it stops its long poll and exits within a second or
; two), wait up to 10 s, then terminate what is left (other sessions included).
!macro DEFINE_STOP_AGENT UN
Function ${UN}StopAgent
  System::Call 'kernel32::OpenEventW(i 0x0002, i 0, w "${AGENT_QUIT_EVENT}") p .r0'
  ${If} $0 P<> 0
    DetailPrint "$(StoppingAgent)"
    System::Call 'kernel32::SetEvent(p r0)'
    System::Call 'kernel32::CloseHandle(p r0)'
    StrCpy $1 0
    ${Do}
      nsExec::Exec 'cmd.exe /d /c tasklist /nh /fi "imagename eq ${AGENT_EXE}" | find /i "${AGENT_EXE}"'
      Pop $0
      ${IfThen} $0 != 0 ${|} ${Break} ${|}
      IntOp $1 $1 + 1
      ${IfThen} $1 > 20 ${|} ${Break} ${|}
      Sleep 500
    ${Loop}
  ${EndIf}
  nsExec::Exec 'cmd.exe /d /c tasklist /nh /fi "imagename eq ${AGENT_EXE}" | find /i "${AGENT_EXE}"'
  Pop $0
  ${If} $0 == 0
    nsExec::Exec 'taskkill.exe /f /im ${AGENT_EXE}'
    Pop $0
    Sleep 500
  ${EndIf}
FunctionEnd
!macroend
!insertmacro DEFINE_STOP_AGENT ""
!insertmacro DEFINE_STOP_AGENT "un."

; Delete what the previous version listed in install-files.txt. Lines are
; paths relative to $INSTDIR; lines ending in "\" are directories, listed
; deepest first after the files, and only removed when empty.
!macro DEFINE_REMOVE_LISTED UN
Function ${UN}RemoveListedFiles
  ClearErrors
  FileOpen $0 "$INSTDIR\${MANIFEST}" r
  ${IfThen} ${Errors} ${|} Return ${|}
  ${Do}
    ClearErrors
    FileRead $0 $1
    ${IfThen} ${Errors} ${|} ${Break} ${|}
    ${TrimNewLines} "$1" $1
    ${If} $1 != ""
      StrCpy $2 $1 1 -1
      ${If} $2 == "\"
        RMDir "$INSTDIR\$1"
      ${Else}
        Delete "$INSTDIR\$1"
      ${EndIf}
    ${EndIf}
  ${Loop}
  FileClose $0
  Delete "$INSTDIR\${MANIFEST}"
FunctionEnd
!macroend
!insertmacro DEFINE_REMOVE_LISTED ""
!insertmacro DEFINE_REMOVE_LISTED "un."

; Wait until the service reports STOPPED (sc prints the state names in
; English on every UI language).
!macro DEFINE_STOP_SERVICE UN
Function ${UN}StopService
  nsExec::Exec 'sc.exe query ${SERVICE_NAME}'
  Pop $0
  ${IfThen} $0 != 0 ${|} Return ${|}
  DetailPrint "$(StoppingService)"
  nsExec::Exec 'sc.exe stop ${SERVICE_NAME}'
  Pop $0
  StrCpy $1 0
  ${Do}
    nsExec::Exec 'cmd.exe /d /c sc.exe query ${SERVICE_NAME} | find "STOPPED"'
    Pop $0
    ${IfThen} $0 == 0 ${|} ${Break} ${|}
    IntOp $1 $1 + 1
    ${IfThen} $1 > 40 ${|} ${Break} ${|}
    Sleep 500
  ${Loop}
FunctionEnd
!macroend
!insertmacro DEFINE_STOP_SERVICE ""
!insertmacro DEFINE_STOP_SERVICE "un."

; The installer runs elevated. Start the app through Explorer so it runs as
; the signed-in user at medium integrity, like a normal launch.
Function LaunchApp
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\${APP_EXE}"'
FunctionEnd

; The push agent runs while the user is signed in to the app, which it records
; in the Run value (the app writes it, the agent or a sign-out removes it).
; Bring it back after an upgrade stopped it, as the user, like the app.
Function LaunchAgentIfEnabled
  ReadRegStr $0 HKCU "${RUN_KEY}" "${AGENT_RUN_VALUE}"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$INSTDIR\${AGENT_EXE}"
    DetailPrint "$(StartingAgent)"
    Exec '"$WINDIR\explorer.exe" "$INSTDIR\${AGENT_EXE}"'
  ${EndIf}
FunctionEnd

; ─────────────────────────────── install ───────────────────────────────

Function .onInit
  ${IfNot} ${RunningX64}
  ${OrIfNot} ${AtLeastWin10}
    MessageBox MB_ICONSTOP|MB_OK "$(NeedsWindows10)" /SD IDOK
    Abort
  ${EndIf}
  ${WinVerGetBuild} $0
  ${If} $0 < 17763
    MessageBox MB_ICONSTOP|MB_OK "$(NeedsWindows10)" /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext all

  System::Call 'kernel32::CreateMutexW(p 0, i 0, w "PPVPN.Desktop.Setup") p .r1 ?e'
  Pop $0
  ${If} $0 == 183 ; ERROR_ALREADY_EXISTS
    MessageBox MB_ICONEXCLAMATION|MB_OK "$(SetupRunning)" /SD IDOK
    Abort
  ${EndIf}

  ; Desktop shortcut: on by default for a fresh interactive install; an
  ; upgrade keeps what the user had; silent installs add one only with
  ; /DESKTOPSHORTCUT.
  StrCpy $CreateDesktop 0
  ReadRegStr $0 HKLM "${UNINST_KEY}" "UninstallString"
  ${IfNot} ${Silent}
  ${AndIf} $0 == ""
    StrCpy $CreateDesktop 1
  ${EndIf}
  ${If} ${FileExists} "$DESKTOP\${APP_NAME}.lnk"
    StrCpy $CreateDesktop 1
  ${EndIf}
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/DESKTOPSHORTCUT" $R1
  ${IfNot} ${Errors}
    StrCpy $CreateDesktop 1
  ${EndIf}
FunctionEnd

Function TasksPage
  !insertmacro MUI_HEADER_TEXT "$(TasksTitle)" "$(TasksSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${NSD_CreateCheckbox} 0 0 100% 12u "$(DesktopShortcut)"
  Pop $DesktopCheckbox
  ${If} $CreateDesktop == 1
    ${NSD_Check} $DesktopCheckbox
  ${EndIf}
  nsDialogs::Show
FunctionEnd

Function TasksPageLeave
  ${NSD_GetState} $DesktopCheckbox $CreateDesktop
FunctionEnd

Section "PPVPN" SecMain
  SectionIn RO
  SetOutPath "$INSTDIR"

  ; 1. Release every file in $INSTDIR: the app, the push agent, the service and its core.
  Call StopApp
  Call StopAgent
  Call StopService
  ; A service registered from another directory is re-registered: the install
  ; helper only starts an existing service and never changes its path.
  ReadRegStr $0 HKLM "SYSTEM\CurrentControlSet\Services\${SERVICE_NAME}" "ImagePath"
  ${If} $0 != ""
  ${AndIf} $0 != '"$INSTDIR\ppvpn-service.exe"'
  ${AndIf} $0 != "$INSTDIR\ppvpn-service.exe"
    DetailPrint "Re-registering ${SERVICE_NAME} (was $0)"
    nsExec::ExecToLog 'sc.exe delete ${SERVICE_NAME}'
    Pop $0
  ${EndIf}
  ; Standard-mode cores started by the app, if any survived it.
  nsExec::Exec 'taskkill.exe /f /im ${CORE_EXE}'
  Pop $0

  ; 2. Replace the files. Remove what the previous version installed first so
  ; stale runtime files do not pile up.
  Call RemoveListedFiles
  File /r "${STAGE_DIR}\*.*"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  ; 3. Register (or restart) the privileged service.
  DetailPrint "$(InstallingService)"
  nsExec::ExecToLog '"$INSTDIR\ppvpn-service-install.exe"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "ppvpn-service-install returned $0"
    MessageBox MB_ICONSTOP|MB_OK "$(ServiceInstallFailed)" /SD IDOK
    Abort
  ${EndIf}

  ; 4. Shortcuts.
  ; The shortcuts carry the app's AppUserModelID (System.AppUserModel.ID), so notifications
  ; and the taskbar button use the shortcut's name and icon. ppvpn.exe sets it and exits.
  CreateShortcut "$SMPROGRAMS\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
  nsExec::Exec '"$INSTDIR\${APP_EXE}" --set-shortcut-aumid "$SMPROGRAMS\${APP_NAME}.lnk"'
  Pop $0
  ${If} $0 != 0
    DetailPrint "setting the shortcut AppUserModelID returned $0"
  ${EndIf}
  ${If} $CreateDesktop == ${BST_CHECKED}
    CreateShortcut "$DESKTOP\${APP_NAME}.lnk" "$INSTDIR\${APP_EXE}" "" "$INSTDIR\${APP_EXE}" 0
    nsExec::Exec '"$INSTDIR\${APP_EXE}" --set-shortcut-aumid "$DESKTOP\${APP_NAME}.lnk"'
    Pop $0
  ${Else}
    Delete "$DESKTOP\${APP_NAME}.lnk"
  ${EndIf}

  ; 5. Apps & features.
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "${APP_NAME}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "${PUBLISHER}"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\${APP_EXE},0"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "https://www.peakpassvpn.com"
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  IntFmt $0 "0x%08X" $0
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" "$0"

  ; 6. The push agent comes back when the user had it (signed in); silent upgrades
  ; (auto-update) bring the app back if it was running.
  Call LaunchAgentIfEnabled
  ${If} ${Silent}
  ${AndIf} $AppWasRunning == 1
    Call LaunchApp
  ${EndIf}
SectionEnd

; ────────────────────────────── uninstall ──────────────────────────────

Function un.onInit
  SetRegView 64
  SetShellVarContext all
  StrCpy $RemoveData 0
  ${GetParameters} $R0
  ClearErrors
  ${GetOptions} $R0 "/PURGE" $R1
  ${IfNot} ${Errors}
    StrCpy $RemoveData 1
  ${EndIf}
FunctionEnd

Function un.DataPage
  !insertmacro MUI_HEADER_TEXT "$(DataTitle)" "$(DataSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${NSD_CreateLabel} 0 0 100% 40u "$(DataExplain)"
  Pop $0
  ${NSD_CreateCheckbox} 0 48u 100% 12u "$(DataRemove)"
  Pop $DataCheckbox
  ${If} $RemoveData == ${BST_CHECKED}
    ${NSD_Check} $DataCheckbox
  ${EndIf}
  GetDlgItem $0 $HWNDPARENT 1
  SendMessage $0 ${WM_SETTEXT} 0 "STR:$(^UninstallBtn)"
  nsDialogs::Show
FunctionEnd

Function un.DataPageLeave
  ${NSD_GetState} $DataCheckbox $RemoveData
FunctionEnd

Section "Uninstall"
  Call un.StopApp
  Call un.StopAgent

  DetailPrint "$(RemovingService)"
  ${If} ${FileExists} "$INSTDIR\ppvpn-service-uninstall.exe"
    nsExec::ExecToLog '"$INSTDIR\ppvpn-service-uninstall.exe"'
    Pop $0
  ${Else}
    Call un.StopService
    nsExec::ExecToLog 'sc.exe delete ${SERVICE_NAME}'
    Pop $0
  ${EndIf}
  nsExec::Exec 'taskkill.exe /f /im ${CORE_EXE}'
  Pop $0

  ; Launch at sign-in and the push agent's autostart (per user).
  DeleteRegValue HKCU "${RUN_KEY}" "${RUN_VALUE}"
  DeleteRegValue HKCU "${RUN_KEY}" "${AGENT_RUN_VALUE}"
  ; Notification registration (per user; Windows App SDK AppNotificationManager.Register).
  DeleteRegKey HKCU "Software\Classes\AppUserModelId\${AUMID}"
  DeleteRegKey HKCU "Software\Classes\CLSID\${TOAST_ACTIVATOR_CLSID}"

  Delete "$SMPROGRAMS\${APP_NAME}.lnk"
  Delete "$DESKTOP\${APP_NAME}.lnk"

  Call un.RemoveListedFiles
  Delete "$INSTDIR\ppvpn-service.log"
  Delete "$INSTDIR\ppvpn-core.log"
  Delete "$INSTDIR\uninstall.exe"
  ; The directory is fixed and owned by this installer.
  RMDir /r "$INSTDIR"
  DeleteRegKey HKLM "${UNINST_KEY}"

  ${If} $RemoveData == ${BST_CHECKED}
    DetailPrint "$(RemovingData)"
    ; Per-user: app data and logs, the saved sign-in, WinSparkle settings.
    SetShellVarContext current
    RMDir /r "$LOCALAPPDATA\PPVPN"
    System::Call 'advapi32::CredDeleteW(w "${CREDENTIAL_TARGET}", i 1, i 0) i .r0'
    System::Call 'advapi32::CredDeleteW(w "${FAKE_CREDENTIAL_TARGET}", i 1, i 0) i .r0'
    DeleteRegKey HKCU "Software\PPVPN"
    ; Machine: the service's core state (C:\ProgramData\PPVPN).
    SetShellVarContext all
    RMDir /r "$APPDATA\PPVPN"
  ${EndIf}
SectionEnd
