; Tauri v2 NSIS installer hooks (bundle.windows.nsis.installerHooks in tauri.conf.json).
;
; What this does, in install order:
;   PREINSTALL   close any running copy, remove stale native runtime files so an older
;                onnxruntime can never be loaded next to the new app.
;   POSTINSTALL  make sure the Microsoft Visual C++ 2015-2022 x64 runtime is present (runs the
;                bundled, Microsoft-signed vc_redist.x64.exe when it is not), check that the
;                WebView2 runtime is present, and record the outcome in the registry and in
;                %TEMP%\plugable-chat-install.log.
;   PREUNINSTALL / POSTUNINSTALL  close the app, remove only our own registry key.
;
; Design rules:
;   - No failure here is fatal. The app has its own first-launch self-check and shows a
;     "can't start" card, so a failed VC++ install must not leave the user without an app.
;     Silent installs (/S, Intune, SCCM) only log; interactive installs also show a MessageBox.
;   - User data (%APPDATA%\plugable-chat, %LOCALAPPDATA%\plugable-chat) is never touched, on
;     upgrade or uninstall. Tauri's own "delete application data" option targets the
;     com.plugable.chat folders, which the app does not use.
;   - vc_redist.x64.exe stays in $INSTDIR\redist after install on purpose: the app's first-launch
;     repair re-runs it elevated if the runtime is later damaged.
;
; NSIS cannot be run on the dev machine used to write this; it is validated by
; scripts/ci/verify-installers.ps1 on Windows (registry key + log + payload hash).

!include "LogicLib.nsh"

!define PC_HOOK_VERSION 1
!define PC_REG_KEY "SOFTWARE\Plugable\plugable-chat\Installer"
!define PC_VCRUNTIME_KEY "SOFTWARE\Microsoft\VisualStudio\14.0\VC\Runtimes\x64"
; Minimum VC++ build number (the registry "Bld" value, 14.38 = 33135). Keep equal to "minBld" in
; scripts/ci/vcredist.pin.json; scripts/ci/clean-host.tests.ps1 fails when they differ.
!define PC_VCREDIST_MIN_BLD 33135
!define PC_WEBVIEW2_GUID "{F3017226-FE2A-4295-8BDF-00C3A9A7E4C5}"
!define PC_VCREDIST_URL "https://aka.ms/vs/17/release/vc_redist.x64.exe"
!define PC_WEBVIEW2_URL "https://developer.microsoft.com/microsoft-edge/webview2/"

; --- logging -------------------------------------------------------------------------------
; Message on the stack. Appends "yyyy-mm-dd hh:mm:ss  message" to %TEMP%\plugable-chat-install.log.
; Registers are saved because hooks run in the middle of Tauri's own sections.
!macro PcLogFunc un
Function ${un}PcLog
  Exch $0
  Push $1
  Push $2
  Push $3
  Push $4
  Push $5
  Push $6
  Push $7
  System::Alloc 16
  Pop $1
  System::Call 'kernel32::GetLocalTime(p r1)'
  System::Call '*$1(&i2 .r2, &i2 .r3, &i2, &i2 .r4, &i2 .r5, &i2 .r6, &i2 .r7)'
  System::Free $1
  IntFmt $3 "%02d" $3
  IntFmt $4 "%02d" $4
  IntFmt $5 "%02d" $5
  IntFmt $6 "%02d" $6
  IntFmt $7 "%02d" $7
  ClearErrors
  FileOpen $1 "$TEMP\plugable-chat-install.log" a
  ${IfNot} ${Errors}
    FileSeek $1 0 END
    FileWrite $1 "$2-$3-$4 $5:$6:$7  $0$\r$\n"
    FileClose $1
  ${EndIf}
  Pop $7
  Pop $6
  Pop $5
  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd
!macroend
!insertmacro PcLogFunc ""
!insertmacro PcLogFunc "un."

!macro PcLog msg
  Push "${msg}"
  Call PcLog
!macroend
!macro PcLogUn msg
  Push "${msg}"
  Call un.PcLog
!macroend

; --- stale runtime files -------------------------------------------------------------------
; Why: an older release can leave a different onnxruntime in foundry-libs (or beside the exe,
; where the Windows loader looks first). A mismatched pair loads and then crashes or reports a
; missing entry point, so the directory is emptied before the new files are copied.
Function PcCleanStaleLibs
  Push $0
  ${If} ${FileExists} "$INSTDIR\foundry-libs"
    StrCpy $0 0
    ${Do}
      RMDir /r "$INSTDIR\foundry-libs"
      ${IfNot} ${FileExists} "$INSTDIR\foundry-libs"
        ${ExitDo}
      ${EndIf}
      IntOp $0 $0 + 1
      Sleep 1000
    ${LoopWhile} $0 < 5
    ${If} ${FileExists} "$INSTDIR\foundry-libs"
      !insertmacro PcLog "WARNING: could not remove $INSTDIR\foundry-libs after 5 tries (file in use or blocked by security software)"
      ${IfNot} ${Silent}
        MessageBox MB_ICONEXCLAMATION|MB_OK "Plugable Chat could not remove old files in:$\r$\n$INSTDIR\foundry-libs$\r$\n$\r$\nClose other programs that may be using them, or check your security software, then run this installer again.$\r$\n$\r$\nDetails: $TEMP\plugable-chat-install.log"
      ${EndIf}
    ${Else}
      !insertmacro PcLog "removed stale $INSTDIR\foundry-libs"
    ${EndIf}
  ${EndIf}
  ; Older releases copied runtime DLLs next to the exe; those would shadow foundry-libs.
  ; The current package re-copies any it still wants after this hook runs.
  Delete "$INSTDIR\onnxruntime.dll"
  Delete "$INSTDIR\onnxruntime_providers_shared.dll"
  Delete "$INSTDIR\onnxruntime-genai.dll"
  Delete "$INSTDIR\Microsoft.AI.Foundry.Local.Core.dll"
  Pop $0
FunctionEnd

; --- VC++ runtime --------------------------------------------------------------------------
; Leaves the outcome in the registry (values under PC_REG_KEY) so verify-installers.ps1 and
; support can read it. Status strings: present | installed | installed-reboot | newer-present |
; missing-payload | failed.
Function PcVcRedistEnsure
  Push $0
  Push $1
  Push $2
  Push $3
  Push $4
  SetRegView 64

  ClearErrors
  ReadRegDWORD $0 HKLM "${PC_VCRUNTIME_KEY}" "Installed"
  ${If} ${Errors}
    StrCpy $0 0
  ${EndIf}
  ClearErrors
  ReadRegDWORD $1 HKLM "${PC_VCRUNTIME_KEY}" "Bld"
  ${If} ${Errors}
    StrCpy $1 0
  ${EndIf}
  StrCpy $2 0
  StrCpy $3 0

  ${If} $0 = 1
  ${AndIf} $1 >= ${PC_VCREDIST_MIN_BLD}
    StrCpy $4 "present"
    !insertmacro PcLog "vcredist: present (Bld $1, minimum ${PC_VCREDIST_MIN_BLD}); not reinstalling"
  ${ElseIfNot} ${FileExists} "$INSTDIR\redist\vc_redist.x64.exe"
    StrCpy $4 "missing-payload"
    !insertmacro PcLog "vcredist: needed (Installed=$0 Bld=$1) but $INSTDIR\redist\vc_redist.x64.exe is not in this package"
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK "The Microsoft Visual C++ Runtime is missing or too old, and this installer does not contain it.$\r$\n$\r$\nPlugable Chat may not start. Install it from:$\r$\n${PC_VCREDIST_URL}$\r$\n$\r$\nDetails: $TEMP\plugable-chat-install.log"
    ${EndIf}
  ${Else}
    !insertmacro PcLog "vcredist: needed (Installed=$0 Bld=$1, minimum ${PC_VCREDIST_MIN_BLD}); running bundled installer"
    ClearErrors
    ExecWait '"$INSTDIR\redist\vc_redist.x64.exe" /install /quiet /norestart /log "$TEMP\plugable-chat-vcredist.log"' $2
    ${If} ${Errors}
      StrCpy $2 -1
    ${EndIf}
    !insertmacro PcLog "vcredist: installer exit code $2"
    ${Switch} $2
      ${Case} 0
        StrCpy $4 "installed"
        ${Break}
      ${Case} 1638
        ; "another version of this product is already installed": a newer runtime is present.
        StrCpy $4 "newer-present"
        ${Break}
      ${Case2} 3010 1641
        StrCpy $4 "installed-reboot"
        StrCpy $3 1
        ${Break}
      ${Default}
        StrCpy $4 "failed"
        ${Break}
    ${EndSwitch}

    ${If} $4 == "failed"
      !insertmacro PcLog "vcredist: FAILED with exit code $2 (see $TEMP\plugable-chat-vcredist.log); continuing, the app will show its own repair guidance"
      ${IfNot} ${Silent}
        MessageBox MB_ICONEXCLAMATION|MB_OK "Plugable Chat was installed, but the Microsoft Visual C++ Runtime could not be installed (error $2).$\r$\n$\r$\nThe app may not start. Install the runtime from:$\r$\n${PC_VCREDIST_URL}$\r$\nthen start Plugable Chat again.$\r$\n$\r$\nDetails: $TEMP\plugable-chat-install.log"
      ${EndIf}
    ${Else}
      ClearErrors
      ReadRegDWORD $0 HKLM "${PC_VCRUNTIME_KEY}" "Installed"
      ReadRegDWORD $1 HKLM "${PC_VCRUNTIME_KEY}" "Bld"
      !insertmacro PcLog "vcredist: after install Installed=$0 Bld=$1 status=$4"
      ${If} $3 = 1
        !insertmacro PcLog "vcredist: a restart is needed to finish the runtime install"
        ${IfNot} ${Silent}
          MessageBox MB_ICONINFORMATION|MB_OK "The Microsoft Visual C++ Runtime was installed but Windows needs a restart to finish.$\r$\n$\r$\nRestart your computer before starting Plugable Chat for the first time."
        ${EndIf}
      ${EndIf}
    ${EndIf}
  ${EndIf}

  WriteRegDWORD HKLM "${PC_REG_KEY}" "HookVersion" ${PC_HOOK_VERSION}
  WriteRegStr HKLM "${PC_REG_KEY}" "VCRedistStatus" "$4"
  WriteRegDWORD HKLM "${PC_REG_KEY}" "VCRedistExitCode" $2
  WriteRegDWORD HKLM "${PC_REG_KEY}" "VCRedistBld" $1
  WriteRegDWORD HKLM "${PC_REG_KEY}" "RebootRequired" $3
  WriteRegStr HKLM "${PC_REG_KEY}" "InstallLog" "$TEMP\plugable-chat-install.log"

  Pop $4
  Pop $3
  Pop $2
  Pop $1
  Pop $0
FunctionEnd

; --- WebView2 ------------------------------------------------------------------------------
; Tauri's embedBootstrapper step runs before this. It needs internet (or an offline mirror) and
; can be blocked by proxies; this verifies the result instead of assuming it.
Function PcWebView2Check
  Push $0
  SetRegView 64
  ClearErrors
  ReadRegStr $0 HKLM "SOFTWARE\WOW6432Node\Microsoft\EdgeUpdate\Clients\${PC_WEBVIEW2_GUID}" "pv"
  ${If} ${Errors}
  ${OrIf} $0 == ""
    ClearErrors
    ReadRegStr $0 HKCU "Software\Microsoft\EdgeUpdate\Clients\${PC_WEBVIEW2_GUID}" "pv"
  ${EndIf}
  ${If} ${Errors}
  ${OrIf} $0 == ""
  ${OrIf} $0 == "0.0.0.0"
    WriteRegStr HKLM "${PC_REG_KEY}" "WebView2Status" "missing"
    !insertmacro PcLog "webview2: NOT detected after install (bootstrapper blocked or offline?)"
    ${IfNot} ${Silent}
      MessageBox MB_ICONEXCLAMATION|MB_OK "The Microsoft WebView2 Runtime was not detected. Plugable Chat needs it to show its window.$\r$\n$\r$\nInstall the Evergreen Standalone Installer (works offline) from:$\r$\n${PC_WEBVIEW2_URL}$\r$\nthen start Plugable Chat.$\r$\n$\r$\nDetails: $TEMP\plugable-chat-install.log"
    ${EndIf}
  ${Else}
    WriteRegStr HKLM "${PC_REG_KEY}" "WebView2Status" "present"
    WriteRegStr HKLM "${PC_REG_KEY}" "WebView2Version" "$0"
    !insertmacro PcLog "webview2: present ($0)"
  ${EndIf}
  Pop $0
FunctionEnd

; --- hooks ---------------------------------------------------------------------------------
!macro NSIS_HOOK_PREINSTALL
  !insertmacro PcLog "---- install start: $INSTDIR (hook v${PC_HOOK_VERSION})"
  ; Tauri already asks to close a running copy; this also covers silent installs and other
  ; users' sessions (a locked exe or DLL is the usual cause of a half-copied upgrade).
  Push $0
  nsExec::Exec 'taskkill /F /T /IM "${MAINBINARYNAME}.exe"'
  Pop $0
  !insertmacro PcLog "closed running instance (taskkill exit $0; 128 means none was running)"
  Pop $0
  Sleep 500
  Call PcCleanStaleLibs
!macroend

!macro NSIS_HOOK_POSTINSTALL
  Call PcVcRedistEnsure
  Call PcWebView2Check
  !insertmacro PcLog "---- install end"
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro PcLogUn "---- uninstall start: $INSTDIR"
  Push $0
  nsExec::Exec 'taskkill /F /T /IM "${MAINBINARYNAME}.exe"'
  Pop $0
  Pop $0
  Sleep 500
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  SetRegView 64
  DeleteRegKey HKLM "${PC_REG_KEY}"
  DeleteRegKey /ifempty HKLM "SOFTWARE\Plugable\plugable-chat"
  DeleteRegKey /ifempty HKLM "SOFTWARE\Plugable"
  !insertmacro PcLogUn "---- uninstall end (user data left in place)"
!macroend
