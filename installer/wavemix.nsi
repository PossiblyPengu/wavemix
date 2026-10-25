; wavemix installer. Build with:
;   makensis -DVERSION=0.8.0 -DEXE=path\to\wavemix.exe -DOUT=wavemix-setup.exe installer/wavemix.nsi
;
; Installs for the current user only, so it needs no administrator rights and never touches
; Program Files.
;
; Running a newer installer over an older install upgrades it: the old version is uninstalled
; silently first (so nothing from it is left behind), your settings and presets are kept, and the
; "start with Windows" and "desktop shortcut" choices come back as you had them. Running an older
; installer over a newer install asks first.

Unicode true
!include "MUI2.nsh"
!include "Sections.nsh"
!include "WordFunc.nsh"
!insertmacro VersionCompare

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
; What people see; APP is the file, folder and registry name.
!define NAME "Wavemix"
!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\${APP}"
!define RUN_KEY "Software\Microsoft\Windows\CurrentVersion\Run"

Name "${NAME} ${VERSION}"
OutFile "${OUT}"
InstallDir "$LOCALAPPDATA\Programs\${APP}"
InstallDirRegKey HKCU "${UNINSTALL_KEY}" "InstallLocation"
RequestExecutionLevel user
SetCompressor /SOLID lzma
BrandingText "${NAME} ${VERSION}"

; What an existing install looks like, read before anything is shown.
Var OldVersion
Var OldUninstaller
Var OldDir

!define MUI_ABORTWARNING
!define MUI_WELCOMEPAGE_TITLE "Install ${NAME}"
!define MUI_WELCOMEPAGE_TEXT "Wavemix is a volume control and audio mixer for Windows.$\r$\n$\r$\nSimple mode rises out of the taskbar when you click its tray icon. Pro mode is a full mixing console.$\r$\n$\r$\nIf Wavemix is already installed, this replaces it with version ${VERSION} and keeps your settings.$\r$\n$\r$\nIt installs just for you; no administrator rights are needed."
!define MUI_FINISHPAGE_RUN "$INSTDIR\${APP}.exe"
!define MUI_FINISHPAGE_RUN_TEXT "Start ${NAME} now"

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

; Hidden step that runs first: remove the previous version completely.
Section "-Remove the previous version" SecOld
  StrCmp $OldUninstaller "" done
  IfFileExists "$OldDir\uninstall.exe" 0 done
    !insertmacro StopApp
    ; Silent, and `_?=` makes it run in place so we can wait for it. A silent uninstall keeps the
    ; user's settings and presets.
    ExecWait '"$OldDir\uninstall.exe" /S _?=$OldDir'
    ; An uninstaller running in place can't delete itself.
    Delete "$OldDir\uninstall.exe"
    RMDir "$OldDir"
  done:
SectionEnd

Section "${NAME}" SecMain
  SectionIn RO
  !insertmacro StopApp
  SetOutPath "$INSTDIR"
  File "/oname=${APP}.exe" "${EXE}"
  WriteUninstaller "$INSTDIR\uninstall.exe"

  CreateShortcut "$SMPROGRAMS\${NAME}.lnk" "$INSTDIR\${APP}.exe"

  ; Add/Remove Programs entry.
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "${NAME}"
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
  CreateShortcut "$DESKTOP\${NAME}.lnk" "$INSTDIR\${APP}.exe"
SectionEnd

; Before anything is shown: find an existing install, refuse to downgrade by accident, and start
; the options the way the user left them.
Function .onInit
  ReadRegStr $OldVersion HKCU "${UNINSTALL_KEY}" "DisplayVersion"
  ReadRegStr $OldUninstaller HKCU "${UNINSTALL_KEY}" "UninstallString"
  ReadRegStr $OldDir HKCU "${UNINSTALL_KEY}" "InstallLocation"
  StrCmp $OldVersion "" first_install

  ; 0 = same, 1 = this installer is newer, 2 = this installer is older.
  ${VersionCompare} "${VERSION}" "$OldVersion" $0
  StrCmp $0 2 0 keep_choices
    IfSilent abort_older
    MessageBox MB_YESNO|MB_ICONEXCLAMATION "A newer version of ${NAME} ($OldVersion) is already installed.$\r$\n$\r$\nInstall the older version ${VERSION} over it?" IDYES keep_choices
  abort_older:
    Abort

  keep_choices:
    ; The old uninstaller is about to remove these, so note what the user had and offer the
    ; same again rather than switching things back on (or off) behind their back.
    ; (Labels, not relative jumps: the macros expand to several instructions.)
    ReadRegStr $1 HKCU "${RUN_KEY}" "${APP}"
    StrCmp $1 "" 0 had_startup
      !insertmacro UnselectSection ${SecStartup}
    had_startup:
    IfFileExists "$DESKTOP\${APP}.lnk" had_desktop 0
      !insertmacro UnselectSection ${SecDesktop}
    had_desktop:
  first_install:
FunctionEnd

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

  ; Settings and presets live in %APPDATA%\wavemix. Keep them unless asked (and always when run
  ; silently, as during an upgrade), so a reinstall picks up where you left off.
  IfSilent +3
  MessageBox MB_YESNO|MB_ICONQUESTION "Also delete your ${NAME} settings and presets?" IDNO +2
  RMDir /r "$APPDATA\${APP}"
SectionEnd
