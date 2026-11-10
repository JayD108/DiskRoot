; Xylem installer (NSIS 3, zlib license; LZMA under CPL: all OSI-approved).
; Built by tools\release.ps1 (locally or in GitHub Actions):
;   makensis /DVERSION=0.1.0 /DSRCDIR=<folder with the exes> /DOUTDIR=<dist> installer\xylem.nsi
;
; Why an installer matters for security: Fast scan starts xylem.exe with
; admin rights. Installed in Program Files, only administrators can replace
; that exe, so malware running as the user can't swap it before the UAC
; prompt.
;
; The uninstaller removes exactly the files it installed (never a
; recursive delete). It leaves the user's settings (%APPDATA%\Xylem) and
; deletion log (%LOCALAPPDATA%\Xylem) in place; README explains how to
; remove them.

Unicode true
!ifndef VERSION
  !define VERSION "0.1.0"
!endif
!ifndef SRCDIR
  !define SRCDIR "..\target\release"
!endif
!ifndef OUTDIR
  !define OUTDIR "..\dist"
!endif
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\Xylem"

Name "Xylem"
OutFile "${OUTDIR}\xylem-setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\Xylem"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
ManifestDPIAware true
ShowInstDetails show
ShowUninstDetails show
BrandingText "Xylem ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "Xylem"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "Xylem installer"
VIAddVersionKey "CompanyName" "Xylem contributors"
VIAddVersionKey "LegalCopyright" "MIT OR Apache-2.0"

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"

!define MUI_ICON "..\crates\native\assets\icon.ico"
!define MUI_UNICON "..\crates\native\assets\icon.ico"
!define MUI_ABORTWARNING
; "Start Xylem" must not inherit the installer's admin rights: launch it
; through Explorer, which runs it as the signed-in user (not elevated).
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start Xylem"
!define MUI_FINISHPAGE_RUN_FUNCTION StartUnelevated

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE-MIT"
!insertmacro MUI_PAGE_COMPONENTS
; No directory page: Xylem always goes to Program Files, which normal users
; can't modify. A user-writable folder would let malware replace xylem.exe,
; which Fast scan runs as admin.
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function StartUnelevated
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\xylem.exe"'
FunctionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "Xylem needs 64-bit Windows 10 or 11." /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext all
  ; Ignore /D=... too: always the protected Program Files folder.
  StrCpy $INSTDIR "$PROGRAMFILES64\Xylem"
FunctionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section "Xylem (required)" SecApp
  SectionIn RO
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\xylem.exe"
  File "..\README.md"
  File "..\LICENSE-MIT"
  File "..\LICENSE-APACHE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\Xylem.lnk" "$INSTDIR\xylem.exe" "" "$INSTDIR\xylem.exe" 0 SW_SHOWNORMAL "" "See what's filling your drive"

  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "Xylem"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "Xylem contributors"
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "https://github.com/JayD108/Xylem"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\xylem.exe"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" 7500
SectionEnd

Section "Command-line tool (xylem-cli)" SecCli
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\xylem-cli.exe"
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\Xylem.lnk" "$INSTDIR\xylem.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "The Xylem app, including the Fast scan helper."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCli} "xylem-cli.exe: scan from the command line and export CSV."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A Xylem shortcut on the desktop."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  Delete "$INSTDIR\xylem.exe"
  Delete "$INSTDIR\xylem-cli.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\Xylem.lnk"
  Delete "$DESKTOP\Xylem.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
SectionEnd
