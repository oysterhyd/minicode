!macro customInstall
  CreateShortCut "$SMPROGRAMS\MiniCode CLI.lnk" "$SYSDIR\cmd.exe" '/K ""$INSTDIR\minicode.cmd" --help"'
!macroend

!macro customUnInstall
  Delete "$SMPROGRAMS\MiniCode CLI.lnk"
!macroend
