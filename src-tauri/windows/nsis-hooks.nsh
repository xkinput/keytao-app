!include LogicLib.nsh
!include x64.nsh

!define KEYTAO_IME_DLL "$INSTDIR\keytao-windows-ime-runtime\current\keytao_windows_ime.dll"

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Scheduling KeyTao Windows input method registration..."
  ${IfNot} ${FileExists} "${KEYTAO_IME_DLL}"
    DetailPrint "KeyTao input method runtime is missing: ${KEYTAO_IME_DLL}"
    Return
  ${EndIf}

  ClearErrors
  ${If} ${RunningX64}
    SetRegView 64
    ${DisableX64FSRedirection}
  ${EndIf}
  Exec '"$WINDIR\System32\regsvr32.exe" /s "${KEYTAO_IME_DLL}"'
  ${If} ${RunningX64}
    ${EnableX64FSRedirection}
  ${EndIf}
  ${If} ${Errors}
    DetailPrint "KeyTao input method registration will be retried by the app."
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
      DetailPrint "Failed to unregister KeyTao input method (regsvr32 exit code $0)."
    ${EndIf}
  ${EndIf}
!macroend
