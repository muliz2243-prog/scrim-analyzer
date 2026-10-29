; ---------------------------------------------------------------------------
;  Scrim Analyzer — installer (Inno Setup 6)
;
;  Build:     "C:\Program Files (x86)\Inno Setup 6\ISCC.exe" installer\installer.iss
;  or:        iscc installer\installer.iss
; ---------------------------------------------------------------------------

#define AppName        "Scrim Analyzer"
#define AppVersion     "1.0.1"
#define AppPublisher   "Scrim Analyzer"
#define AppExe         "scrim-analyzer.exe"
#define AppCli         "rofl-analyzer.exe"
#define AppId          "{{8F3C1A62-5B4E-4C0D-9E2A-7B1D3F6C9A11}"

[Setup]
AppId={#AppId}
AppName={#AppName}
AppVersion={#AppVersion}
AppVerName={#AppName} {#AppVersion}
AppPublisher={#AppPublisher}
DefaultDirName={localappdata}\Programs\ScrimAnalyzer
DefaultGroupName={#AppName}
DisableProgramGroupPage=yes
DisableDirPage=no
OutputDir=..\dist
OutputBaseFilename=ScrimAnalyzer-Setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; No elevation: the app writes to its own folder (capturas\, notes).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesInstallIn64BitMode=x64compatible
UninstallDisplayIcon={app}\{#AppExe}
SetupIconFile=..\assets\scrim-analyzer.ico
ShowLanguageDialog=no

[Languages]
Name: "spanish"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "Crear acceso directo en el Escritorio"; GroupDescription: "Accesos directos:"
Name: "autostart";  Description: "Iniciar Scrim Analyzer al iniciar Windows"; GroupDescription: "Accesos directos:"

[Files]
Source: "..\target\release\{#AppExe}";   DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\{#AppCli}";   DestDir: "{app}"; Flags: ignoreversion
Source: "..\assets\scrim-analyzer.ico";  DestDir: "{app}"; Flags: ignoreversion

[Dirs]
; App working folder (captures and notes): writable by users.
Name: "{app}\capturas"; Permissions: users-modify

[Icons]
Name: "{group}\{#AppName}";             Filename: "{app}\{#AppExe}"
Name: "{group}\Uninstall {#AppName}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\{#AppName}";       Filename: "{app}\{#AppExe}"; Tasks: desktopicon
Name: "{userstartup}\{#AppName}";       Filename: "{app}\{#AppExe}"; Tasks: autostart

[Run]
Filename: "{app}\{#AppExe}"; Description: "Iniciar {#AppName}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: filesandordirs; Name: "{app}\capturas"

