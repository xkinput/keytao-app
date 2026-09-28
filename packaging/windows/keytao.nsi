Unicode true
!include "MUI2.nsh"
!include "LogicLib.nsh"
!include "FileFunc.nsh"
!include "x64.nsh"

; All inputs are absolute paths supplied by build-windows-flutter.ps1.
!ifndef VERSION
  !error "VERSION is required"
!endif
!ifndef VERSION_NUMERIC
  !error "VERSION_NUMERIC is required"
!endif
!ifndef BUNDLE_DIR
  !error "BUNDLE_DIR is required"
!endif
!ifndef OUTPUT_FILE
  !error "OUTPUT_FILE is required"
!endif
!ifndef REPO_ROOT
  !error "REPO_ROOT is required"
!endif
!ifndef UNINSTALL_FILES
  !error "UNINSTALL_FILES is required"
!endif
!ifndef INSTALL_FILES
  !error "INSTALL_FILES is required"
!endif

; Match Tauri's perMachine x64 defaults. The publisher defaults to the
; second component of ink.rea.keytao-app, not the Cargo authors field.
!define PRODUCTNAME "KeyTao"
!define MANUFACTURER "rea"
!define UNINSTKEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\KeyTao"
!define MANUPRODUCTKEY "Software\rea\KeyTao"
!include "nsis-hooks.nsh"

Name "${PRODUCTNAME}"
OutFile "${OUTPUT_FILE}"
InstallDir "$PROGRAMFILES64\KeyTao"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
SetOverwrite on
ShowInstDetails show
ShowUninstDetails show
!define MUI_ICON "${REPO_ROOT}\packaging\icons\icon.ico"
!define MUI_UNICON "${REPO_ROOT}\packaging\icons\icon.ico"
VIProductVersion "${VERSION_NUMERIC}"
VIAddVersionKey "ProductName" "KeyTao"
VIAddVersionKey "CompanyName" "rea"
VIAddVersionKey "FileDescription" "KeyTao Setup"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Copyright (C) 2026 rea"

Var PreviousInstallDir
Var UpdateMode
!define MUI_PAGE_CUSTOMFUNCTION_PRE DirectoryPage
!insertmacro MUI_PAGE_DIRECTORY
Page custom CreateRunningProcessesPage LeaveRunningProcessesPage
!insertmacro MUI_PAGE_INSTFILES
!define MUI_FINISHPAGE_TEXT "$(KeyTaoFinishText)"
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
UninstPage custom un.CreateRunningProcessesPage un.LeaveRunningProcessesPage
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"
!insertmacro MUI_LANGUAGE "SimpChinese"
LangString KeyTaoFinishText ${LANG_ENGLISH} "KeyTao has been installed. Applications that already loaded the old input method can keep running. Reopen those applications, or sign out of Windows later, to use the updated input method."
LangString KeyTaoFinishText ${LANG_SIMPCHINESE} "KeyTao 已安装完成。已经加载旧输入法的应用可以继续使用；在方便时重新打开这些应用，或注销 Windows 后重新登录，即可使用新版输入法。"
!include /CHARSET=UTF8 "running-processes.nsh"
!include "upgrade-existing.nsh"
!include "replace-payload.nsh"

!macro Context
  SetShellVarContext all
  SetRegView 64
!macroend

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "This KeyTao package requires 64-bit Windows."
    Abort
  ${EndIf}
  !insertmacro Context
  ReadRegStr $PreviousInstallDir HKLM "${MANUPRODUCTKEY}" ""
  ${If} $PreviousInstallDir == ""
    ReadRegStr $PreviousInstallDir HKLM "${UNINSTKEY}" "InstallLocation"
    ; Tauri quotes InstallLocation, but not MANUPRODUCTKEY's default value.
    StrCpy $0 $PreviousInstallDir 1
    ${If} $0 == '$\"'
      StrCpy $PreviousInstallDir $PreviousInstallDir -1 1
    ${EndIf}
  ${EndIf}
  ${If} $PreviousInstallDir != ""
    StrCpy $INSTDIR $PreviousInstallDir
  ${EndIf}
FunctionEnd

Function DirectoryPage
  ; An upgrade cannot accidentally become a second install with the same keys.
  ${If} $PreviousInstallDir != ""
    Abort
  ${EndIf}
FunctionEnd

Section "KeyTao" SEC_APP
  !insertmacro Context
  ${If} $PreviousInstallDir != ""
    StrCpy $INSTDIR $PreviousInstallDir
  ${EndIf}
  ; Stop only this installation's verified KeyTao programs before modifying files,
  ; including /S upgrades where the preparation page is not displayed.
  Call EnsureProcessesStopped
  Call PrepareExistingInstallation
  ; Replace each owned file with rollback for failed copies. A DLL mapped by
  ; another application is renamed in the same directory before the new file
  ; is written, allowing that application to keep its existing mapping.
  !include "${INSTALL_FILES}"
  SetOutPath "$INSTDIR"
  ClearErrors
  WriteUninstaller "$INSTDIR\uninstall.exe"
  ${If} ${Errors}
    Abort "Unable to write the KeyTao uninstaller. Check disk space and file permissions."
  ${EndIf}
  WriteRegStr HKLM "${MANUPRODUCTKEY}" "" "$INSTDIR"
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayName" "KeyTao"
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINSTKEY}" "Publisher" "rea"
  WriteRegStr HKLM "${UNINSTKEY}" "MainBinaryName" "keytao-app.exe"
  WriteRegStr HKLM "${UNINSTKEY}" "DisplayIcon" '$\"$INSTDIR\keytao-app.exe$\"'
  WriteRegStr HKLM "${UNINSTKEY}" "InstallLocation" '$\"$INSTDIR$\"'
  WriteRegStr HKLM "${UNINSTKEY}" "UninstallString" '$\"$INSTDIR\uninstall.exe$\"'
  WriteRegStr HKLM "${UNINSTKEY}" "QuietUninstallString" '$\"$INSTDIR\uninstall.exe$\" /S'
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINSTKEY}" "NoRepair" 1
  ${If} ${Errors}
    Abort "Unable to write the KeyTao installation registry entries."
  ${EndIf}
  ${GetSize} "$INSTDIR" "/S=0K" $0 $1 $2
  WriteRegDWORD HKLM "${UNINSTKEY}" "EstimatedSize" $0
  CreateShortcut "$SMPROGRAMS\KeyTao.lnk" "$INSTDIR\keytao-app.exe"
  CreateShortcut "$DESKTOP\KeyTao.lnk" "$INSTDIR\keytao-app.exe"
  ; Keep the hook in a function: its failure-path Return must not skip setup
  ; bookkeeping, matching the old Tauri installer recovery behavior.
  Call InstallIme
SectionEnd

Function InstallIme
  !insertmacro NSIS_HOOK_POSTINSTALL
FunctionEnd

Function un.onInit
  !insertmacro Context
  StrCpy $UpdateMode 0
  ${GetOptions} $CMDLINE "/UPDATE" $0
  ${IfNot} ${Errors}
    StrCpy $UpdateMode 1
  ${EndIf}
FunctionEnd

Section "Uninstall"
  !insertmacro Context
  Call un.EnsureProcessesStopped
  SetOutPath "$TEMP"
  !insertmacro NSIS_HOOK_PREUNINSTALL
  ; Delete only files installed by this package, never arbitrary files in a
  ; user-selected directory. The manifest is generated from the staged bundle.
  !include "${UNINSTALL_FILES}"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  ${If} $UpdateMode != 1
    Delete "$SMPROGRAMS\KeyTao.lnk"
    Delete "$DESKTOP\KeyTao.lnk"
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "KeyTao"
  ${EndIf}
  DeleteRegKey HKLM "${UNINSTKEY}"
  ; Retain the old install-location preference and all user data, as Tauri did.
SectionEnd
