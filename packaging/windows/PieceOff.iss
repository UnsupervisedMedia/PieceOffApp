; Inno Setup script for the PieceOff Windows installer.
; Built by scripts/bundle.ps1 when ISCC.exe is available:
;   ISCC.exe /DAppVersion=0.2.1 /DSourceDir=<bundle folder> /DOutputDir=<dist> PieceOff.iss
; Installs for the current user only (no administrator prompt).

#ifndef AppVersion
  #error Pass /DAppVersion=x.y.z
#endif
#ifndef SourceDir
  #error Pass /DSourceDir=<bundle folder>
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

[Setup]
AppId={{6B0E5A0C-2C57-4F0B-9D6E-5C6F3B1F7A11}
AppName=PieceOff
AppVersion={#AppVersion}
AppVerName=PieceOff {#AppVersion}
AppPublisher=PieceOff contributors
AppPublisherURL=https://pieceoff.app
AppSupportURL=https://github.com/UnsupervisedMedia/PieceOffApp/issues
DefaultDirName={autopf}\PieceOff
DefaultGroupName=PieceOff
PrivilegesRequired=lowest
PrivilegesRequiredOverridesAllowed=dialog
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
OutputDir={#OutputDir}
OutputBaseFilename=PieceOff-{#AppVersion}-win64-setup
SetupIconFile={#SourcePath}\..\icon\pieceoff.ico
UninstallDisplayIcon={app}\PieceOff.exe
LicenseFile={#SourceDir}\LICENSE
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
CloseApplications=yes

[Tasks]
Name: "desktopicon"; Description: "Create a &desktop shortcut"; GroupDescription: "Shortcuts:"; Flags: unchecked

[Files]
Source: "{#SourceDir}\*"; DestDir: "{app}"; Flags: recursesubdirs createallsubdirs ignoreversion; Excludes: "START-HERE.txt,pieceoff.ps1"

[Icons]
Name: "{group}\PieceOff"; Filename: "{app}\PieceOff.exe"
Name: "{group}\Uninstall PieceOff"; Filename: "{uninstallexe}"
Name: "{autodesktop}\PieceOff"; Filename: "{app}\PieceOff.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\PieceOff.exe"; Description: "Start PieceOff"; Flags: nowait postinstall skipifsilent
