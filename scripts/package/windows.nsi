; PilotHouse installer for Windows: installs for the current user (no admin),
; adds Start menu and optional desktop shortcuts, and an uninstaller.
; Built by scripts/package/windows.ps1 with `makensis /DVERSION=x.y.z`.

Unicode true
!include "MUI2.nsh"

!ifndef VERSION
  !define VERSION "0.0.0"
!endif
!ifndef NUMERIC
  !define NUMERIC "0.0.0"
!endif

Name "PilotHouse"
OutFile "..\..\dist\PilotHouse-Windows-Setup-x64.exe"
InstallDir "$LOCALAPPDATA\Programs\PilotHouse"
InstallDirRegKey HKCU "Software\PilotHouse" "InstallDir"
RequestExecutionLevel user
SetCompressor /SOLID lzma
VIProductVersion "${NUMERIC}.0"
VIAddVersionKey "ProductName" "PilotHouse"
VIAddVersionKey "FileDescription" "PilotHouse installer"
VIAddVersionKey "FileVersion" "${NUMERIC}"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "LegalCopyright" "Apache License 2.0"

!define MUI_ICON "..\..\crates\app\assets\icon\PilotHouse.ico"
!define MUI_UNICON "..\..\crates\app\assets\icon\PilotHouse.ico"
!define MUI_FINISHPAGE_RUN "$INSTDIR\pilothouse.exe"
!insertmacro MUI_PAGE_LICENSE "..\..\LICENSE"
!insertmacro MUI_PAGE_DIRECTORY
!insertmacro MUI_PAGE_COMPONENTS
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

!define UNINSTALL_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\PilotHouse"

Section "PilotHouse" SecApp
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "..\..\target\release\pilothouse.exe"
  File "..\..\LICENSE"
  File "..\..\NOTICE"
  WriteUninstaller "$INSTDIR\Uninstall.exe"
  WriteRegStr HKCU "Software\PilotHouse" "InstallDir" "$INSTDIR"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayName" "PilotHouse"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "Publisher" "LibreGuild"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "DisplayIcon" "$INSTDIR\pilothouse.exe"
  WriteRegStr HKCU "${UNINSTALL_KEY}" "UninstallString" '"$INSTDIR\Uninstall.exe"'
  WriteRegStr HKCU "${UNINSTALL_KEY}" "URLInfoAbout" "https://github.com/LibreGuild/PilotHouse"
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoModify" 1
  WriteRegDWORD HKCU "${UNINSTALL_KEY}" "NoRepair" 1
  CreateDirectory "$SMPROGRAMS\PilotHouse"
  CreateShortcut "$SMPROGRAMS\PilotHouse\PilotHouse.lnk" "$INSTDIR\pilothouse.exe"
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\PilotHouse.lnk" "$INSTDIR\pilothouse.exe"
SectionEnd

Section "Uninstall"
  ; The app's data (chats, settings) stays in %APPDATA%\PilotHouse.
  Delete "$INSTDIR\pilothouse.exe"
  Delete "$INSTDIR\LICENSE"
  Delete "$INSTDIR\NOTICE"
  Delete "$INSTDIR\Uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\PilotHouse\PilotHouse.lnk"
  RMDir "$SMPROGRAMS\PilotHouse"
  Delete "$DESKTOP\PilotHouse.lnk"
  DeleteRegKey HKCU "${UNINSTALL_KEY}"
  DeleteRegKey HKCU "Software\PilotHouse"
SectionEnd
