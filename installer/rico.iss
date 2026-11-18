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
; In-app update (Rico starts the installer with /SILENT /UPDATE=1): Rico
; opens again by itself at the end (see docs/update-design.md).
Filename: "{app}\rico.exe"; Parameters: "--updated"; Flags: nowait; Check: IsUpdate

[Code]
const
  UninstallKey = 'Software\Microsoft\Windows\CurrentVersion\Uninstall\{6B0E4F8C-3B1D-4E7A-9C2F-8A5D1E3B7C40}_is1';

// Started by Rico's updater (/UPDATE=1).
function IsUpdate: Boolean;
begin
  Result := ExpandConstant('{param:UPDATE|0}') = '1';
end;

// The next number of a version "x.y.z" (anything else counts as 0).
function NextPart(var S: String): Integer;
var
  P: Integer;
begin
  P := Pos('.', S);
  if P = 0 then begin
    Result := StrToIntDef(S, 0);
    S := '';
  end else begin
    Result := StrToIntDef(Copy(S, 1, P - 1), 0);
    Delete(S, 1, P);
  end;
end;

// The candidate number of "0.18.0-rc.2" (2); 0 for a final release.
function RcNumber(V: String): Integer;
var
  P: Integer;
begin
  P := Pos('-rc.', V);
  if P = 0 then
    Result := 0
  else
    Result := StrToIntDef(Copy(V, P + 4, Length(V)), 0);
end;

// "0.18.0" of "0.18.0-rc.2".
function CoreOf(V: String): String;
var
  P: Integer;
begin
  P := Pos('-', V);
  if P = 0 then
    Result := V
  else
    Result := Copy(V, 1, P - 1);
end;

// -1, 0 or 1 as A is older, equal or newer than B, candidates included:
// 0.18.0-rc.1 < 0.18.0-rc.2 < 0.18.0 < 0.18.1.
function CompareVersions(A, B: String): Integer;
var
  I, X, Y, RA, RB: Integer;
  CA, CB: String;
begin
  Result := 0;
  RA := RcNumber(A);
  RB := RcNumber(B);
  CA := CoreOf(A);
  CB := CoreOf(B);
  for I := 1 to 3 do begin
    X := NextPart(CA);
    Y := NextPart(CB);
    if X < Y then begin Result := -1; exit; end;
    if X > Y then begin Result := 1; exit; end;
  end;
  if RA = RB then exit;
  if RA = 0 then
    Result := 1
  else if RB = 0 then
    Result := -1
  else if RA < RB then
    Result := -1
  else
    Result := 1;
end;

// The installed version and folder, for the user or for everyone.
function Installed(var Version, Folder: String): Boolean;
begin
  Result := RegQueryStringValue(HKCU, UninstallKey, 'DisplayVersion', Version) or
            RegQueryStringValue(HKLM, UninstallKey, 'DisplayVersion', Version);
  if Result then
    if not RegQueryStringValue(HKCU, UninstallKey, 'InstallLocation', Folder) then
      RegQueryStringValue(HKLM, UninstallKey, 'InstallLocation', Folder);
end;

// The updater never goes back: in update mode an installer that is not
// newer than the installed Rico stops, and the installed Rico is opened
// again (Rico closed itself to update). By hand, any version can still be
// installed (Windows asks, it is the person's choice).
function InitializeSetup: Boolean;
var
  Version, Folder: String;
  Code: Integer;
begin
  Result := True;
  if IsUpdate and Installed(Version, Folder) then
    if CompareVersions('{#AppVersion}', Version) <= 0 then begin
      Result := False;
      if Folder <> '' then
        Exec(AddBackslash(Folder) + 'rico.exe', '', '', SW_SHOW, ewNoWait, Code);
    end;
end;
