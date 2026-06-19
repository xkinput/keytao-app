!include LogicLib.nsh
!include x64.nsh

!define KEYTAO_IME_DLL "$INSTDIR\keytao-windows-ime-runtime\current\keytao_windows_ime.dll"

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Registering KeyTao Windows input method..."
  ${IfNot} ${FileExists} "${KEYTAO_IME_DLL}"
    MessageBox MB_ICONSTOP|MB_OK "KeyTao input method runtime is missing: ${KEYTAO_IME_DLL}"
    Abort
  ${EndIf}

  ClearErrors
  ${If} ${RunningX64}
    SetRegView 64
    ${DisableX64FSRedirection}
  ${EndIf}
  ExecWait '"$WINDIR\System32\regsvr32.exe" /s "${KEYTAO_IME_DLL}"' $0
  ${If} ${RunningX64}
    ${EnableX64FSRedirection}
  ${EndIf}
  ${If} $0 != 0
    MessageBox MB_ICONSTOP|MB_OK "Failed to register KeyTao input method (regsvr32 exit code $0)."
    Abort
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} ${FileExists} "${KEYTAO_IME_DLL}"
    DetailPrint "Unregistering KeyTao Windows input method..."
    ClearErrors
    ${If} ${RunningX64}
      SetRegView 64
      ${DisableX64FSRedirection}
    ${EndIf}
    ExecWait '"$WINDIR\System32\regsvr32.exe" /u /s "${KEYTAO_IME_DLL}"' $0
    ${If} ${RunningX64}
      ${EnableX64FSRedirection}
    ${EndIf}
    ${If} $0 != 0
      MessageBox MB_ICONSTOP|MB_OK "Failed to unregister KeyTao input method (regsvr32 exit code $0)."
      Abort
    ${EndIf}
  ${EndIf}
!macroend
