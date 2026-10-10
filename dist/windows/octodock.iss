
#ifndef AppVersion
  #error AppVersion must be defined (/DAppVersion=x.y.z)
#endif
#ifndef Arch
  #error Arch must be defined (/DArch=x86_64 or /DArch=aarch64)
#endif
#ifndef PackageDir
  #error PackageDir must be defined (/DPackageDir=<staged package directory>)
#endif
#ifndef OutputDir
  #define OutputDir "."
#endif

#if Arch == "aarch64"
  #define ArchAllowed "arm64"
#else
  #define ArchAllowed "x64compatible"
#endif

[Setup]
AppId=io.github.octopuslowtech.octodock
AppName=Octodock
AppVersion={#AppVersion}
AppVerName=Octodock {#AppVersion}
AppPublisher=Octodock
AppPublisherURL=https://github.com/octopuslowtech/octodock
AppSupportURL=https://github.com/octopuslowtech/octodock/issues
VersionInfoVersion={#AppVersion}
PrivilegesRequired=lowest
DefaultDirName={autopf}\Octodock
DisableProgramGroupPage=yes
DisableDirPage=auto
DisableReadyPage=yes
ArchitecturesAllowed={#ArchAllowed}
ArchitecturesInstallIn64BitMode={#ArchAllowed}
MinVersion=10.0
OutputDir={#OutputDir}
OutputBaseFilename=octodock-{#AppVersion}-windows-{#Arch}-setup
SetupIconFile=octodock.ico
UninstallDisplayIcon={app}\octodock.exe
UninstallDisplayName=Octodock
WizardStyle=modern
Compression=lzma2/max
SolidCompression=yes
CloseApplications=yes
RestartApplications=no

[Tasks]
Name: "desktopicon"; Description: "{cm:CreateDesktopIcon}"; GroupDescription: "{cm:AdditionalIcons}"; Flags: unchecked

[Files]
Source: "{#PackageDir}\octodock.exe"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\LICENSE"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\THIRD_PARTY_NOTICES.md"; DestDir: "{app}"; Flags: ignoreversion
Source: "{#PackageDir}\licenses\*"; DestDir: "{app}\licenses"; Flags: ignoreversion recursesubdirs createallsubdirs

[Icons]
Name: "{autoprograms}\Octodock"; Filename: "{app}\octodock.exe"
Name: "{autodesktop}\Octodock"; Filename: "{app}\octodock.exe"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\octodock"; ValueType: string; ValueName: ""; ValueData: "URL:Octodock"; Flags: uninsdeletekey
Root: HKCU; Subkey: "Software\Classes\octodock"; ValueType: string; ValueName: "URL Protocol"; ValueData: ""
Root: HKCU; Subkey: "Software\Classes\octodock\DefaultIcon"; ValueType: string; ValueName: ""; ValueData: """{app}\octodock.exe"",0"
Root: HKCU; Subkey: "Software\Classes\octodock\shell\open\command"; ValueType: string; ValueName: ""; ValueData: """{app}\octodock.exe"" ""%1"""

[Run]
Filename: "{app}\octodock.exe"; Description: "{cm:LaunchProgram,Octodock}"; Flags: nowait postinstall skipifsilent

