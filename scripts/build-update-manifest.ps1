[CmdletBinding()]
param(
    [string]$ReleaseDirectory = "artifacts/release",
    [Parameter(Mandatory = $true)]
    [string]$Repository,
    [string]$NotesPath = "release/update-notes.txt"
)

$ErrorActionPreference = 'Stop'
$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$releaseRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $ReleaseDirectory))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot 'artifacts'))
if (-not $releaseRoot.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Release directory должен находиться внутри $artifactRoot`: $releaseRoot"
}
if ($Repository -notmatch '^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$') {
    throw "Repository должен иметь вид owner/name: $Repository"
}

$manifestText = Get-Content -LiteralPath (Join-Path $repositoryRoot 'Cargo.toml') -Raw
if ($manifestText -notmatch '(?m)^version\s*=\s*"([^"]+)"\s*$') {
    throw 'Не удалось прочитать версию Takt из Cargo.toml.'
}
$version = $Matches[1]
$channel = if ($version -match '-alpha(?:\.|$)') {
    'alpha'
}
elseif ($version -match '-(?:beta|rc)(?:\.|$)') {
    'beta'
}
elseif ($version -notmatch '-') {
    'stable'
}
else {
    throw "Неизвестный release channel в версии $version."
}

$notesFile = [System.IO.Path]::GetFullPath((Join-Path $repositoryRoot $NotesPath))
if (-not (Test-Path -LiteralPath $notesFile -PathType Leaf)) {
    throw "Не найден файл release notes: $notesFile"
}
$notes = @(Get-Content -LiteralPath $notesFile -Encoding utf8 | ForEach-Object { $_.Trim() } | Where-Object { $_ })
if ($notes.Count -eq 0 -or $notes.Count -gt 24) {
    throw 'Release notes должны содержать от 1 до 24 непустых строк.'
}
foreach ($note in $notes) {
    if ([Text.Encoding]::UTF8.GetByteCount($note) -gt 4096) {
        throw 'Одна строка release notes превышает 4096 UTF-8 байт.'
    }
}

$installerName = "Takt-$version-setup-x86_64.exe"
$portableName = "Takt-$version-windows-x86_64.zip"
$installerPath = Join-Path $releaseRoot $installerName
$portablePath = Join-Path $releaseRoot $portableName
foreach ($path in @($installerPath, $portablePath)) {
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Не найден release package: $path"
    }
}

$publishedAt = (& git -C $repositoryRoot log -1 --format=%cI 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or [string]::IsNullOrWhiteSpace($publishedAt)) {
    throw "Не удалось получить дату release commit: $publishedAt"
}
$baseUrl = "https://github.com/$Repository/releases/download/v$version"
$packages = @(
    [ordered]@{
        kind = 'installer'
        architecture = 'x86_64-windows'
        url = "$baseUrl/$installerName"
        sha256 = (Get-FileHash -LiteralPath $installerPath -Algorithm SHA256).Hash.ToLowerInvariant()
        size = (Get-Item -LiteralPath $installerPath).Length
    },
    [ordered]@{
        kind = 'portable'
        architecture = 'x86_64-windows'
        url = "$baseUrl/$portableName"
        sha256 = (Get-FileHash -LiteralPath $portablePath -Algorithm SHA256).Hash.ToLowerInvariant()
        size = (Get-Item -LiteralPath $portablePath).Length
    }
)
$unsigned = [ordered]@{
    schema = 1
    channel = $channel
    version = $version
    published_at = $publishedAt
    notes = $notes
    packages = $packages
}
$unsignedPath = Join-Path $releaseRoot '.takt-update-unsigned.json'
$signedPath = Join-Path $releaseRoot "takt-update-$channel.json"
try {
    $unsigned | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $unsignedPath -Encoding utf8
    Push-Location $repositoryRoot
    try {
        & cargo run --quiet --locked --bin takt-update-manifest -- sign $unsignedPath $signedPath
        if ($LASTEXITCODE -ne 0) { throw 'Не удалось подписать update manifest.' }
        $publicKey = (Get-Content -LiteralPath (Join-Path $repositoryRoot 'release/update-public-key.txt') -Raw).Trim()
        foreach ($kind in @('installer', 'portable')) {
            & cargo run --quiet --locked --bin takt-update-manifest -- verify `
                $signedPath $publicKey '0.0.0' $channel $kind 'x86_64-windows' 'github.com'
            if ($LASTEXITCODE -ne 0) { throw "Проверка update manifest для $kind завершилась ошибкой." }
        }
    }
    finally {
        Pop-Location
    }
}
finally {
    if (Test-Path -LiteralPath $unsignedPath -PathType Leaf) {
        Remove-Item -LiteralPath $unsignedPath -Force
    }
}

$checksumsPath = Join-Path $releaseRoot 'SHA256SUMS.txt'
$manifestName = Split-Path -Leaf $signedPath
$manifestHash = (Get-FileHash -LiteralPath $signedPath -Algorithm SHA256).Hash.ToLowerInvariant()
$checksumLines = @()
if (Test-Path -LiteralPath $checksumsPath -PathType Leaf) {
    $checksumLines = @(Get-Content -LiteralPath $checksumsPath | Where-Object {
        $_ -and -not $_.EndsWith("  $manifestName", [StringComparison]::Ordinal)
    })
}
$checksumLines += "$manifestHash  $manifestName"
$checksumLines | Sort-Object | Set-Content -LiteralPath $checksumsPath -Encoding ascii

Write-Host "Подписанный update manifest создан: $signedPath"
