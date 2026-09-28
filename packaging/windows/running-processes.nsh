!ifndef KEYTAO_RUNNING_PROCESSES_INCLUDED
!define KEYTAO_RUNNING_PROCESSES_INCLUDED
!include "nsDialogs.nsh"
!include "TextFunc.nsh"

Var PreflightStatus
Var PreflightReport
Var PreflightList
Var PreflightSummary
Var PreflightRefresh
Var PreflightStopArg

LangString PreflightTitle ${LANG_ENGLISH} "Prepare to update KeyTao"
LangString PreflightTitle ${LANG_SIMPCHINESE} "安装前准备"
LangString PreflightSubtitle ${LANG_ENGLISH} "Continue to automatically stop this installation's KeyTao app."
LangString PreflightSubtitle ${LANG_SIMPCHINESE} "点击继续，自动结束当前安装目录中的 KeyTao 程序。"
LangString PreflightInstructions ${LANG_ENGLISH} "Click Continue to force-close the KeyTao programs listed below and proceed. Finish any KeyTao configuration or deployment work first."
LangString PreflightInstructions ${LANG_SIMPCHINESE} "点击“继续”后，将强制结束下方 KeyTao 程序并继续安装或卸载。请先完成 KeyTao 的配置、方案部署等操作。"
LangString PreflightHostHelp ${LANG_ENGLISH} "Weasel, your browser, Explorer and other applications will stay open. Reopen apps already using the old KeyTao input method later to use the new version."
LangString PreflightHostHelp ${LANG_SIMPCHINESE} "小狼毫、浏览器、资源管理器和其他应用会保持运行。已经加载旧 KeyTao 输入法的应用，稍后重新打开即可使用新版。"
LangString PreflightChecking ${LANG_ENGLISH} "Checking running programs..."
LangString PreflightChecking ${LANG_SIMPCHINESE} "正在检测运行中的程序…"
LangString PreflightClear ${LANG_ENGLISH} "No KeyTao programs need to be stopped. You can continue."
LangString PreflightClear ${LANG_SIMPCHINESE} "没有需要结束的 KeyTao 程序，可以继续。"
LangString PreflightBusy ${LANG_ENGLISH} "These KeyTao programs will be stopped when you continue."
LangString PreflightBusy ${LANG_SIMPCHINESE} "点击继续后，将自动结束以下 KeyTao 程序。"
LangString PreflightFailed ${LANG_ENGLISH} "The check failed. Setup will not change the existing installation."
LangString PreflightFailed ${LANG_SIMPCHINESE} "占用检测失败，暂不能继续。现有安装尚未更改。"
LangString PreflightRetry ${LANG_ENGLISH} "Check again"
LangString PreflightRetry ${LANG_SIMPCHINESE} "重新检测"
LangString PreflightContinue ${LANG_ENGLISH} "Continue"
LangString PreflightContinue ${LANG_SIMPCHINESE} "继续"
LangString PreflightBlocked ${LANG_ENGLISH} "A KeyTao program could not be stopped. Retry, or close the listed KeyTao program manually."
LangString PreflightBlocked ${LANG_SIMPCHINESE} "未能结束 KeyTao 程序。请重试，或手动退出列出的 KeyTao 程序。"

; Use the same gate for installation, ordinary uninstall and /S /UPDATE.
; Only KeyTao executables at this installation's verified paths qualify.
; Weasel is a separate input method; KeyTao loads its own in-process librime.
; Loading a TIP DLL does not give ownership of a browser/editor process.
!macro DefineRunningProcesses Prefix
Function ${Prefix}CheckRunningProcesses
  InitPluginsDir
  File /oname=$PLUGINSDIR\check-running-processes.ps1 "${REPO_ROOT}\packaging\windows\check-running-processes.ps1"
  Delete "$PLUGINSDIR\running-processes.txt"
  StrCpy $PreflightStatus 20
  StrCpy $PreflightReport ""
  ; Page previews leave PreflightStopArg empty. Only Continue/section entry
  ; sets -StopOwnedProcesses; never infer ownership from loaded DLLs.
  nsExec::ExecToStack /TIMEOUT=60000 '"$WINDIR\Sysnative\WindowsPowerShell\v1.0\powershell.exe" -NoLogo -NoProfile -NonInteractive -WindowStyle Hidden -ExecutionPolicy Bypass -File "$PLUGINSDIR\check-running-processes.ps1" -InstallDir "$INSTDIR\." -ReportPath "$PLUGINSDIR\running-processes.txt" $PreflightStopArg'
  Pop $PreflightStatus
  Pop $0
  ${If} $PreflightStatus != 0
  ${AndIf} $PreflightStatus != 10
    StrCpy $PreflightStatus 20
    DetailPrint "$0"
  ${EndIf}
  ClearErrors
  FileOpen $0 "$PLUGINSDIR\running-processes.txt" r
  ${If} ${Errors}
    StrCpy $PreflightStatus 20
    StrCpy $PreflightReport "$(PreflightFailed)"
  ${Else}
    ${Do}
      ClearErrors
      FileReadUTF16LE $0 $1
      ${If} ${Errors}
        ${ExitDo}
      ${EndIf}
      StrCpy $PreflightReport "$PreflightReport$1"
    ${Loop}
    FileClose $0
  ${EndIf}
FunctionEnd

Function ${Prefix}UpdateRunningProcessesPage
  ${NSD_SetText} $PreflightSummary "$(PreflightChecking)"
  EnableWindow $PreflightRefresh 0
  GetDlgItem $0 $HWNDPARENT 1
  EnableWindow $0 0
  StrCpy $PreflightStopArg ""
  Call ${Prefix}CheckRunningProcesses
  ; A read-only list avoids an edit control activating the old input method
  ; inside the installer itself. Read each bounded report line separately.
  SendMessage $PreflightList ${LB_RESETCONTENT} 0 0
  ClearErrors
  FileOpen $0 "$PLUGINSDIR\running-processes.txt" r
  ${IfNot} ${Errors}
    ${Do}
      ClearErrors
      FileReadUTF16LE $0 $1
      ${If} ${Errors}
        ${ExitDo}
      ${EndIf}
      ${TrimNewLines} $1 $1
      ${NSD_LB_AddString} $PreflightList $1
    ${Loop}
    FileClose $0
  ${EndIf}
  ${If} $PreflightStatus == 0
    ${NSD_SetText} $PreflightSummary "$(PreflightClear)"
  ${ElseIf} $PreflightStatus == 10
    ${NSD_SetText} $PreflightSummary "$(PreflightBusy)"
  ${Else}
    ${NSD_SetText} $PreflightSummary "$(PreflightFailed)"
  ${EndIf}
  EnableWindow $PreflightRefresh 1
  GetDlgItem $0 $HWNDPARENT 1
  EnableWindow $0 1
FunctionEnd

Function ${Prefix}RefreshRunningProcesses
  Pop $0
  Call ${Prefix}UpdateRunningProcessesPage
FunctionEnd

Function ${Prefix}CreateRunningProcessesPage
  !insertmacro MUI_HEADER_TEXT "$(PreflightTitle)" "$(PreflightSubtitle)"
  nsDialogs::Create 1018
  Pop $0
  ${If} $0 == error
    Abort
  ${EndIf}
  ; The MUI inner dialog (1018) is only 140 dialog units high.
  ${NSD_CreateLabel} 0 0 100% 26u "$(PreflightInstructions)"
  Pop $0
  ${NSD_CreateLabel} 0 29u 100% 12u "$(PreflightChecking)"
  Pop $PreflightSummary
  ${NSD_CreateListBox} 0 43u 100% 43u ""
  Pop $PreflightList
  ${NSD_CreateLabel} 0 89u 100% 29u "$(PreflightHostHelp)"
  Pop $0
  ${NSD_CreateButton} 0 122u 80u 14u "$(PreflightRetry)"
  Pop $PreflightRefresh
  ${NSD_OnClick} $PreflightRefresh ${Prefix}RefreshRunningProcesses
  Call ${Prefix}UpdateRunningProcessesPage
  GetDlgItem $0 $HWNDPARENT 1
  ${NSD_SetText} $0 "$(PreflightContinue)"
  nsDialogs::Show
FunctionEnd

Function ${Prefix}LeaveRunningProcessesPage
  Call ${Prefix}EnsureProcessesStopped
FunctionEnd

Function ${Prefix}EnsureProcessesStopped
  ${Do}
    StrCpy $PreflightStopArg "-StopOwnedProcesses"
    Call ${Prefix}CheckRunningProcesses
    StrCpy $PreflightStopArg ""
    ${If} $PreflightStatus == 0
      SetErrorLevel 0
      Return
    ${EndIf}
    DetailPrint "$PreflightReport"
    ${If} $PreflightStatus == 10
      IfSilent preflight_busy_abort
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "$(PreflightBlocked)$\r$\n$\r$\n$PreflightReport$\r$\n$(PreflightHostHelp)" /SD IDCANCEL IDRETRY preflight_retry
      preflight_busy_abort:
        SetErrorLevel 32
        Abort "$(PreflightBlocked)"
    ${Else}
      IfSilent preflight_error_abort
      MessageBox MB_RETRYCANCEL|MB_ICONSTOP "$(PreflightFailed)$\r$\n$\r$\n$PreflightReport" /SD IDCANCEL IDRETRY preflight_retry
      preflight_error_abort:
        SetErrorLevel 1
        Abort "$(PreflightFailed)"
    ${EndIf}
    preflight_retry:
  ${Loop}
FunctionEnd
!macroend

!insertmacro DefineRunningProcesses ""
!insertmacro DefineRunningProcesses "un."
!endif
