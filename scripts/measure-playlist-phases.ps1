[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$Manifest,

    [ValidateRange(3, 1000)]
    [int]$Iterations = 25,

    [ValidateRange(1, 100)]
    [int]$WarmupIterations = 3,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\takt-playlist-bench.exe'),

    [string]$ResultRoot = (Join-Path $PSScriptRoot '..\benchmark-results'),

    [switch]$AllowDirty,

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$manifestPath = [System.IO.Path]::GetFullPath($Manifest)
$executablePath = [System.IO.Path]::GetFullPath($Executable)
$resultRootPath = [System.IO.Path]::GetFullPath($ResultRoot)
if (-not $resultRootPath.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог результатов оказался вне репозитория: $resultRootPath"
}
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Паспорт fixture не найден: $manifestPath"
}
if (-not (Test-Path -LiteralPath $executablePath -PathType Leaf)) {
    throw 'Инструмент не собран. Выполните: cargo build --profile profiling --features benchmark-tools --bin takt-playlist-bench --locked --offline'
}

try {
    $manifestData = Get-Content -LiteralPath $manifestPath -Raw -Encoding utf8 | ConvertFrom-Json -ErrorAction Stop
}
catch {
    throw "Не удалось прочитать паспорт fixture: $($_.Exception.Message)"
}
if ($manifestData.schema -ne 1 -or $manifestData.kind -ne 'takt-large-playlist' -or $manifestData.file_count -lt 1) {
    throw 'Неподдерживаемая схема паспорта fixture'
}
$fixtureDirectory = Split-Path -Parent $manifestPath
$actualFiles = @(Get-ChildItem -LiteralPath $fixtureDirectory -Filter '*.wav' -File -ErrorAction Stop).Count
if ($actualFiles -ne [int]$manifestData.file_count) {
    throw "Паспорт ожидает $($manifestData.file_count) WAV-файлов, найдено $actualFiles"
}
$workloadHash = (Get-FileHash -LiteralPath $manifestPath -Algorithm SHA256).Hash

try {
    $buildInfo = (& $executablePath --build-info 2>&1 | Out-String).Trim() | ConvertFrom-Json -ErrorAction Stop
}
catch {
    throw "Не удалось прочитать build-info инструмента: $($_.Exception.Message)"
}
if ($buildInfo.schema -ne 1 -or [string]::IsNullOrWhiteSpace([string]$buildInfo.git_commit)) {
    throw 'Инструмент вернул неподдерживаемую схему build-info'
}

if ($ValidateOnly) {
    Write-Host "Паспорт, $actualFiles WAV и build-info инструмента: OK"
    return
}

$gitCommit = (& git -C $repository rev-parse HEAD 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось определить текущий commit: $gitCommit"
}
$trackedChanges = (& git -C $repository status --porcelain --untracked-files=no 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось проверить состояние Git: $trackedChanges"
}
if (-not $AllowDirty) {
    if (-not [string]::IsNullOrWhiteSpace($trackedChanges)) {
        throw 'Для baseline нужны закоммиченные отслеживаемые файлы. Для чернового прогона используйте -AllowDirty.'
    }
    if ([string]$buildInfo.git_commit -ne $gitCommit -or [string]$buildInfo.tracked_state -ne 'true') {
        throw 'Profiling-инструмент не соответствует текущему чистому commit. Пересоберите его после commit.'
    }
}

New-Item -ItemType Directory -Path $resultRootPath -Force | Out-Null
$timestamp = [DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss')
$outputPath = Join-Path $resultRootPath "playlist-phases-$timestamp-$actualFiles-files.json"

& $executablePath `
    --directory $fixtureDirectory `
    --output $outputPath `
    --workload-sha256 $workloadHash `
    --expected-files $actualFiles `
    --iterations $Iterations `
    --warmup-iterations $WarmupIterations
if ($LASTEXITCODE -ne 0) {
    throw "Фазовый benchmark завершился с кодом $LASTEXITCODE"
}

$report = Get-Content -LiteralPath $outputPath -Raw -Encoding utf8 | ConvertFrom-Json
if ($report.schema -ne 1 -or $report.kind -ne 'takt-playlist-phases' -or $report.workload.sha256 -ne $workloadHash) {
    throw 'Фазовый benchmark записал некорректный отчёт'
}
Write-Host ("Фазы p50: scan {0:N3} мс · sort {1:N3} мс · labels {2:N3} мс · shared {3:N3} мс · model {4:N4} мс · total {5:N3} мс" -f `
    $report.metrics_ms.directory_scan.p50, $report.metrics_ms.natural_sort.p50, `
    $report.metrics_ms.ui_labels.p50, $report.metrics_ms.shared_strings.p50, `
    $report.metrics_ms.slint_vec_model.p50, $report.metrics_ms.total.p50)
