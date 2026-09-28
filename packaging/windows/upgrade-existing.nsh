!ifndef KEYTAO_UPGRADE_EXISTING_INCLUDED
!define KEYTAO_UPGRADE_EXISTING_INCLUDED

Function PrepareExistingInstallation
  ${If} ${FileExists} "$INSTDIR\flutter_windows.dll"
  ${AndIf} ${FileExists} "$INSTDIR\keytao_app_bridge.dll"
    ; Flutter upgrades replace the owned payload in place. This also repairs
    ; partial upgrades and avoids invoking alpha.90 uninstallers that wrongly
    ; reject browsers holding TIP DLLs. Preserve the complete old versioned
    ; runtime, including dictionaries, for hosts that remain open. POSTINSTALL
    ; registers a fresh runtime directory even when reinstalling this version.
    ; Do not require data/app.so: interrupted copies can leave it missing.
    DetailPrint "Updating KeyTao in place; keeping existing input method runtimes available to open applications..."
  ${ElseIf} ${FileExists} "$INSTDIR\uninstall.exe"
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
FunctionEnd
!endif
