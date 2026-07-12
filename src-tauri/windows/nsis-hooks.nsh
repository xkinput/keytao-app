!include LogicLib.nsh
!include x64.nsh

!define KEYTAO_IME_DLL_NATIVE "$INSTDIR\keytao-windows-ime-runtime\current\keytao_windows_ime.dll"
!define KEYTAO_IME_X86_SOURCE_DIR "$INSTDIR\keytao-windows-ime-runtime\x86"
!define KEYTAO_IME_X86_INSTALL_DIR "$PROGRAMFILES32\KeyTao\keytao-windows-ime-runtime\x86"
!define KEYTAO_IME_DLL_X86 "${KEYTAO_IME_X86_INSTALL_DIR}\keytao_windows_ime.dll"

!macro NSIS_HOOK_POSTINSTALL
  DetailPrint "Scheduling KeyTao Windows input method registration..."
  ${IfNot} ${FileExists} "${KEYTAO_IME_DLL_NATIVE}"
    DetailPrint "KeyTao input method runtime is missing: ${KEYTAO_IME_DLL_NATIVE}"
    Return
  ${EndIf}

  ${If} ${RunningX64}
    ${If} ${FileExists} "${KEYTAO_IME_X86_SOURCE_DIR}\keytao_windows_ime.dll"
      DetailPrint "Installing the KeyTao x86 text service in Program Files (x86)..."
      CreateDirectory "${KEYTAO_IME_X86_INSTALL_DIR}"
      ClearErrors
      ExecWait '"$WINDIR\System32\robocopy.exe" "${KEYTAO_IME_X86_SOURCE_DIR}" "${KEYTAO_IME_X86_INSTALL_DIR}" /E /COPY:DAT /DCOPY:DAT /R:2 /W:1 /NFL /NDL /NJH /NJS /NP' $2
      ${If} ${Errors}
        DetailPrint "KeyTao x86 input method runtime copy failed to start."
      ${ElseIf} $2 > 7
        DetailPrint "KeyTao x86 input method runtime copy failed with robocopy exit code $2."
      ${Else}
        ${IfNot} ${FileExists} "${KEYTAO_IME_DLL_X86}"
          DetailPrint "KeyTao x86 input method runtime copy did not produce ${KEYTAO_IME_DLL_X86}."
        ${Else}
          ClearErrors
          SetRegView 32
          ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "${KEYTAO_IME_DLL_X86}"' $1
          ${If} ${Errors}
            DetailPrint "KeyTao x86 input method registration failed to start."
          ${ElseIf} $1 != 0
            DetailPrint "KeyTao x86 input method registration failed with exit code $1."
          ${EndIf}
          SetRegView 64
        ${EndIf}
      ${EndIf}
    ${ElseIf} ${FileExists} "${KEYTAO_IME_DLL_X86}"
      ClearErrors
      SetRegView 32
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "${KEYTAO_IME_DLL_X86}"' $1
      ${If} ${Errors}
        DetailPrint "KeyTao x86 input method registration failed to start."
      ${ElseIf} $1 != 0
        DetailPrint "KeyTao x86 input method registration failed with exit code $1."
      ${EndIf}
      SetRegView 64
    ${Else}
      DetailPrint "KeyTao x86 input method runtime is missing: ${KEYTAO_IME_X86_SOURCE_DIR}"
    ${EndIf}
  ${EndIf}

  ClearErrors
  ${If} ${RunningX64}
    SetRegView 64
    ${DisableX64FSRedirection}
  ${EndIf}
  ExecWait '"$WINDIR\System32\regsvr32.exe" /s "${KEYTAO_IME_DLL_NATIVE}"' $0
  ${If} ${RunningX64}
    ${EnableX64FSRedirection}
  ${EndIf}
  ${If} ${Errors}
    DetailPrint "KeyTao input method registration will be retried by the app."
  ${ElseIf} $0 != 0
    DetailPrint "KeyTao native input method registration failed with exit code $0; it will be retried by the app."
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} ${RunningX64}
    ${If} ${FileExists} "${KEYTAO_IME_DLL_X86}"
      DetailPrint "Unregistering KeyTao x86 Windows input method..."
      ClearErrors
      SetRegView 32
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /u /s "${KEYTAO_IME_DLL_X86}"' $1
      ${If} $1 != 0
        DetailPrint "Failed to unregister KeyTao x86 input method (regsvr32 exit code $1)."
      ${EndIf}
      SetRegView 64
    ${ElseIf} ${FileExists} "${KEYTAO_IME_X86_SOURCE_DIR}\keytao_windows_ime.dll"
      DetailPrint "Unregistering the legacy KeyTao x86 Windows input method path..."
      ClearErrors
      SetRegView 32
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /u /s "${KEYTAO_IME_X86_SOURCE_DIR}\keytao_windows_ime.dll"' $1
      SetRegView 64
    ${EndIf}
  ${EndIf}

  ${If} ${FileExists} "${KEYTAO_IME_DLL_NATIVE}"
    DetailPrint "Unregistering KeyTao Windows input method..."
    ClearErrors
    ${If} ${RunningX64}
      SetRegView 64
      ${DisableX64FSRedirection}
    ${EndIf}
    ExecWait '"$WINDIR\System32\regsvr32.exe" /u /s "${KEYTAO_IME_DLL_NATIVE}"' $0
    ${If} ${RunningX64}
      ${EnableX64FSRedirection}
    ${EndIf}
    ${If} $0 != 0
      DetailPrint "Failed to unregister KeyTao input method (regsvr32 exit code $0)."
    ${EndIf}
  ${EndIf}

  ${If} ${RunningX64}
    RMDir /r /REBOOTOK "${KEYTAO_IME_X86_INSTALL_DIR}"
    RMDir /REBOOTOK "$PROGRAMFILES32\KeyTao\keytao-windows-ime-runtime"
    RMDir /REBOOTOK "$PROGRAMFILES32\KeyTao"
  ${EndIf}
!macroend
