; DiskRoot installer (NSIS 3, zlib license; LZMA under CPL: all OSI-approved).
; Built by tools\release.ps1 (locally or in GitHub Actions):
;   makensis /DVERSION=0.1.0 /DSRCDIR=<folder with the exes> /DOUTDIR=<dist> installer\diskroot.nsi
;
; Why an installer matters for security: Fast scan starts diskroot.exe with
; admin rights. Installed in Program Files, only administrators can replace
; that exe, so malware running as the user can't swap it before the UAC
; prompt.
;
; The uninstaller removes exactly the files it installed (never a
; recursive delete). It leaves the user's settings (%APPDATA%\DiskRoot) and
; deletion log (%LOCALAPPDATA%\DiskRoot) in place; README explains how to
; remove them.

Unicode true
!ifndef VERSION
  !define VERSION "0.6.1"
!endif
!ifndef SRCDIR
  !define SRCDIR "..\target\release"
!endif
!ifndef OUTDIR
  !define OUTDIR "..\dist"
!endif
!define UNINST_KEY "Software\Microsoft\Windows\CurrentVersion\Uninstall\DiskRoot"

Name "DiskRoot"
OutFile "${OUTDIR}\diskroot-setup-${VERSION}.exe"
InstallDir "$PROGRAMFILES64\DiskRoot"
RequestExecutionLevel admin
SetCompressor /SOLID lzma
ManifestDPIAware true
ShowInstDetails show
ShowUninstDetails show
BrandingText "DiskRoot ${VERSION}"

VIProductVersion "${VERSION}.0"
VIAddVersionKey "ProductName" "DiskRoot"
VIAddVersionKey "ProductVersion" "${VERSION}"
VIAddVersionKey "FileVersion" "${VERSION}"
VIAddVersionKey "FileDescription" "DiskRoot installer"
VIAddVersionKey "CompanyName" "DiskRoot contributors"
VIAddVersionKey "LegalCopyright" "MIT OR Apache-2.0"

!include "MUI2.nsh"
!include "x64.nsh"
!include "LogicLib.nsh"
!include "WinMessages.nsh"

!define MUI_ICON "..\crates\native\assets\icon.ico"
!define MUI_UNICON "..\crates\native\assets\icon.ico"
; The logo on the welcome and finish pages (164x314 BMP from tools/make-icons.py).
!define MUI_WELCOMEFINISHPAGE_BITMAP "welcome.bmp"
!define MUI_UNWELCOMEFINISHPAGE_BITMAP "welcome.bmp"
!define MUI_ABORTWARNING
; "Start DiskRoot" must not inherit the installer's admin rights: launch it
; through Explorer, which runs it as the signed-in user (not elevated).
!define MUI_FINISHPAGE_RUN
!define MUI_FINISHPAGE_RUN_TEXT "Start DiskRoot"
!define MUI_FINISHPAGE_RUN_FUNCTION StartUnelevated

!insertmacro MUI_PAGE_WELCOME
!insertmacro MUI_PAGE_LICENSE "..\LICENSE-MIT"
!insertmacro MUI_PAGE_COMPONENTS
; No directory page: DiskRoot always goes to Program Files, which normal users
; can't modify. A user-writable folder would let malware replace diskroot.exe,
; which Fast scan runs as admin.
!insertmacro MUI_PAGE_INSTFILES
!insertmacro MUI_PAGE_FINISH
!insertmacro MUI_UNPAGE_CONFIRM
!insertmacro MUI_UNPAGE_INSTFILES
!insertmacro MUI_LANGUAGE "English"

Function StartUnelevated
  Exec '"$WINDIR\explorer.exe" "$INSTDIR\diskroot.exe"'
FunctionEnd

Function .onInit
  ${IfNot} ${RunningX64}
    MessageBox MB_ICONSTOP "DiskRoot needs 64-bit Windows 10 or 11." /SD IDOK
    Abort
  ${EndIf}
  SetRegView 64
  SetShellVarContext all
  ; Ignore /D=... too: always the protected Program Files folder.
  StrCpy $INSTDIR "$PROGRAMFILES64\DiskRoot"
FunctionEnd

Function un.onInit
  SetRegView 64
  SetShellVarContext all
FunctionEnd

Section "DiskRoot (required)" SecApp
  SectionIn RO
  ; Upgrading from the previous version (before the rename): run its own
  ; uninstaller first (silently). It removes its files, shortcuts,
  ; right-click entry, PATH entry and scheduled task; the user's settings and
  ; saved scans stay, and DiskRoot moves them to its folders when it starts.
  ReadRegStr $0 HKLM "Software\Microsoft\Windows\CurrentVersion\Uninstall\Xylem" "InstallLocation"
  ${If} $0 != ""
  ${AndIf} ${FileExists} "$0\uninstall.exe"
    DetailPrint "Removing the previous version from $0"
    ExecWait '"$0\uninstall.exe" /S _?=$0'
    Delete "$0\uninstall.exe"
    RMDir "$0"
  ${EndIf}
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\diskroot.exe"
  File "..\README.md"
  File "..\LICENSE-MIT"
  File "..\LICENSE-APACHE"
  WriteUninstaller "$INSTDIR\uninstall.exe"
  CreateShortcut "$SMPROGRAMS\DiskRoot.lnk" "$INSTDIR\diskroot.exe" "" "$INSTDIR\diskroot.exe" 0 SW_SHOWNORMAL "" "See what's filling your drive"

  WriteRegStr HKLM "${UNINST_KEY}" "DisplayName" "DiskRoot"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayVersion" "${VERSION}"
  WriteRegStr HKLM "${UNINST_KEY}" "Publisher" "DiskRoot contributors"
  WriteRegStr HKLM "${UNINST_KEY}" "URLInfoAbout" "https://github.com/JayD108/DiskRoot"
  WriteRegStr HKLM "${UNINST_KEY}" "DisplayIcon" "$INSTDIR\diskroot.exe"
  WriteRegStr HKLM "${UNINST_KEY}" "InstallLocation" "$INSTDIR"
  WriteRegStr HKLM "${UNINST_KEY}" "UninstallString" '"$INSTDIR\uninstall.exe"'
  WriteRegStr HKLM "${UNINST_KEY}" "QuietUninstallString" '"$INSTDIR\uninstall.exe" /S'
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoModify" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "NoRepair" 1
  WriteRegDWORD HKLM "${UNINST_KEY}" "EstimatedSize" 7500
SectionEnd

Section "Explorer right-click: Analyze with DiskRoot" SecExplorer
  ; Runs as the signed-in user (Explorer launches it), never elevated.
  !macro DiskRootVerb ROOT ARG
    WriteRegStr HKLM "Software\Classes\${ROOT}\shell\DiskRoot" "" "Analyze with DiskRoot"
    WriteRegStr HKLM "Software\Classes\${ROOT}\shell\DiskRoot" "Icon" '"$INSTDIR\diskroot.exe"'
    WriteRegStr HKLM "Software\Classes\${ROOT}\shell\DiskRoot\command" "" '"$INSTDIR\diskroot.exe" "${ARG}"'
  !macroend
  !insertmacro DiskRootVerb "Directory" "%V"
  !insertmacro DiskRootVerb "Directory\Background" "%V"
  !insertmacro DiskRootVerb "Drive" "%V"
SectionEnd

Section "Command-line tool (droot), on PATH" SecCli
  SetOutPath "$INSTDIR"
  File "${SRCDIR}\droot.exe"
  ; Put the install folder on the system PATH so `droot` works in any
  ; new terminal. Done with PowerShell on the registry value itself (NSIS
  ; strings are limited to 1024 characters; a long PATH must never be cut):
  ; keeps %variables%, the value type and every other entry, adds nothing if
  ; it's already there. No script file is written (nothing to swap).
  nsExec::ExecToLog `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d='$INSTDIR';$$k=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment',$$true);$$p=[string]$$k.GetValue('Path','','DoNotExpandEnvironmentNames');$$t=try{$$k.GetValueKind('Path')}catch{'ExpandString'};if(-not(($$p -split ';')|?{$$_.TrimEnd('\') -ieq $$d})){$$k.SetValue('Path',$$(if($$p -eq ''){$$d}elseif($$p.EndsWith(';')){$$p+$$d+';'}else{$$p+';'+$$d}),$$t)};$$k.Close()"`
  Pop $0
  ; Tell open programs (Explorer, new terminals) that PATH changed.
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
SectionEnd

Section /o "Desktop shortcut" SecDesktop
  CreateShortcut "$DESKTOP\DiskRoot.lnk" "$INSTDIR\diskroot.exe"
SectionEnd

!insertmacro MUI_FUNCTION_DESCRIPTION_BEGIN
  !insertmacro MUI_DESCRIPTION_TEXT ${SecApp} "The DiskRoot app, including the Fast scan helper."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecCli} "droot.exe: scan from the command line, export CSV/JSON. Added to PATH so $\"droot$\" works in any new terminal."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecDesktop} "A DiskRoot shortcut on the desktop."
  !insertmacro MUI_DESCRIPTION_TEXT ${SecExplorer} "Adds $\"Analyze with DiskRoot$\" when you right-click a folder or drive."
!insertmacro MUI_FUNCTION_DESCRIPTION_END

Section "Uninstall"
  ; Scheduled scans: remove the per-user tasks DiskRoot created (names start with
  ; "DiskRoot scheduled scan"); nothing else in Task Scheduler is touched.
  nsExec::ExecToLog `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "Get-ScheduledTask -TaskPath '\' -ErrorAction SilentlyContinue | Where-Object { $$_.TaskName -like 'DiskRoot scheduled scan*' } | Unregister-ScheduledTask -Confirm:$$false"`
  Pop $0
  ; Remove only DiskRoot's folder from the system PATH (everything else kept as it was).
  nsExec::ExecToLog `"$SYSDIR\WindowsPowerShell\v1.0\powershell.exe" -NoProfile -NonInteractive -ExecutionPolicy Bypass -Command "$$d='$INSTDIR';$$k=[Microsoft.Win32.Registry]::LocalMachine.OpenSubKey('SYSTEM\CurrentControlSet\Control\Session Manager\Environment',$$true);$$p=[string]$$k.GetValue('Path','','DoNotExpandEnvironmentNames');if($$p -ne ''){$$t=$$k.GetValueKind('Path');$$a=@($$p -split ';');$$b=@($$a|?{$$_.TrimEnd('\') -ine $$d});if($$b.Count -ne $$a.Count){$$k.SetValue('Path',($$b -join ';'),$$t)}};$$k.Close()"`
  Pop $0
  SendMessage ${HWND_BROADCAST} ${WM_SETTINGCHANGE} 0 "STR:Environment" /TIMEOUT=5000
  Delete "$INSTDIR\diskroot.exe"
  Delete "$INSTDIR\droot.exe"
  Delete "$INSTDIR\README.md"
  Delete "$INSTDIR\LICENSE-MIT"
  Delete "$INSTDIR\LICENSE-APACHE"
  Delete "$INSTDIR\uninstall.exe"
  RMDir "$INSTDIR"
  Delete "$SMPROGRAMS\DiskRoot.lnk"
  Delete "$DESKTOP\DiskRoot.lnk"
  DeleteRegKey HKLM "${UNINST_KEY}"
  DeleteRegKey HKLM "Software\Classes\Directory\shell\DiskRoot"
  DeleteRegKey HKLM "Software\Classes\Directory\Background\shell\DiskRoot"
  DeleteRegKey HKLM "Software\Classes\Drive\shell\DiskRoot"
SectionEnd
