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
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!macro Context
  SetShellVarContext all
  SetRegView 64
!macroend

; The Tauri template also stops all processes with this executable name for
; perMachine installs. Do this before the old uninstaller touches the TSF.
!macro StopApp
  nsExec::ExecToStack '"$SYSDIR\taskkill.exe" /F /T /IM keytao-app.exe'
  Pop $0
  Pop $1
  ${If} $0 != 0
  ${AndIf} $0 != 128
    DetailPrint "$1"
    Abort "Unable to stop KeyTao. Close it in every Windows session and retry."
  ${EndIf}
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
  !insertmacro StopApp
  ${If} $PreviousInstallDir != ""
    StrCpy $INSTDIR $PreviousInstallDir
  ${EndIf}
  ${If} ${FileExists} "$INSTDIR\uninstall.exe"
    DetailPrint "Removing the previous KeyTao payload; preserving user data..."
    ClearErrors
    ; _?= keeps the old uninstaller synchronous and /UPDATE preserves Tauri
    ; app data, shortcuts and autostart. Never delete the user's Rime directory.
    ExecWait '"$INSTDIR\uninstall.exe" /S /UPDATE _?=$INSTDIR' $0
    ${If} ${Errors}
      Abort "Unable to start the previous KeyTao uninstaller."
    ${EndIf}
    ${If} $0 != 0
      Abort "The previous KeyTao uninstaller failed."
    ${EndIf}
  ${ElseIf} ${FileExists} "$INSTDIR\keytao-app.exe"
    Abort "The existing KeyTao installation has no uninstaller. Repair it before upgrading."
  ${EndIf}
  ; Refuse a still-mapped old executable instead of silently scheduling its
  ; replacement for reboot and reporting an upgraded app that cannot run.
  ${If} ${FileExists} "$INSTDIR\keytao-app.exe"
    ClearErrors
    Delete "$INSTDIR\keytao-app.exe"
    ${If} ${Errors}
      Abort "KeyTao is still locked. Restart Windows and retry."
    ${EndIf}
  ${EndIf}
  SetOutPath "$INSTDIR"
  ClearErrors
  File /r "${BUNDLE_DIR}\*"
  ${If} ${Errors}
    Abort "Unable to copy the KeyTao release bundle."
  ${EndIf}
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
  !insertmacro StopApp
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
