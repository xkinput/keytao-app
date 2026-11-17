!ifndef KEYTAO_UNELEVATED_LAUNCH_INCLUDED
!define KEYTAO_UNELEVATED_LAUNCH_INCLUDED
!include "LogicLib.nsh"
!if ${NSIS_PTR_SIZE} != 4
  !error "KeyTao's installer requires the default x86 NSIS target."
!endif

Function LaunchKeyTaoAsUser
  ${If} ${Silent}
    Return
  ${EndIf}
  System::Store "S"
  StrCpy $1 0
  StrCpy $2 0
  StrCpy $3 0
  StrCpy $4 0
  StrCpy $5 0
  StrCpy $6 0
  StrCpy $7 0
  StrCpy $8 0
  StrCpy $9 0

  ; Reuse the IME server's shell-token approach, never the installer's token.
  System::Call 'user32::GetShellWindow() p .r0'
  StrCmp $0 0 done
  System::Call 'user32::GetWindowThreadProcessId(p r0, *i .r0)'
  StrCmp $0 0 done
  System::Call 'kernel32::OpenProcess(i 0x1000, i 0, i r0) p .r1'
  StrCmp $1 0 done
  System::Call 'advapi32::OpenProcessToken(p r1, i 0xA, *p .r2) i .r0'
  StrCmp $0 0 done
  ; TokenElevation must be queried successfully and report a non-elevated shell.
  System::Call 'advapi32::GetTokenInformation(p r2, i 20, *i .r0, i 4, *i .R0) i .R1'
  StrCmp $R1 0 done
  StrCmp $0 0 0 done
  System::Call 'advapi32::DuplicateTokenEx(p r2, i 0x18B, p 0, i 2, i 1, *p .r3) i .r0'
  StrCmp $0 0 done

  System::Call 'advapi32::OpenProcessToken(p -1, i 0x20, *p .r4) i .r0'
  StrCmp $0 0 done
  System::Call 'advapi32::LookupPrivilegeValueW(p 0, w "SeImpersonatePrivilege", *l .R0) i .r0'
  StrCmp $0 0 done
  System::Call '*(i 1, l R0, i 2) p .r5'
  StrCmp $5 0 done
  System::Call 'advapi32::AdjustTokenPrivileges(p r4, i 0, p r5, i 0, p 0, p 0) i .r0 ?e'
  Pop $R0
  StrCmp $0 0 done
  StrCmp $R0 0 0 done

  ; The target user's profile supplies LOCALAPPDATA and all other app paths.
  System::Call 'userenv::CreateEnvironmentBlock(*p .r8, p r3, i 0) i .r0'
  StrCmp $0 0 done
  System::Call '*(i 68, p 0, p 0, p 0, i 0, i 0, i 0, i 0, i 0, i 0, i 0, i 0, &i2 0, &i2 0, p 0, p 0, p 0, p 0) p .r6'
  System::Call '*(p 0, p 0, i 0, i 0) p .r7'
  StrCmp $6 0 done
  StrCmp $7 0 done
  System::Call 'advapi32::CreateProcessWithTokenW(p r3, i 0, w "$INSTDIR\keytao-app.exe", w "$\"$INSTDIR\keytao-app.exe$\"", i 0x400, p r8, w "$INSTDIR", p r6, p r7) i .r9'
  ${If} $9 != 0
    System::Call '*$7(p .R0, p .R1)'
    System::Call 'kernel32::CloseHandle(p R0)'
    System::Call 'kernel32::CloseHandle(p R1)'
  ${EndIf}

  done:
  ${If} $8 != 0
    System::Call 'userenv::DestroyEnvironmentBlock(p r8)'
  ${EndIf}
  System::Free $7
  System::Free $6
  System::Free $5
  ${If} $4 != 0
    System::Call 'kernel32::CloseHandle(p r4)'
  ${EndIf}
  ${If} $3 != 0
    System::Call 'kernel32::CloseHandle(p r3)'
  ${EndIf}
  ${If} $2 != 0
    System::Call 'kernel32::CloseHandle(p r2)'
  ${EndIf}
  ${If} $1 != 0
    System::Call 'kernel32::CloseHandle(p r1)'
  ${EndIf}
  ${If} $9 == 0
    ; Fail closed: a missing or elevated shell must never launch an elevated app.
    DetailPrint "KeyTao was not launched because a non-elevated user launch was unavailable."
  ${EndIf}
  System::Store "L"
FunctionEnd
!endif
