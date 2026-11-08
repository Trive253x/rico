; Installer per Windows (Inno Setup 6).
; Compilato dalla action "Installer Windows":
;   ISCC.exe /DAppVersion=0.1.0.42 installer\ferrocord.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

[Setup]
AppId={{6B0E4F8C-3B1D-4E7A-9C2F-8A5D1E3B7C40}
AppName=ferrocord
AppVersion={#AppVersion}
AppVerName=ferrocord {#AppVersion}
AppPublisher=Tommaso
DefaultDirName={autopf}\ferrocord
DefaultGroupName=ferrocord
DisableProgramGroupPage=yes
; Installazione per l'utente corrente, senza diritti di amministratore
; (si può scegliere "per tutti gli utenti" nella finestra iniziale).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=Output
OutputBaseFilename=ferrocord-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\ferrocord.exe
; Icona del programma di installazione (la stessa dell'app, da assets/make_icon.py).
SetupIconFile=..\assets\icon.ico
LicenseFile=..\LICENSE

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "..\target\release\ferrocord.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\ferrocord-tui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\ferrocord"; Filename: "{app}\ferrocord.exe"
Name: "{group}\ferrocord (terminale)"; Filename: "{app}\ferrocord-tui.exe"
Name: "{group}\{cm:UninstallProgram,ferrocord}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\ferrocord"; Filename: "{app}\ferrocord.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\ferrocord.exe"; Description: "{cm:LaunchProgram,ferrocord}"; Flags: nowait postinstall skipifsilent
