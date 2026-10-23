; Vigie installer (decision 0009): per-user, no elevation, into %LOCALAPPDATA%\Programs\Vigie.
; Built by `cargo xtask dist` with Inno Setup 6 (6.7.3 locally, 6.7.1 in CI):
;   ISCC.exe /DAppVersion=0.1.0 /DSourceExe=<path to vigie.exe> /DOutputDir=<dist> vigie.iss

#ifndef AppVersion
  #define AppVersion "0.0.0"
#endif
#ifndef SourceExe
  #define SourceExe "..\..\target\release\vigie.exe"
#endif
#ifndef OutputDir
  #define OutputDir "..\..\dist"
#endif

[Setup]
AppId={{5B7E1C2A-6F0D-4C8E-9A31-2D7B8E4F9C10}
AppName=Vigie
AppVersion={#AppVersion}
AppVerName=Vigie {#AppVersion}
AppPublisher=Evan Pluchart
AppPublisherURL=https://evanpluchart.fr/vigie
AppSupportURL=https://github.com/EvanPluchart/vigie
; Per-user install, no administrator prompt. The folder is fixed: "Replace Task Manager" only
; accepts the Vigie installed there.
PrivilegesRequired=lowest
DefaultDirName={userpf}\Vigie
DisableDirPage=yes
DefaultGroupName=Vigie
DisableProgramGroupPage=yes
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\..\assets\brand\icons\vigie.ico
UninstallDisplayIcon={app}\vigie.exe
UninstallDisplayName=Vigie
OutputDir={#OutputDir}
OutputBaseFilename=Vigie-{#AppVersion}-setup
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern
; A running Vigie is closed before files are replaced (upgrades) and before uninstalling.
AppMutex=Local\Vigie.SingleInstance
CloseApplications=yes
RestartApplications=no
LicenseFile=..\..\LICENSE

[Languages]
Name: "en"; MessagesFile: "compiler:Default.isl"
Name: "fr"; MessagesFile: "compiler:Languages\French.isl"

[CustomMessages]
en.StartWithWindows=Start Vigie when I sign in
fr.StartWithWindows=Démarrer Vigie à l'ouverture de session
en.LaunchVigie=Launch Vigie
fr.LaunchVigie=Lancer Vigie
en.SettingsKept=Your settings are kept in %APPDATA%\Vigie. Delete that folder to remove them too.
fr.SettingsKept=Vos réglages sont conservés dans %APPDATA%\Vigie. Supprimez ce dossier pour les effacer aussi.
en.TaskmgrStillOn=Vigie still opens with Ctrl+Shift+Esc, and the administrator prompt to give it back to Task Manager was declined. Uninstalling now would leave Ctrl+Shift+Esc opening nothing, so Vigie stays installed. Turn the option off in Vigie's Settings, then uninstall.
fr.TaskmgrStillOn=Vigie s'ouvre toujours avec Ctrl+Maj+Échap, et l'invite administrateur pour rendre ce raccourci au Gestionnaire des tâches a été refusée. Désinstaller maintenant laisserait Ctrl+Maj+Échap sans effet : Vigie reste installé. Désactivez l'option dans les Réglages de Vigie, puis désinstallez.

[Tasks]
Name: "startup"; Description: "{cm:StartWithWindows}"; Flags: unchecked
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceExe}"; DestDir: "{app}"; DestName: "vigie.exe"; Flags: ignoreversion
Source: "..\..\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{userprograms}\Vigie"; Filename: "{app}\vigie.exe"
Name: "{userdesktop}\Vigie"; Filename: "{app}\vigie.exe"; Tasks: desktopicon

[Registry]
; Only the value this installer creates; removed on uninstall.
Root: HKCU; Subkey: "Software\Microsoft\Windows\CurrentVersion\Run"; ValueType: string; ValueName: "Vigie"; ValueData: """{app}\vigie.exe"""; Flags: uninsdeletevalue; Tasks: startup

[Run]
Filename: "{app}\vigie.exe"; Description: "{cm:LaunchVigie}"; Flags: nowait postinstall skipifsilent

[UninstallDelete]
Type: dirifempty; Name: "{app}"

[Code]
const
  IfeoKey = 'SOFTWARE\Microsoft\Windows NT\CurrentVersion\Image File Execution Options\taskmgr.exe';

{ True when Ctrl+Shift+Esc is mapped to a Vigie (decision 0009). }
function TaskmgrReplaced(): Boolean;
var
  Value: String;
begin
  Result := RegQueryStringValue(HKLM64, IfeoKey, 'Debugger', Value) and
    (Pos('vigie.exe', Lowercase(Value)) > 0);
end;

{ Before any file goes: give Ctrl+Shift+Esc back to Task Manager, or keep Vigie installed. }
function InitializeUninstall(): Boolean;
var
  ResultCode: Integer;
begin
  Result := True;
  if TaskmgrReplaced() then
  begin
    ShellExec('runas', ExpandConstant('{app}\vigie.exe'), '--set-replace-taskmgr=off', '',
      SW_HIDE, ewWaitUntilTerminated, ResultCode);
    if TaskmgrReplaced() then
    begin
      SuppressibleMsgBox(CustomMessage('TaskmgrStillOn'), mbError, MB_OK, IDOK);
      Result := False;
    end;
  end;
end;

procedure CurUninstallStepChanged(CurUninstallStep: TUninstallStep);
begin
  if (CurUninstallStep = usPostUninstall) and not UninstallSilent() then
    MsgBox(CustomMessage('SettingsKept'), mbInformation, MB_OK);
end;
