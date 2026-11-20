; Inno Setup script, compiled by .github/workflows/release.yml:
;   iscc /DVersion=0.1.0 /DSourceDir=dist packaging\windows\enfait.iss
#ifndef Version
  #define Version "0.0.0"
#endif
#ifndef SourceDir
  #define SourceDir "..\..\dist"
#endif

[Setup]
AppId={{6E1C4B7A-3F0D-4C52-9B8E-EF5A1D2C9A41}
AppName=enfait
AppVersion={#Version}
AppPublisher=enfait
AppPublisherURL=https://github.com/IIyn/enfait
DefaultDirName={autopf}\enfait
DefaultGroupName=enfait
DisableProgramGroupPage=yes
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
SetupIconFile=..\..\assets\icon\enfait.ico
UninstallDisplayIcon={app}\enfait.exe
OutputBaseFilename=enfait-{#Version}-x86_64-windows-setup
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ChangesEnvironment=yes

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "french"; MessagesFile: "compiler:Languages\French.isl"

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#SourceDir}\enfait.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\enfait-cli.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#SourceDir}\LICENSE"; DestDir: "{app}"; DestName: "LICENSE.txt"; Flags: ignoreversion

[Icons]
Name: "{autoprograms}\enfait"; Filename: "{app}\enfait.exe"
Name: "{autodesktop}\enfait"; Filename: "{app}\enfait.exe"; Tasks: desktopicon

[Registry]
; Puts enfait-cli on the PATH (per-user or machine-wide, matching the install mode).
Root: HKA; Subkey: "{code:EnvKey}"; ValueType: expandsz; ValueName: "Path"; \
    ValueData: "{olddata};{app}"; Check: NeedsAddPath(ExpandConstant('{app}'))

[Run]
Filename: "{app}\enfait.exe"; Description: "{cm:LaunchProgram,enfait}"; Flags: nowait postinstall skipifsilent

[Code]
function EnvKey(Param: String): String;
begin
  if IsAdminInstallMode then
    Result := 'SYSTEM\CurrentControlSet\Control\Session Manager\Environment'
  else
    Result := 'Environment';
end;

function NeedsAddPath(Dir: String): Boolean;
var
  Path: String;
begin
  if not RegQueryStringValue(HKA, EnvKey(''), 'Path', Path) then
    Path := '';
  Result := Pos(';' + Uppercase(Dir) + ';', ';' + Uppercase(Path) + ';') = 0;
end;
