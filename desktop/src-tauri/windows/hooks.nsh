!define JACK_HOOK_DIR "${__FILEDIR__}"

!macro JACK_EXTRACT_SETUP
  InitPluginsDir
  SetOutPath "$PLUGINSDIR"
  File "/oname=steelseries-gg.ps1" "${JACK_HOOK_DIR}\steelseries-gg.ps1"
!macroend

!macro JACK_STOP_FOR_INSTALL
  ${If} ${FileExists} "$INSTDIR\jack-desktop.exe"
    !insertmacro JACK_EXTRACT_SETUP
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\steelseries-gg.ps1" -Mode StopJack -JackPath "$INSTDIR\jack-desktop.exe"'
    Pop $R0
    Pop $R3
    ${If} $R0 != 0
      IfSilent +2
        MessageBox MB_OK|MB_ICONEXCLAMATION "Quit Jack from its settings, then retry. Audio routes could not be restored."
      SetErrorLevel 1
      Abort
    ${EndIf}
    SetOutPath "$INSTDIR"
  ${EndIf}
!macroend

!macro NSIS_HOOK_PREINSTALL
  ; Existing Jack installations use the recovery screen if GG disappears later.
  ; An app update must never install GG or request its consent again.
  ${IfNot} ${FileExists} "$INSTDIR\jack-desktop.exe"
    !insertmacro JACK_EXTRACT_SETUP
    StrCpy $R1 "EnsureInstalled"
    IfSilent 0 +2
      StrCpy $R1 "RequireInstalled"
    StrCpy $R2 ""
    ${If} $LANGUAGE == 1042
      StrCpy $R2 "-Korean"
    ${EndIf}
    jack_gg_retry:
    nsExec::ExecToStack '"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -File "$PLUGINSDIR\steelseries-gg.ps1" -Mode $R1 $R2'
    Pop $R0
    Pop $R3
    ${If} $R0 != 0
      ${If} $R0 == 20
        SetErrorLevel 20
        Abort
      ${EndIf}
      IfSilent jack_gg_abort
      MessageBox MB_RETRYCANCEL|MB_ICONEXCLAMATION "SteelSeries GG setup could not be completed. Retry, or cancel Jack installation." IDRETRY jack_gg_retry
      jack_gg_abort:
      SetErrorLevel 1
      Abort
    ${EndIf}
    SetOutPath "$INSTDIR"
  ${EndIf}
  !insertmacro JACK_STOP_FOR_INSTALL
!macroend

!macro NSIS_HOOK_PREUNINSTALL
  !insertmacro JACK_STOP_FOR_INSTALL
!macroend

!macro NSIS_HOOK_POSTUNINSTALL
  ReadRegStr $R0 HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Jack"
  ${If} $R0 == '$\"$INSTDIR\jack-desktop.exe$\" --background'
    DeleteRegValue HKCU "Software\Microsoft\Windows\CurrentVersion\Run" "Jack"
  ${EndIf}
  ; GG and the user's Jack settings are intentionally retained.
!macroend
