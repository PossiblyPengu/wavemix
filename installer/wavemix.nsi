; wavemix installer. Build with:
;   makensis -DVERSION=0.7.0 -DEXE=path\to\wavemix.exe -DOUT=wavemix-setup.exe installer/wavemix.nsi
;
; Installs for the current user only, so it needs no administrator rights and never touches
; Program Files.

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef EXE
  !define EXE "wavemix.exe"
!endif
!ifndef OUT
  !define OUT "wavemix-setup.exe"
!endif

!define APP "wavemix"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${APP} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "${APP} ${VERSION}"

!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install ${APP}"
!define MUI_WELCOMEPAGE_TEXT "wavemix is a volume control and audio mixer for Windows.$\r$\n$\r$\nSimple mode rises out of the taskbar when you click its tray icon. Pro mode is a full mixing console.$\r$\n$\r$\nIt installs just for you; no administrator rights are needed."
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP}.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start wavemix now"

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

; A running copy would keep the exe locked, so close it first (upgrades and uninstalls).
!macro StopApp
  nsExec::Exec 'taskkill /F /IM ${APP}.exe'
  Pop $0
  Sleep 400
!macroend

Section "wavemix" SecMain
  SectionIn RO
  !insertmacro StopApp
  SetOutPath "$INSTDIR"
  File "/oname=${APP}.exe" "${EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${APP}.lnk" "$INSTDIR\${APP}.exe"

  ; Add/Remove Programs entry.
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${APP}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\${APP}.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  SectionGetSize ${SecMain} $0
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "EstimatedSize" $0
SectionEnd

Section "Start with Windows" SecStartup
  ; Same value the app's own "Start with Windows" setting writes, so the two agree.
  WriteRegStr HKCU "${RUN_KEY}" "${APP}" '"$INSTDIR\${APP}.exe" --background'
SectionEnd

Section "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\${APP}.lnk" "$INSTDIR\${APP}.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecMain} "The program."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecStartup} "Run quietly in the tray when you sign in to Windows."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "Add a shortcut to the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  !insertmacro StopApp
  Delete "$INSTDIR\${APP}.exe"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\${APP}.lnk"
  Delete "$DESKTOP\${APP}.lnk"
  DeleteRegValue HKCU "${RUN_KEY}" "${APP}"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"

  ; Settings and presets live in %APPDATA%\wavemix. Keep them unless asked, so a reinstall
  ; picks up where you left off.
  IfSilent +3
  MessageBox MB_YESNO|MB_ICONQUESTION "Also delete your wavemix settings and presets?" IDNO +2
  RMDir /r "$APPDATA\${APP}"
SectionEnd
