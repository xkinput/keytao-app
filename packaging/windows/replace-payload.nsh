!ifndef KEYTAO_REPLACE_PAYLOAD_INCLUDED
!define KEYTAO_REPLACE_PAYLOAD_INCLUDED
!include "LogicLib.nsh"
!include "FileFunc.nsh"

Var PayloadTarget
Var PayloadDirectory
Var PayloadBackup
Var PayloadHadRebootFlag

; Windows permits an image DLL to be renamed on the same volume while hosts
; still execute its mapped bytes. Replace the pathname, not the mapped image.
; The caller supplies a validated relative file path from the bundle manifest.
!macro InstallPayloadFile RelativePath
  StrCpy $PayloadTarget "$INSTDIR\${RelativePath}"
  StrCpy $PayloadBackup ""
  ${GetParent} "$PayloadTarget" $PayloadDirectory
  ClearErrors
  CreateDirectory "$PayloadDirectory"
  ${If} ${Errors}
    Abort "Unable to create the installation directory: $PayloadDirectory"
  ${EndIf}
  ${If} ${FileExists} "$PayloadTarget\*.*"
    Abort "A directory occupies an installed file path: $PayloadTarget"
  ${EndIf}
  ${If} ${FileExists} "$PayloadTarget"
    ClearErrors
    GetTempFileName $PayloadBackup "$PayloadDirectory"
    ${If} ${Errors}
      Abort "Unable to prepare a replacement for: $PayloadTarget"
    ${EndIf}
    ; GetTempFileName creates a placeholder. Rename requires a free pathname.
    Delete "$PayloadBackup"
    ${If} ${Errors}
      Abort "Unable to prepare the backup pathname: $PayloadBackup"
    ${EndIf}
    ClearErrors
    Rename "$PayloadTarget" "$PayloadBackup"
    ${If} ${Errors}
      Abort "Unable to replace the installed file: $PayloadTarget. The original file has been preserved."
    ${EndIf}
  ${EndIf}

  ; 'try' reports failure without an interactive Ignore button or delayed
  ; replacement that could leave the installed application incomplete.
  SetOverwrite try
  ClearErrors
  File /oname=$PayloadTarget "${BUNDLE_DIR}\${RelativePath}"
  SetOverwrite on
  ${If} ${Errors}
    Delete "$PayloadTarget"
    ${If} $PayloadBackup != ""
      ClearErrors
      Rename "$PayloadBackup" "$PayloadTarget"
      ${If} ${Errors}
        Abort "Unable to install $PayloadTarget or restore its original. The original remains at $PayloadBackup."
      ${EndIf}
    ${EndIf}
    Abort "Unable to install: $PayloadTarget. Any previous file has been restored."
  ${EndIf}

  ${If} $PayloadBackup != ""
    StrCpy $PayloadHadRebootFlag 0
    ${If} ${RebootFlag}
      StrCpy $PayloadHadRebootFlag 1
    ${EndIf}
    !ifdef KEYTAO_PAYLOAD_TEST_SKIP_REBOOT_DELETE
      ; Isolated fixtures must not write machine-wide pending reboot actions.
      ; Keep a mapped backup until the fixture unloads its own test DLL.
      Delete "$PayloadBackup"
    !else
      Delete /REBOOTOK "$PayloadBackup"
    !endif
    ; A retained old DLL is harmless. Its hosts can exit naturally; this file
    ; cleanup must not turn a successful upgrade into an immediate reboot.
    ${If} $PayloadHadRebootFlag == 0
      SetRebootFlag false
    ${Else}
      SetRebootFlag true
    ${EndIf}
    ClearErrors
  ${EndIf}
!macroend
!endif
