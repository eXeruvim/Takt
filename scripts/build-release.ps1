[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts/release",
    [string]$RuntimeDirectory = "artifacts/native-runtime",
    [switch]$AllowDirty
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))
$runtime = [System.IO.Path]::GetFullPath((Join-Path $repository $RuntimeDirectory))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'artifacts'))

foreach ($path in @($output, $runtime)) {
    if (-not $path.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Release-пути должны находиться внутри $artifactRoot`: $path"
    }
}

Push-Location $repository
try {
    $status = (& git --no-optional-locks status --porcelain --untracked-files=normal 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось проверить состояние Git: $status"
    }
    if (-not $AllowDirty -and -not [string]::IsNullOrWhiteSpace($status)) {
        throw 'Публичный release-пакет разрешено собирать только из чистого Git checkout. Для локальной проверки используйте -AllowDirty.'
    }

    $manifestText = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
    if ($manifestText -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') {
        throw 'Не удалось прочитать версию Oxide из Cargo.toml.'
    }
    $version = $Matches[1]
    $packageName = "Oxide-$version-windows-x86_64"

    if (-not (Test-Path -LiteralPath (Join-Path $runtime 'libmpv-2.dll') -PathType Leaf) -or
        -not (Test-Path -LiteralPath (Join-Path $runtime 'mpv.lib') -PathType Leaf)) {
        & (Join-Path $PSScriptRoot 'fetch-native-runtime.ps1') -OutputDirectory $RuntimeDirectory
        if ($LASTEXITCODE -ne 0) {
            throw 'Не удалось подготовить проверенный native runtime.'
        }
    }

    $previousRustFlags = $env:RUSTFLAGS
    $previousPath = $env:PATH
    $previousEpoch = $env:SOURCE_DATE_EPOCH
    try {
        $commitTime = (& git log -1 --format=%ct 2>&1 | Out-String).Trim()
        if ($LASTEXITCODE -ne 0 -or $commitTime -notmatch '^\d+$') {
            throw 'Не удалось получить SOURCE_DATE_EPOCH из Git.'
        }
        $env:SOURCE_DATE_EPOCH = $commitTime
        # rc.exe places a current timestamp in its COFF output. /Brepro makes
        # lld-link derive timestamp-like PE fields from content instead.
        $env:RUSTFLAGS = "-Lnative=$runtime -Clink-arg=-fuse-ld=lld -Clink-arg=/Brepro"
        $env:PATH = "$runtime;$previousPath"
        & cargo build --release --locked
        if ($LASTEXITCODE -ne 0) {
            throw 'Release-сборка Oxide завершилась ошибкой.'
        }
    }
    finally {
        $env:RUSTFLAGS = $previousRustFlags
        $env:PATH = $previousPath
        $env:SOURCE_DATE_EPOCH = $previousEpoch
    }

    $executable = Join-Path $repository 'target/release/oxide.exe'
    $updaterExecutable = Join-Path $repository 'target/release/oxide-updater.exe'
    if (-not (Test-Path -LiteralPath $updaterExecutable -PathType Leaf)) {
        throw "Release updater helper не создан: $updaterExecutable"
    }
    $updaterBuildInfoText = (& $updaterExecutable --build-info 2>&1 | Out-String).Trim()
    $updaterBuildInfo = $updaterBuildInfoText | ConvertFrom-Json
    if ($LASTEXITCODE -ne 0 -or
        [string]$updaterBuildInfo.component -ne 'oxide-updater' -or
        [string]$updaterBuildInfo.package_version -ne $version) {
        throw "Updater helper вернул некорректный build-info: $updaterBuildInfoText"
    }
    & (Join-Path $PSScriptRoot 'verify-native-runtime.ps1') `
        -Executable ([System.IO.Path]::GetRelativePath($repository, $executable)) `
        -RuntimeDirectory $RuntimeDirectory
    if ($LASTEXITCODE -ne 0) {
        throw 'Release executable не прошёл native runtime probe.'
    }

    New-Item -ItemType Directory -Path $output -Force | Out-Null
    $staging = Join-Path $output '.staging'
    if (Test-Path -LiteralPath $staging) {
        $stagingItem = Get-Item -LiteralPath $staging -Force
        if (($stagingItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Release staging не должен быть reparse point: $staging"
        }
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    $package = Join-Path $staging $packageName
    New-Item -ItemType Directory -Path $package -Force | Out-Null

    $copies = @(
        @($executable, 'oxide.exe'),
        @($updaterExecutable, 'oxide-updater.exe'),
        @((Join-Path $runtime 'libmpv-2.dll'), 'libmpv-2.dll'),
        @((Join-Path $runtime 'LICENSE.LGPL'), 'LICENSE.libmpv.LGPL'),
        @((Join-Path $repository 'LICENSE'), 'LICENSE'),
        @((Join-Path $repository 'README.md'), 'README.md'),
        @((Join-Path $repository 'THIRD_PARTY_NOTICES.md'), 'THIRD_PARTY_NOTICES.md'),
        @((Join-Path $repository 'CODEC_MATRIX.md'), 'CODEC_MATRIX.md'),
        @((Join-Path $repository 'sbom/oxide.cdx.json'), 'sbom/oxide.cdx.json'),
        @((Join-Path $repository 'sbom/native-runtime.cdx.json'), 'sbom/native-runtime.cdx.json'),
        @((Join-Path $repository 'release/native-runtime.json'), 'native-runtime.json')
    )
    foreach ($copy in $copies) {
        if (-not (Test-Path -LiteralPath $copy[0] -PathType Leaf)) {
            throw "В release-пакете отсутствует обязательный файл: $($copy[0])"
        }
        $destination = Join-Path $package $copy[1]
        New-Item -ItemType Directory -Path (Split-Path -Parent $destination) -Force | Out-Null
        Copy-Item -LiteralPath $copy[0] -Destination $destination -Force
    }

    $buildInfoProbe = Join-Path $runtime 'oxide-build-info.exe'
    Copy-Item -LiteralPath $executable -Destination $buildInfoProbe -Force
    try {
        $buildInfoText = (& $buildInfoProbe --build-info 2>&1 | Out-String).Trim()
        $buildInfoExitCode = $LASTEXITCODE
        try {
            $buildInfo = $buildInfoText | ConvertFrom-Json
        }
        catch {
            throw "Release executable вернул некорректный build-info: $buildInfoText"
        }
        if ($buildInfoExitCode -ne 0 -or [string]$buildInfo.package_version -ne $version) {
            throw "Release executable сообщает неверную версию: $buildInfoText"
        }
    }
    finally {
        if (Test-Path -LiteralPath $buildInfoProbe -PathType Leaf) {
            Remove-Item -LiteralPath $buildInfoProbe -Force
        }
    }

    $zipPath = Join-Path $output "$packageName.zip"
    if (Test-Path -LiteralPath $zipPath -PathType Leaf) {
        Remove-Item -LiteralPath $zipPath -Force
    }
    Add-Type -AssemblyName System.IO.Compression
    $epoch = [DateTimeOffset]::FromUnixTimeSeconds([long]$commitTime)
    if ($epoch.Year -lt 1980) {
        $epoch = [DateTimeOffset]::new(1980, 1, 1, 0, 0, 0, [TimeSpan]::Zero)
    }
    $stream = [System.IO.File]::Open($zipPath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::ReadWrite, [System.IO.FileShare]::None)
    try {
        $zip = [System.IO.Compression.ZipArchive]::new($stream, [System.IO.Compression.ZipArchiveMode]::Create, $false)
        try {
            $files = Get-ChildItem -LiteralPath $package -File -Recurse | Sort-Object { [System.IO.Path]::GetRelativePath($staging, $_.FullName).Replace('\', '/') }
            foreach ($file in $files) {
                $entryName = [System.IO.Path]::GetRelativePath($staging, $file.FullName).Replace('\', '/')
                if ($entryName -like '*internal notes*') {
                    throw 'Приватный internal notes не должен попадать в release-пакет.'
                }
                $entry = $zip.CreateEntry($entryName, [System.IO.Compression.CompressionLevel]::Optimal)
                $entry.LastWriteTime = $epoch
                $input = [System.IO.File]::OpenRead($file.FullName)
                $outputStream = $entry.Open()
                try {
                    $input.CopyTo($outputStream)
                }
                finally {
                    $outputStream.Dispose()
                    $input.Dispose()
                }
            }
        }
        finally {
            $zip.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }

    $hash = (Get-FileHash -LiteralPath $zipPath -Algorithm SHA256).Hash.ToLowerInvariant()
    "$hash  $(Split-Path -Leaf $zipPath)" | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii

    $smoke = Join-Path $output '.smoke'
    if (Test-Path -LiteralPath $smoke) {
        $smokeItem = Get-Item -LiteralPath $smoke -Force
        if (($smokeItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Release smoke-каталог не должен быть reparse point: $smoke"
        }
        Remove-Item -LiteralPath $smoke -Recurse -Force
    }
    New-Item -ItemType Directory -Path $smoke | Out-Null
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [System.IO.Compression.ZipFile]::ExtractToDirectory($zipPath, $smoke)
        $smokePackage = Join-Path $smoke $packageName
        if (Test-Path -LiteralPath (Join-Path $smokePackage 'internal notes')) {
            throw 'Приватный internal notes обнаружен после распаковки release.'
        }
        $smokeExecutable = Join-Path $smokePackage 'oxide.exe'
        $smokeUpdater = Join-Path $smokePackage 'oxide-updater.exe'
        $smokeBuildInfo = (& $smokeExecutable --build-info 2>&1 | Out-String).Trim() | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or [string]$smokeBuildInfo.package_version -ne $version) {
            throw 'Распакованный portable executable не прошёл build-info smoke-тест.'
        }
        $smokeUpdaterInfo = (& $smokeUpdater --build-info 2>&1 | Out-String).Trim() | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or [string]$smokeUpdaterInfo.package_version -ne $version) {
            throw 'Распакованный updater helper не прошёл build-info smoke-тест.'
        }
        $smokeNativeInfo = (& $smokeExecutable --native-info 2>&1 | Out-String).Trim() | ConvertFrom-Json
        if ($LASTEXITCODE -ne 0 -or [string]$smokeNativeInfo.mpv -ne [string](Get-Content -LiteralPath (Join-Path $repository 'release/native-runtime.json') -Raw | ConvertFrom-Json).runtime_versions.mpv) {
            throw 'Распакованный portable executable не прошёл native-info smoke-тест.'
        }
    }
    finally {
        if (Test-Path -LiteralPath $smoke) {
            Remove-Item -LiteralPath $smoke -Recurse -Force
        }
    }

    Remove-Item -LiteralPath $staging -Recurse -Force
    Write-Host "Portable release создан: $zipPath"
    Write-Host "SHA-256: $hash"
}
finally {
    Pop-Location
}
