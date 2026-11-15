; Installer per Windows (Inno Setup 6).
; Compilato dalla action "Installer Windows":
;   ISCC.exe /DAppVersion=0.1.0.42 installer\rico.iss

#ifndef AppVersion
  #define AppVersion "0.1.0"
#endif

[Setup]
; Lo stesso AppId di quando il programma si chiamava ferrocord (fino alla
; 0.12): l'installer aggiorna quella installazione invece di affiancarne
; un'altra (e ne riusa la cartella).
AppId={{6B0E4F8C-3B1D-4E7A-9C2F-8A5D1E3B7C40}
AppName=Rico
AppVersion={#AppVersion}
AppVerName=Rico {#AppVersion}
AppPublisher=Tommaso
DefaultDirName={autopf}\Rico
DefaultGroupName=Rico
; Il gruppo nel menu Start prende il nome nuovo (quello vecchio si toglie
; in [InstallDelete]).
UsePreviousGroup=no
DisableProgramGroupPage=yes
; Installazione per l'utente corrente, senza diritti di amministratore
; (si può scegliere "per tutti gli utenti" nella finestra iniziale).
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir=Output
OutputBaseFilename=rico-setup-{#AppVersion}
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
UninstallDisplayIcon={app}\rico.exe
; Icona del programma di installazione (la stessa dell'app, da assets/make_icon.py).
SetupIconFile=..\assets\icon.ico
LicenseFile=..\LICENSE

[Languages]
Name: "italian"; MessagesFile: "compiler:Languages\Italian.isl"
Name: "english"; MessagesFile: "compiler:Default.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[InstallDelete]
; Quello che resta dalle versioni chiamate ferrocord: gli eseguibili e i
; collegamenti col vecchio nome.
Type: files; Name: "{app}\ferrocord.exe"
Type: files; Name: "{app}\ferrocord-tui.exe"
Type: filesandordirs; Name: "{autoprograms}\ferrocord"
Type: files; Name: "{autodesktop}\ferrocord.lnk"

[Files]
Source: "..\target\release\rico.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\target\release\rico-tui.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\README.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{group}\Rico"; Filename: "{app}\rico.exe"
Name: "{group}\Rico (terminale)"; Filename: "{app}\rico-tui.exe"
Name: "{group}\{cm:UninstallProgram,Rico}"; Filename: "{uninstallexe}"
Name: "{autodesktop}\Rico"; Filename: "{app}\rico.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\rico.exe"; Description: "{cm:LaunchProgram,Rico}"; Flags: nowait postinstall skipifsilent
