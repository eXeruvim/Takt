#ifndef MyAppVersion
  #error MyAppVersion is required
#endif
#ifndef MyNumericVersion
  #error MyNumericVersion is required
#endif
#ifndef PayloadDir
  #error PayloadDir is required
#endif
#ifndef RepositoryRoot
  #error RepositoryRoot is required
#endif

[Setup]
AppId={{2C8790F5-19A9-4E70-9A94-8AD00676214F}
AppName=Oxide
AppVersion={#MyAppVersion}
AppVerName=Oxide {#MyAppVersion}
AppPublisher=Oxide contributors
AppComments=Видео- и аудиоплеер
DefaultDirName={localappdata}\Programs\Oxide
DefaultGroupName=Oxide
DisableProgramGroupPage=yes
AllowNoIcons=yes
LicenseFile={#RepositoryRoot}\LICENSE
OutputBaseFilename=Oxide-{#MyAppVersion}-setup-x86_64
SetupIconFile={#RepositoryRoot}\assets\icons\oxide.ico
UninstallDisplayIcon={app}\oxide.exe
UninstallDisplayName=Oxide
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern dynamic
CloseApplications=force
CloseApplicationsFilter=oxide.exe
RestartApplications=no
RestartIfNeededByRun=no
ChangesAssociations=yes
SetupLogging=yes
VersionInfoCompany=Oxide contributors
VersionInfoDescription=Oxide {#MyAppVersion} installer
VersionInfoProductName=Oxide
VersionInfoProductVersion={#MyNumericVersion}
VersionInfoVersion={#MyNumericVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.AssociationsTask=Add Oxide to the “Open with” list for supported media
russian.AssociationsTask=Добавить Oxide в список «Открыть с помощью» для поддерживаемых медиа
english.DesktopTask=Create a desktop shortcut
russian.DesktopTask=Создать ярлык на рабочем столе

[Tasks]
Name: "associations"; Description: "{cm:AssociationsTask}"; Flags: checkedonce
Name: "desktopicon"; Description: "{cm:DesktopTask}"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs notimestamp

[Icons]
Name: "{autoprograms}\Oxide"; Filename: "{app}\oxide.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Oxide"; Filename: "{app}\oxide.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
Root: HKCU; Subkey: "Software\Classes\Oxide.Media"; ValueType: string; ValueData: "Oxide media file"; Flags: uninsdeletekey; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Oxide.Media\DefaultIcon"; ValueType: string; ValueData: "{app}\oxide.exe,0"; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Oxide.Media\shell\open\command"; ValueType: string; ValueData: """{app}\oxide.exe"" ""%1"""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Oxide"; Flags: uninsdeletekey; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\shell\open\command"; ValueType: string; ValueData: """{app}\oxide.exe"" ""%1"""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".webm"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mov"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".avi"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".m4v"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".ts"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".flv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".wmv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mpg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mpeg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".ogv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mp3"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".flac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".wav"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".wave"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".m4a"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".m4b"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".aac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".ogg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".oga"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".opus"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".wma"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".aif"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".aiff"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".aifc"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".ape"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".wv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".tta"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".alac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe\SupportedTypes"; ValueType: string; ValueName: ".mka"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mov\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.avi\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.flv\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wmv\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mpg\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mpeg\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ogv\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wave\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4b\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.oga\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wma\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aif\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aiff\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aifc\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ape\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wv\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.tta\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.alac\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mka\OpenWithProgids"; ValueType: string; ValueName: "Oxide.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations

[Run]
Filename: "{app}\oxide.exe"; Description: "{cm:LaunchProgram,Oxide}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
