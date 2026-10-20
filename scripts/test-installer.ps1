[CmdletBinding()]
param(
    [string]$Installer,
    [string]$SmokeRoot = "artifacts/installer-smoke"
)

$ErrorActionPreference = 'Stop'

function Get-PeSubsystem([string]$Path) {
    $bytes = [System.IO.File]::ReadAllBytes($Path)
    if ($bytes.Length -lt 256 -or $bytes[0] -ne 0x4D -or $bytes[1] -ne 0x5A) {
        throw "Файл не является PE executable: $Path"
    }
    $peOffset = [System.BitConverter]::ToInt32($bytes, 0x3C)
    $optionalHeader = $peOffset + 24
    if ($peOffset -lt 0 -or $optionalHeader + 70 -gt $bytes.Length -or
        $bytes[$peOffset] -ne 0x50 -or $bytes[$peOffset + 1] -ne 0x45) {
        throw "Некорректный PE header: $Path"
    }
    return [System.BitConverter]::ToUInt16($bytes, $optionalHeader + 68)
}
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'artifacts'))
$smoke = [System.IO.Path]::GetFullPath((Join-Path $repository $SmokeRoot))
if (-not $smoke.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Installer smoke-каталог должен находиться внутри $artifactRoot`: $smoke"
}

$manifestText = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
if ($manifestText -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') {
    throw 'Не удалось прочитать версию Takt из Cargo.toml.'
}
$version = $Matches[1]
if ([string]::IsNullOrWhiteSpace($Installer)) {
    $Installer = "artifacts/release/Takt-$version-setup-x86_64.exe"
}
$installerPath = [System.IO.Path]::GetFullPath((Join-Path $repository $Installer))
if (-not $installerPath.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase) -or
    -not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
    throw "Не найден проверяемый установщик внутри artifacts: $installerPath"
}

$installDirectory = Join-Path $smoke 'app'
$installLog = Join-Path $smoke 'install.log'
$uninstallLog = Join-Path $smoke 'uninstall.log'
$progIdKey = 'HKCU:\Software\Classes\Takt.Media'
$applicationKey = 'HKCU:\Software\Classes\Applications\takt.exe'
$uninstallKey = 'HKCU:\Software\Microsoft\Windows\CurrentVersion\Uninstall\{2C8790F5-19A9-4E70-9A94-8AD00676214F}_is1'
$startMenuShortcut = Join-Path $env:APPDATA 'Microsoft\Windows\Start Menu\Programs\Takt.lnk'

foreach ($path in @($progIdKey, $applicationKey, $uninstallKey, $startMenuShortcut)) {
    if (Test-Path -LiteralPath $path) {
        throw "Smoke-тест не будет изменять существующую установку Takt: $path"
    }
}
if (Test-Path -LiteralPath $smoke) {
    $smokeItem = Get-Item -LiteralPath $smoke -Force
    if (($smokeItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Installer smoke-каталог не должен быть reparse point: $smoke"
    }
    Remove-Item -LiteralPath $smoke -Recurse -Force
}
New-Item -ItemType Directory -Path $smoke | Out-Null

$uninstaller = Join-Path $installDirectory 'unins000.exe'
$installed = $false
try {
    $installArguments = @(
        '/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', '/SP-',
        '/LANG=russian', '/TASKS=associations', "/DIR=$installDirectory", "/LOG=$installLog"
    )
    $process = Start-Process -FilePath $installerPath -ArgumentList $installArguments -WindowStyle Hidden -Wait -PassThru
    if ($process.ExitCode -ne 0) {
        throw "Установщик завершился с кодом $($process.ExitCode). См. $installLog"
    }
    $installed = $true

    foreach ($file in @(
        'takt.exe',
        'takt-updater.exe',
        'libmpv-2.dll',
        'LICENSE',
        'SUPPORT.md',
        'THIRD_PARTY_NOTICES.md',
        'CODEC_MATRIX.md',
        'native-runtime.json',
        'sbom/takt.cdx.json',
        'sbom/native-runtime.cdx.json',
        'unins000.exe'
    )) {
        if (-not (Test-Path -LiteralPath (Join-Path $installDirectory $file) -PathType Leaf)) {
            throw "После установки отсутствует файл: $file"
        }
    }
    $installedExe = Join-Path $installDirectory 'takt.exe'
    if ((Get-PeSubsystem $installedExe) -ne 2) {
        throw 'Установленный takt.exe открывает консоль вместо Windows GUI subsystem.'
    }
    $fileDescription = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($installedExe).FileDescription
    if ($fileDescription.Trim() -ne 'Takt media player') {
        throw "Установленный takt.exe содержит неожиданное описание процесса: $fileDescription"
    }
    $buildInfo = (& $installedExe --build-info 2>&1 | Out-String).Trim() | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or [string]$buildInfo.package_version -ne $version) {
        throw 'Установленный takt.exe не прошёл build-info probe.'
    }
    $native = Get-Content -LiteralPath (Join-Path $repository 'release/native-runtime.json') -Raw | ConvertFrom-Json
    $nativeInfo = (& $installedExe --native-info 2>&1 | Out-String).Trim() | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or [string]$nativeInfo.mpv -ne [string]$native.runtime_versions.mpv) {
        throw 'Установленный takt.exe не прошёл native-info probe.'
    }
    $installedDllHash = (Get-FileHash -LiteralPath (Join-Path $installDirectory 'libmpv-2.dll') -Algorithm SHA256).Hash
    if ($installedDllHash -ne ([string]$native.components[0].sha256).ToUpperInvariant()) {
        throw 'Установщик изменил или подменил libmpv-2.dll.'
    }

    foreach ($path in @($progIdKey, $applicationKey, $uninstallKey, $startMenuShortcut)) {
        if (-not (Test-Path -LiteralPath $path)) {
            throw "После установки отсутствует интеграция Windows: $path"
        }
    }
    foreach ($extension in @('.mp4', '.mkv', '.mp3', '.flac')) {
        $openWith = Get-Item -LiteralPath "HKCU:\Software\Classes\$extension\OpenWithProgids"
        if ($openWith.GetValueNames() -notcontains 'Takt.Media') {
            throw "Takt не зарегистрирован в «Открыть с помощью» для $extension."
        }
    }

    $uninstallProcess = Start-Process -FilePath $uninstaller `
        -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART', "/LOG=$uninstallLog") `
        -WindowStyle Hidden -Wait -PassThru
    if ($uninstallProcess.ExitCode -ne 0) {
        throw "Деинсталлятор завершился с кодом $($uninstallProcess.ExitCode). См. $uninstallLog"
    }
    $installed = $false

    if (Test-Path -LiteralPath $installDirectory) {
        throw "Деинсталлятор оставил каталог приложения: $installDirectory"
    }
    foreach ($path in @($progIdKey, $applicationKey, $uninstallKey, $startMenuShortcut)) {
        if (Test-Path -LiteralPath $path) {
            throw "Деинсталлятор оставил интеграцию Windows: $path"
        }
    }
    foreach ($extension in @('.mp4', '.mkv', '.mp3', '.flac')) {
        $openWithPath = "HKCU:\Software\Classes\$extension\OpenWithProgids"
        if (Test-Path -LiteralPath $openWithPath) {
            $openWith = Get-Item -LiteralPath $openWithPath
            if ($openWith.GetValueNames() -contains 'Takt.Media') {
                throw "Деинсталлятор оставил Takt.Media у расширения $extension."
            }
        }
    }
    Write-Host 'Installer smoke-тест пройден: install, runtime, associations, Start Menu и uninstall.'
}
finally {
    if ($installed -and (Test-Path -LiteralPath $uninstaller -PathType Leaf)) {
        $cleanup = Start-Process -FilePath $uninstaller `
            -ArgumentList @('/VERYSILENT', '/SUPPRESSMSGBOXES', '/NORESTART') `
            -WindowStyle Hidden -Wait -PassThru
        if ($cleanup.ExitCode -ne 0) {
            Write-Warning "Аварийная деинсталляция завершилась с кодом $($cleanup.ExitCode); каталог оставлен для диагностики: $installDirectory"
        }
    }
}
