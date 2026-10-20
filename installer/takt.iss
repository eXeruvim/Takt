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
AppName=Takt
AppVersion={#MyAppVersion}
AppVerName=Takt {#MyAppVersion}
AppPublisher=Takt contributors
AppComments=Видео- и аудиоплеер
DefaultDirName={localappdata}\Programs\Takt
DefaultGroupName=Takt
DisableProgramGroupPage=yes
AllowNoIcons=yes
LicenseFile={#RepositoryRoot}\LICENSE
OutputBaseFilename=Takt-{#MyAppVersion}-setup-x86_64
SetupIconFile={#RepositoryRoot}\assets\icons\takt.ico
UninstallDisplayIcon={app}\takt.exe
UninstallDisplayName=Takt
PrivilegesRequired=lowest
ArchitecturesAllowed=x64compatible
ArchitecturesInstallIn64BitMode=x64compatible
MinVersion=10.0
Compression=lzma2/max
SolidCompression=yes
WizardStyle=modern dynamic
CloseApplications=force
CloseApplicationsFilter=takt.exe,oxide.exe
RestartApplications=no
RestartIfNeededByRun=no
ChangesAssociations=yes
SetupLogging=yes
VersionInfoCompany=Takt contributors
VersionInfoDescription=Takt {#MyAppVersion} installer
VersionInfoProductName=Takt
VersionInfoProductVersion={#MyNumericVersion}
VersionInfoVersion={#MyNumericVersion}

[Languages]
Name: "english"; MessagesFile: "compiler:Default.isl"
Name: "russian"; MessagesFile: "compiler:Languages\Russian.isl"

[CustomMessages]
english.AssociationsTask=Add Takt to the “Open with” list for supported media
russian.AssociationsTask=Добавить Takt в список «Открыть с помощью» для поддерживаемых медиа
english.DesktopTask=Create a desktop shortcut
russian.DesktopTask=Создать ярлык на рабочем столе

[Tasks]
Name: "associations"; Description: "{cm:AssociationsTask}"; Flags: checkedonce
Name: "desktopicon"; Description: "{cm:DesktopTask}"; Flags: unchecked

[Files]
Source: "{#PayloadDir}\*"; DestDir: "{app}"; Flags: ignoreversion recursesubdirs createallsubdirs notimestamp

[InstallDelete]
; One-time cleanup for installations made before the Takt rename. AppId stays
; unchanged so Inno Setup upgrades the existing installation in place.
Type: files; Name: "{app}\oxide.exe"
Type: files; Name: "{app}\oxide-updater.exe"
Type: files; Name: "{autoprograms}\Oxide.lnk"
Type: files; Name: "{autodesktop}\Oxide.lnk"

[Icons]
Name: "{autoprograms}\Takt"; Filename: "{app}\takt.exe"; WorkingDir: "{app}"
Name: "{autodesktop}\Takt"; Filename: "{app}\takt.exe"; WorkingDir: "{app}"; Tasks: desktopicon

[Registry]
; Remove the pre-rename application registration during an in-place upgrade.
Root: HKCU; Subkey: "Software\Classes\Oxide.Media"; ValueType: none; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\Applications\oxide.exe"; ValueType: none; Flags: deletekey
Root: HKCU; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mov\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.avi\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.flv\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.wmv\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mpg\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mpeg\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.ogv\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.wave\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.m4b\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.oga\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.wma\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.aif\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.aiff\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.aifc\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.ape\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.wv\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.tta\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.alac\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\.mka\OpenWithProgids"; ValueType: none; ValueName: "Oxide.Media"; Flags: deletevalue
Root: HKCU; Subkey: "Software\Classes\Takt.Media"; ValueType: string; ValueData: "Takt media file"; Flags: uninsdeletekey; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Takt.Media\DefaultIcon"; ValueType: string; ValueData: "{app}\takt.exe,0"; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Takt.Media\shell\open\command"; ValueType: string; ValueData: """{app}\takt.exe"" ""%1"""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe"; ValueType: string; ValueName: "FriendlyAppName"; ValueData: "Takt"; Flags: uninsdeletekey; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\shell\open\command"; ValueType: string; ValueData: """{app}\takt.exe"" ""%1"""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mp4"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mkv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".webm"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mov"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".avi"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".m4v"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".ts"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".flv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".wmv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mpg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mpeg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".ogv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mp3"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".flac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".wav"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".wave"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".m4a"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".m4b"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".aac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".ogg"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".oga"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".opus"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".wma"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".aif"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".aiff"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".aifc"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".ape"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".wv"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".tta"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".alac"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\Applications\takt.exe\SupportedTypes"; ValueType: string; ValueName: ".mka"; ValueData: ""; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mp4\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mkv\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.webm\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mov\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.avi\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4v\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ts\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.flv\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wmv\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mpg\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mpeg\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ogv\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mp3\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.flac\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wav\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wave\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4a\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.m4b\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aac\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ogg\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.oga\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.opus\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wma\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aif\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aiff\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.aifc\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.ape\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.wv\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.tta\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.alac\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations
Root: HKCU; Subkey: "Software\Classes\.mka\OpenWithProgids"; ValueType: string; ValueName: "Takt.Media"; ValueData: ""; Flags: uninsdeletevalue; Tasks: associations

[Run]
Filename: "{app}\takt.exe"; Description: "{cm:LaunchProgram,Takt}"; WorkingDir: "{app}"; Flags: nowait postinstall skipifsilent
