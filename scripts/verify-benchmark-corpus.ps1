[CmdletBinding()]
param(
    [string]$Manifest = (Join-Path $PSScriptRoot '..\benchmark-corpus\manifest.local.json'),

    [switch]$RequireReleaseCoverage,

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Resolve-CorpusPath {
    param(
        [Parameter(Mandatory = $true)]
        [string]$CorpusRoot,
        [Parameter(Mandatory = $true)]
        [string]$RelativePath
    )

    if ([System.IO.Path]::IsPathRooted($RelativePath)) {
        throw "Путь корпуса должен быть относительным: $RelativePath"
    }
    $resolved = [System.IO.Path]::GetFullPath((Join-Path $CorpusRoot $RelativePath))
    if (-not $resolved.StartsWith($CorpusRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw "Путь выходит за пределы корпуса: $RelativePath"
    }
    return $resolved
}

function Assert-TextValue {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Entry,
        [Parameter(Mandatory = $true)]
        [string]$Name
    )

    $property = $Entry.PSObject.Properties[$Name]
    if ($null -eq $property -or [string]::IsNullOrWhiteSpace([string]$property.Value)) {
        throw "Поле '$Name' отсутствует или пусто"
    }
    return [string]$property.Value
}

if ($ValidateOnly) {
    $repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
    $probe = Resolve-CorpusPath -CorpusRoot $repository -RelativePath 'benchmark-corpus\README.md'
    if (-not $probe.StartsWith($repository, [System.StringComparison]::OrdinalIgnoreCase)) {
        throw 'Самопроверка безопасного пути завершилась ошибкой'
    }
    try {
        Resolve-CorpusPath -CorpusRoot $repository -RelativePath '..\outside.bin' | Out-Null
        throw 'Выход за границы корпуса не был отклонён'
    }
    catch {
        if ($_.Exception.Message -eq 'Выход за границы корпуса не был отклонён') {
            throw
        }
    }
    Write-Host 'Синтаксис и проверка границ корпуса: OK'
    return
}

$manifestPath = [System.IO.Path]::GetFullPath($Manifest)
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Манифест корпуса не найден: $manifestPath"
}
$corpusRoot = [System.IO.Path]::GetFullPath((Split-Path -Parent $manifestPath))
$manifestData = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 | ConvertFrom-Json
if ($manifestData.schema -ne 1) {
    throw "Неподдерживаемая схема корпуса: $($manifestData.schema)"
}
if ([string]::IsNullOrWhiteSpace([string]$manifestData.corpus_id)) {
    throw 'corpus_id отсутствует или пуст'
}
$entries = @($manifestData.files)
if ($entries.Count -eq 0) {
    throw 'Манифест корпуса не содержит файлов'
}

$knownIds = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
$verified = [System.Collections.Generic.List[object]]::new()
foreach ($entry in $entries) {
    $id = Assert-TextValue -Entry $entry -Name 'id'
    if (-not $knownIds.Add($id)) {
        throw "Повторяющийся id корпуса: $id"
    }
    $relativePath = Assert-TextValue -Entry $entry -Name 'path'
    $mediaKind = Assert-TextValue -Entry $entry -Name 'media_kind'
    if ($mediaKind -notin @('audio', 'video', 'damaged')) {
        throw "Неизвестный media_kind у '$id': $mediaKind"
    }
    $container = Assert-TextValue -Entry $entry -Name 'container'
    $license = Assert-TextValue -Entry $entry -Name 'license'
    $source = Assert-TextValue -Entry $entry -Name 'source'
    $expectedHash = (Assert-TextValue -Entry $entry -Name 'sha256').ToUpperInvariant()
    if ($expectedHash -notmatch '^[0-9A-F]{64}$') {
        throw "Некорректный SHA-256 у '$id'"
    }
    if ($null -eq $entry.size_bytes -or [int64]$entry.size_bytes -lt 1) {
        throw "Некорректный size_bytes у '$id'"
    }

    $path = Resolve-CorpusPath -CorpusRoot $corpusRoot -RelativePath $relativePath
    if (-not (Test-Path -LiteralPath $path -PathType Leaf)) {
        throw "Файл корпуса не найден для '$id': $path"
    }
    $item = Get-Item -LiteralPath $path -Force -ErrorAction Stop
    if ($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) {
        throw "Ссылки и reparse points запрещены в корпусе: $path"
    }
    if ($item.Length -ne [int64]$entry.size_bytes) {
        throw "Размер '$id' не совпадает: ожидалось $($entry.size_bytes), найдено $($item.Length)"
    }
    $actualHash = (Get-FileHash -LiteralPath $path -Algorithm SHA256).Hash
    if ($actualHash -ne $expectedHash) {
        throw "SHA-256 '$id' не совпадает"
    }

    $features = @($entry.features | ForEach-Object { [string]$_ })
    $verified.Add([pscustomobject][ordered]@{
        id = $id
        media_kind = $mediaKind
        container = $container
        size_mib = [Math]::Round($item.Length / 1MB, 3)
        features = $features -join ', '
        license = $license
        source = $source
    })
}

if ($RequireReleaseCoverage) {
    $allFeatures = @($entries.features | ForEach-Object { [string]$_ })
    $requiredFeatures = @(
        'artwork:none', 'artwork:normal', 'artwork:large',
        'resolution:1080p', 'resolution:4k',
        'fps:24', 'fps:30', 'fps:60',
        'codec:h264', 'codec:hevc', 'codec:vp9', 'codec:av1',
        'subtitles', 'multi-track', 'chapters', 'unicode', 'damaged', 'lossless', 'gapless'
    )
    $missing = @($requiredFeatures | Where-Object { $_ -notin $allFeatures })
    if ($missing.Count -gt 0) {
        throw "Корпус не покрывает обязательные признаки: $($missing -join ', ')"
    }
}

$totalBytes = ($entries | Measure-Object -Property size_bytes -Sum).Sum
$verified | Format-Table -AutoSize
Write-Host ("Корпус '{0}': {1} файлов, {2:N3} МиБ, хеши и размеры совпадают." -f `
    $manifestData.corpus_id, $entries.Count, ($totalBytes / 1MB))
