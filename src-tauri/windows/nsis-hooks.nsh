!include LogicLib.nsh
!include FileFunc.nsh
!include x64.nsh

!define KEYTAO_IME_STAGING_NATIVE "$INSTDIR\keytao-windows-ime-runtime\current"
!define KEYTAO_IME_STAGING_ARM64X "$INSTDIR\keytao-windows-ime-runtime\arm64x"
!define KEYTAO_IME_STAGING_X86 "$INSTDIR\keytao-windows-ime-runtime\x86"
!define KEYTAO_IME_REG_KEY "Software\KeyTao"
!define KEYTAO_IME_LEGACY_X86_DIR "$PROGRAMFILES32\KeyTao\keytao-windows-ime-runtime\x86"

!macro NSIS_HOOK_POSTINSTALL
  ${If} ${RunningX64}
    SetRegView 64
  ${EndIf}
  ReadEnvStr $R6 "ProgramData"
  ${If} $R6 == ""
    StrCpy $R6 "$WINDIR\..\ProgramData"
  ${EndIf}
  WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "preparing"
  DetailPrint "Preparing a versioned KeyTao Windows input method runtime..."
  ${If} ${IsNativeARM64}
    StrCpy $4 "${KEYTAO_IME_STAGING_ARM64X}"
    StrCpy $5 "arm64x"
  ${Else}
    StrCpy $4 "${KEYTAO_IME_STAGING_NATIVE}"
    StrCpy $5 "current"
  ${EndIf}
  ${IfNot} ${FileExists} "$4\keytao_windows_ime.dll"
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "missing staging runtime: $4"
    DetailPrint "KeyTao input method staging runtime is missing: $4"
    Return
  ${EndIf}

  ClearErrors
  GetTempFileName $R7
  ${If} ${Errors}
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "unable to allocate runtime directory"
    DetailPrint "Unable to allocate a versioned KeyTao input method runtime directory."
    Return
  ${EndIf}
  ${GetFileName} "$R7" $R8
  Delete "$R7"
  StrCpy $R7 "$R6\KeyTao\keytao-windows-ime-runtime\${VERSION}\$R8"
  ClearErrors
  CreateDirectory "$R7"
  ${If} ${Errors}
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "unable to create runtime directory: $R7"
    DetailPrint "Unable to create the KeyTao input method runtime directory: $R7"
    Return
  ${EndIf}
  WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeRuntimeDir" "$R7"
  StrCpy $R8 "$R7\$5"
  StrCpy $3 "$R8\keytao_windows_ime.dll"
  StrCpy $R9 ""

  ClearErrors
  ExecWait '"$WINDIR\System32\robocopy.exe" "$4" "$R8" /E /COPY:DAT /DCOPY:DAT /R:2 /W:1 /NFL /NDL /NJH /NJS /NP' $2
  ${If} ${Errors}
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "native runtime copy failed to start"
    DetailPrint "KeyTao native input method runtime copy failed to start."
    RMDir /r /REBOOTOK "$R7"
    Return
  ${ElseIf} $2 > 7
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "native runtime copy failed: $2"
    DetailPrint "KeyTao native input method runtime copy failed with robocopy exit code $2."
    RMDir /r /REBOOTOK "$R7"
    Return
  ${ElseIfNot} ${FileExists} "$3"
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "native runtime copy is incomplete"
    DetailPrint "KeyTao native input method runtime copy did not produce $3."
    RMDir /r /REBOOTOK "$R7"
    Return
  ${EndIf}

  ${If} ${RunningX64}
    ${If} ${FileExists} "${KEYTAO_IME_STAGING_X86}\keytao_windows_ime.dll"
      DetailPrint "Installing the KeyTao x86 text service in the versioned runtime..."
      StrCpy $6 "$R7\x86"
      ClearErrors
      ExecWait '"$WINDIR\System32\robocopy.exe" "${KEYTAO_IME_STAGING_X86}" "$6" /E /COPY:DAT /DCOPY:DAT /R:2 /W:1 /NFL /NDL /NJH /NJS /NP' $2
      ${If} ${Errors}
        DetailPrint "KeyTao x86 input method runtime copy failed to start."
      ${ElseIf} $2 > 7
        DetailPrint "KeyTao x86 input method runtime copy failed with robocopy exit code $2."
      ${ElseIfNot} ${FileExists} "$6\keytao_windows_ime.dll"
        DetailPrint "KeyTao x86 input method runtime copy did not produce its COM server."
      ${Else}
        StrCpy $R9 "$6\keytao_windows_ime.dll"
        ClearErrors
        SetRegView 32
        ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /s "$R9"' $1
        ${If} ${Errors}
          DetailPrint "KeyTao x86 input method registration failed to start."
        ${ElseIf} $1 != 0
          DetailPrint "KeyTao x86 input method registration failed with exit code $1."
        ${EndIf}
        SetRegView 64
      ${EndIf}
    ${Else}
      DetailPrint "KeyTao x86 input method staging runtime is missing."
    ${EndIf}
  ${EndIf}

  DetailPrint "Registering the KeyTao native text service..."
  ClearErrors
  ${If} ${RunningX64}
    SetRegView 64
    ${DisableX64FSRedirection}
  ${EndIf}
  ExecWait '"$WINDIR\System32\regsvr32.exe" /s "$3"' $0
  ${If} ${RunningX64}
    ${EnableX64FSRedirection}
  ${EndIf}
  ${If} ${Errors}
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "native registration failed to start"
    DetailPrint "KeyTao input method registration will be retried by the app."
  ${ElseIf} $0 != 0
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "native registration failed: $0"
    DetailPrint "KeyTao native input method registration failed with exit code $0; it will be retried by the app."
  ${Else}
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus" "registered"
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeRuntimeDir" "$R7"
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeNativeDll" "$3"
    WriteRegStr HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeX86Dll" "$R9"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  ${If} ${RunningX64}
    SetRegView 64
  ${EndIf}
  ReadRegStr $R7 HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeRuntimeDir"
  ReadRegStr $3 HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeNativeDll"
  ReadRegStr $R9 HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeX86Dll"

  ${If} $3 == ""
    ${If} ${IsNativeARM64}
      StrCpy $3 "${KEYTAO_IME_STAGING_ARM64X}\keytao_windows_ime.dll"
    ${Else}
      StrCpy $3 "${KEYTAO_IME_STAGING_NATIVE}\keytao_windows_ime.dll"
    ${EndIf}
  ${EndIf}
  ${If} $R9 == ""
    StrCpy $R9 "${KEYTAO_IME_LEGACY_X86_DIR}\keytao_windows_ime.dll"
  ${EndIf}

  ${If} ${RunningX64}
    ${If} ${FileExists} "$R9"
      DetailPrint "Unregistering KeyTao x86 Windows input method..."
      ClearErrors
      SetRegView 32
      ExecWait '"$WINDIR\SysWOW64\regsvr32.exe" /u /s "$R9"' $1
      ${If} $1 != 0
        DetailPrint "Failed to unregister KeyTao x86 input method (regsvr32 exit code $1)."
      ${EndIf}
      SetRegView 64
    ${EndIf}
  ${EndIf}

  ${If} ${FileExists} "$3"
    DetailPrint "Unregistering KeyTao Windows input method..."
    ClearErrors
    ${If} ${RunningX64}
      SetRegView 64
      ${DisableX64FSRedirection}
    ${EndIf}
    ExecWait '"$WINDIR\System32\regsvr32.exe" /u /s "$3"' $0
    ${If} ${RunningX64}
      ${EnableX64FSRedirection}
    ${EndIf}
    ${If} $0 != 0
      DetailPrint "Failed to unregister KeyTao input method (regsvr32 exit code $0)."
    ${EndIf}
  ${EndIf}

  ${If} $R7 != ""
    RMDir /r /REBOOTOK "$R7"
  ${EndIf}
  RMDir /r /REBOOTOK "${KEYTAO_IME_LEGACY_X86_DIR}"
  RMDir /REBOOTOK "$PROGRAMFILES32\KeyTao\keytao-windows-ime-runtime"
  RMDir /REBOOTOK "$PROGRAMFILES32\KeyTao"
  DeleteRegValue HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeRuntimeDir"
  DeleteRegValue HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeNativeDll"
  DeleteRegValue HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeX86Dll"
  DeleteRegValue HKLM "${KEYTAO_IME_REG_KEY}" "WindowsImeInstallStatus"
  DeleteRegKey /ifempty HKLM "${KEYTAO_IME_REG_KEY}"
!macroend
