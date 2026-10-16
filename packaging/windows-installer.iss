; Inno Setup script for the SquatchBrowse Windows installer.
; Installs for the current user only, so it never asks for admin rights.
; Built by .github/workflows/build.yml:  iscc /DAppVersion=1.N packaging\windows-installer.iss

#ifndef AppVersion
  #define AppVersion "1.0"
#endif

[Setup]
AppId={{BE2ACAF7-B48B-46A1-A364-CB335815499C}
AppName=SquatchBrowse
AppVersion={#AppVersion}
AppPublisher=RedSquatch
DefaultDirName={localappdata}\Programs\SquatchBrowse
DefaultGroupName=SquatchBrowse
DisableProgramGroupPage=yes
PrivilegesRequired=lowest
OutputDir=Output
OutputBaseFilename=SquatchBrowse-Setup
SetupIconFile=icon.ico
UninstallDisplayIcon={app}\squatchbrowse.exe
Compression=lzma2
SolidCompression=yes
WizardStyle=modern
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
; Windows 10 or later (WebView2 ships with Windows 10/11)
MinVersion=10.0

[Tasks]
Name: "desktopicon"; Description: "Create a desktop shortcut"; GroupDescription: "Shortcuts:"

[Files]
Source: "..\target\release\squatchbrowse.exe"; DestDir: "{app}"; Flags: ignoreversion

[Icons]
Name: "{group}\SquatchBrowse"; Filename: "{app}\squatchbrowse.exe"
Name: "{autodesktop}\SquatchBrowse"; Filename: "{app}\squatchbrowse.exe"; Tasks: desktopicon

[Run]
Filename: "{app}\squatchbrowse.exe"; Description: "Open SquatchBrowse now"; Flags: nowait postinstall skipifsilent
