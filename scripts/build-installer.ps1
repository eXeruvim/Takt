[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts/release",
    [string]$Compiler,
    [switch]$AllowDirty
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'artifacts'))
if (-not $output.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Installer output должен находиться внутри $artifactRoot`: $output"
}

$toolManifest = Get-Content -LiteralPath (Join-Path $repository 'release/installer-tool.json') -Raw | ConvertFrom-Json
if ([string]::IsNullOrWhiteSpace($Compiler)) {
    $compilerCandidates = @(
        (Join-Path $env:LOCALAPPDATA 'Programs/Inno Setup 7/ISCC.exe'),
        (Join-Path $env:ProgramFiles 'Inno Setup 7/ISCC.exe')
    )
    $Compiler = $compilerCandidates | Where-Object { Test-Path -LiteralPath $_ -PathType Leaf } | Select-Object -First 1
}
if ([string]::IsNullOrWhiteSpace($Compiler) -or -not (Test-Path -LiteralPath $Compiler -PathType Leaf)) {
    throw "Не найден Inno Setup $($toolManifest.version) x64. Установите JRSoftware.InnoSetup.7 или передайте -Compiler."
}
$compilerPath = [System.IO.Path]::GetFullPath($Compiler)
$compilerVersion = (& $compilerPath --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $compilerVersion -notmatch "(^|\s)$([regex]::Escape([string]$toolManifest.version))(\s|$)") {
    throw "Требуется Inno Setup $($toolManifest.version), получено: $compilerVersion"
}

Push-Location $repository
try {
    $status = (& git --no-optional-locks status --porcelain --untracked-files=normal 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось проверить состояние Git: $status"
    }
    if (-not $AllowDirty -and -not [string]::IsNullOrWhiteSpace($status)) {
        throw 'Публичный installer разрешено собирать только из чистого Git checkout. Для локальной проверки используйте -AllowDirty.'
    }

    $manifestText = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
    if ($manifestText -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') {
        throw 'Не удалось прочитать версию Oxide из Cargo.toml.'
    }
    $version = $Matches[1]
    if ($version -notmatch '^(\d+)\.(\d+)\.(\d+)(?:-[^.]+\.(\d+))?$') {
        throw "Версия Oxide не преобразуется в Windows version quad: $version"
    }
    $numericVersion = "$($Matches[1]).$($Matches[2]).$($Matches[3]).$(if ($Matches[4]) { $Matches[4] } else { '0' })"

    $playlistSource = Get-Content -LiteralPath (Join-Path $repository 'src/player/playlist.rs') -Raw
    if ($playlistSource -notmatch '(?s)pub const MEDIA_EXTENSIONS:\s*&\[&str\]\s*=\s*&\[(.*?)\];') {
        throw 'Не удалось прочитать MEDIA_EXTENSIONS из player/playlist.rs.'
    }
    $mediaExtensions = @([regex]::Matches($Matches[1], '"([^"]+)"') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    $installerSource = Get-Content -LiteralPath (Join-Path $repository 'installer/oxide.iss') -Raw
    $openWithExtensions = @([regex]::Matches($installerSource, 'Classes\\\.([A-Za-z0-9]+)\\OpenWithProgids') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    $supportedTypeExtensions = @([regex]::Matches($installerSource, 'ValueName:\s*"\.([A-Za-z0-9]+)"') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique)
    foreach ($set in @($openWithExtensions, $supportedTypeExtensions)) {
        $difference = @(Compare-Object -ReferenceObject $mediaExtensions -DifferenceObject $set)
        if ($difference.Count -ne 0) {
            throw "File associations установщика расходятся с MEDIA_EXTENSIONS: $($difference | Out-String)"
        }
    }

    $releaseParameters = @{ OutputDirectory = $OutputDirectory }
    if ($AllowDirty) {
        $releaseParameters.AllowDirty = $true
    }
    & (Join-Path $PSScriptRoot 'build-release.ps1') @releaseParameters
    if ($LASTEXITCODE -ne 0) {
        throw 'Не удалось подготовить portable payload для установщика.'
    }

    $portableName = "Oxide-$version-windows-x86_64"
    $portableArchive = Join-Path $output "$portableName.zip"
    $staging = Join-Path $output '.installer-staging'
    if (Test-Path -LiteralPath $staging) {
        $stagingItem = Get-Item -LiteralPath $staging -Force
        if (($stagingItem.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
            throw "Installer staging не должен быть reparse point: $staging"
        }
        Remove-Item -LiteralPath $staging -Recurse -Force
    }
    New-Item -ItemType Directory -Path $staging | Out-Null
    try {
        Add-Type -AssemblyName System.IO.Compression.FileSystem
        [System.IO.Compression.ZipFile]::ExtractToDirectory($portableArchive, $staging)
        $payload = Join-Path $staging $portableName
        $installerName = "Oxide-$version-setup-x86_64"
        & $compilerPath --quiet-progress --no-ide-signtools `
            "--output-dir=$output" `
            "--output-filename=$installerName" `
            "--define=MyAppVersion=$version" `
            "--define=MyNumericVersion=$numericVersion" `
            "--define=PayloadDir=$payload" `
            "--define=RepositoryRoot=$repository" `
            (Join-Path $repository 'installer/oxide.iss')
        if ($LASTEXITCODE -ne 0) {
            throw "ISCC завершился с кодом $LASTEXITCODE."
        }
    }
    finally {
        if (Test-Path -LiteralPath $staging) {
            Remove-Item -LiteralPath $staging -Recurse -Force
        }
    }

    $installerPath = Join-Path $output "Oxide-$version-setup-x86_64.exe"
    if (-not (Test-Path -LiteralPath $installerPath -PathType Leaf)) {
        throw "Установщик не создан: $installerPath"
    }
    $installerInfo = [System.Diagnostics.FileVersionInfo]::GetVersionInfo($installerPath)
    if ($installerInfo.ProductName.Trim() -ne 'Oxide' -or $installerInfo.ProductVersion.Trim() -ne $numericVersion) {
        throw "Некорректные Version Info установщика: $($installerInfo.ProductName) $($installerInfo.ProductVersion)"
    }

    $standaloneArtifacts = @(
        @((Join-Path $repository 'sbom/oxide.cdx.json'), "Oxide-$version-sbom.cdx.json"),
        @((Join-Path $repository 'sbom/native-runtime.cdx.json'), "Oxide-$version-native-runtime.cdx.json"),
        @((Join-Path $repository 'THIRD_PARTY_NOTICES.md'), "Oxide-$version-THIRD-PARTY-NOTICES.md"),
        @((Join-Path $repository 'CODEC_MATRIX.md'), "Oxide-$version-CODEC-MATRIX.md")
    )
    foreach ($artifact in $standaloneArtifacts) {
        Copy-Item -LiteralPath $artifact[0] -Destination (Join-Path $output $artifact[1]) -Force
    }
    $releaseFiles = @($portableArchive, $installerPath) + @($standaloneArtifacts | ForEach-Object { Join-Path $output $_[1] }) | Sort-Object
    $hashLines = foreach ($file in $releaseFiles) {
        $hash = (Get-FileHash -LiteralPath $file -Algorithm SHA256).Hash.ToLowerInvariant()
        "$hash  $(Split-Path -Leaf $file)"
    }
    $hashLines | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii
    Write-Host "Installer создан: $installerPath"
}
finally {
    Pop-Location
}
