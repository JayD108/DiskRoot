; Xylem installer (Inno Setup 6). Built by tools\release.ps1; don't run
; ISCC on this directly unless the release binaries are in target\release.
;
; Why an installer matters for security: Fast scan starts xylem.exe with
; admin rights. Installed in Program Files, only administrators can replace
; that exe, so malware running as the user can't swap it before the UAC
; prompt. (A copy in Downloads has no such protection.)

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\target\release"
#endif
#ifndef OutDir
  #define OutDir "..\dist"
#endif

[Setup]
AppId={{6F1C3B5E-8E0A-4C7B-9D3A-58A1F4C2E7B9}
AppName=Xylem
AppVersion={#AppVersion}
AppVerName=Xylem {#AppVersion}
AppPublisher=Xylem contributors
AppComments=Fast disk space analyzer for Windows
DefaultDirName={autopf}\Xylem
DefaultGroupName=Xylem
DisableProgramGroupPage=yes
; Per-machine install into Program Files by default (admin-protected).
; /CURRENTUSER on the command line allows a per-user test install.
PrivilegesRequired=admin
PrivilegesRequiredOverridesAllowed=commandline
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
OutputDir={#OutDir}
OutputBaseFilename=xylem-setup-{#AppVersion}
SetupIconFile=..\crates\native\assets\icon.ico
UninstallDisplayIcon={app}\xylem.exe
UninstallDisplayName=Xylem
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
LicenseFile=..\LICENSE-MIT
; Settings and the deletion log live in the user's profile and are kept on
; uninstall (the log is the user's record of what they deleted).

[Types]
Name: "full"; Description: "Xylem with command-line tool"
Name: "compact"; Description: "Xylem only"
Name: "custom"; Description: "Custom"; Flags: iscustom

[Components]
Name: "app"; Description: "Xylem (native app, includes the Fast scan helper)"; Types: full compact custom; Flags: fixed
Name: "cli"; Description: "xylem-cli (command line: scan, CSV export)"; Types: full

[Files]
Source: "{#SourceDir}\xylem.exe"; DestDir: "{app}"; Components: app; Flags: ignoreversion
Source: "{#SourceDir}\xylem-cli.exe"; DestDir: "{app}"; Components: cli; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion isreadme
Source: "..\LICENSE-MIT"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE-APACHE"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\Xylem"; Filename: "{app}\xylem.exe"; Comment: "See what's filling your drive"
Name: "{autodesktop}\Xylem"; Filename: "{app}\xylem.exe"; Tasks: desktopicon

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; Flags: unchecked

[Run]
Filename: "{app}\xylem.exe"; Description: "Start Xylem"; Flags: nowait postinstall skipifsilent
