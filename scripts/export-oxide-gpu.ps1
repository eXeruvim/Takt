[CmdletBinding()]
param(
    [string]$TracePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Convert-WpaNumber {
    param([Parameter(Mandatory = $true)][string]$Value)

    $number = 0.0
    if ([double]::TryParse(
        $Value,
        [System.Globalization.NumberStyles]::Float,
        [System.Globalization.CultureInfo]::CurrentCulture,
        [ref]$number
    )) {
        return $number
    }
    return [double]::Parse(
        $Value,
        [System.Globalization.NumberStyles]::Float,
        [System.Globalization.CultureInfo]::InvariantCulture
    )
}

function Get-TracePassport {
    param(
        [Parameter(Mandatory = $true)][string]$Root,
        [string]$RequestedTrace
    )

    if (-not [string]::IsNullOrWhiteSpace($RequestedTrace)) {
        $trace = [System.IO.Path]::GetFullPath($RequestedTrace)
        if (-not (Test-Path -LiteralPath $trace -PathType Leaf)) {
            throw "ETL-трасса не найдена: $trace"
        }
        $passportPath = [System.IO.Path]::ChangeExtension($trace, '.json')
        if (-not (Test-Path -LiteralPath $passportPath -PathType Leaf)) {
            throw "Рядом с ETL нет паспорта: $passportPath"
        }
        return Get-Item -LiteralPath $passportPath -ErrorAction Stop
    }

    $passport = Get-ChildItem -LiteralPath $Root -Filter '*.json' -File -ErrorAction Stop |
        Sort-Object LastWriteTime -Descending |
        Where-Object {
            try {
                $candidate = Get-Content -LiteralPath $_.FullName -Raw -Encoding utf8 |
                    ConvertFrom-Json -ErrorAction Stop
                $candidate.kind -eq 'oxide-wpr-trace' -and $candidate.profile -eq 'GPU'
            }
            catch {
                $false
            }
        } |
        Select-Object -First 1
    if ($null -eq $passport) {
        throw 'Паспорт GPU-трассы не найден. Сначала выполните capture-oxide-wpr.ps1.'
    }
    return $passport
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$traceRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'benchmark-results\wpr'))
$passport = Get-TracePassport -Root $traceRoot -RequestedTrace $TracePath
$metadata = Get-Content -LiteralPath $passport.FullName -Raw -Encoding utf8 |
    ConvertFrom-Json -ErrorAction Stop
if ($metadata.kind -ne 'oxide-wpr-trace' -or $metadata.schema -ne 1 -or $metadata.profile -ne 'GPU') {
    throw "Паспорт не описывает поддерживаемую GPU-трассу: $($passport.FullName)"
}
if ([int]$metadata.process_id -le 0 -or [double]$metadata.actual_trace_duration_seconds -le 0) {
    throw 'В паспорте отсутствует корректный PID или длительность трассы.'
}

$trace = [System.IO.Path]::GetFullPath(
    (Join-Path $passport.DirectoryName ([string]$metadata.trace_file))
)
if (-not $trace.StartsWith(
    $traceRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
) -or -not (Test-Path -LiteralPath $trace -PathType Leaf)) {
    throw "Некорректный путь GPU-трассы: $trace"
}

$template = Join-Path $PSScriptRoot 'wpa\gpu-utilization.wpaProfile'
if (-not (Test-Path -LiteralPath $template -PathType Leaf)) {
    throw "Шаблон GPU-профиля не найден: $template"
}
$wpaExporter = Get-Command wpaexporter.exe -ErrorAction SilentlyContinue
if ($null -eq $wpaExporter) {
    throw 'WPA Exporter не найден. Установите Windows Performance Analyzer.'
}

$traceBaseName = [System.IO.Path]::GetFileNameWithoutExtension($trace)
$analysisRoot = Join-Path $traceRoot 'analysis'
$output = Join-Path $analysisRoot "$traceBaseName-gpu-$([DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss'))"
New-Item -ItemType Directory -Path $output -ErrorAction Stop | Out-Null
$derivedProfile = Join-Path $output 'oxide-gpu.wpaProfile'

[xml]$profile = Get-Content -LiteralPath $template -Raw -Encoding utf8
$graph = $profile.SelectSingleNode("//*[local-name()='Graph']")
if ($null -eq $graph) {
    throw 'Шаблон GPU-профиля не содержит граф.'
}
$preset = $graph.SelectSingleNode("./*[local-name()='Preset']")
if ($null -eq $preset) {
    throw 'Шаблон GPU-профиля повреждён.'
}
$oxideGraph = $graph.CloneNode($true)
$oxidePreset = $oxideGraph.SelectSingleNode("./*[local-name()='Preset']")
$oxidePreset.SetAttribute('Name', 'Oxide GPU by Type')
$oxidePreset.SetAttribute('InitialFilterQuery', "[Process]:=`"oxide.exe ($([int]$metadata.process_id))`"")
$oxidePreset.SetAttribute('InitialFilterShouldKeep', 'true')

$systemGraph = $graph.CloneNode($true)
$systemGraph.SetAttribute('GraphItemGuid', [guid]::NewGuid().ToString())
$systemPreset = $systemGraph.SelectSingleNode("./*[local-name()='Preset']")
$systemPreset.SetAttribute('Name', 'System GPU by Process and Type')
$systemPreset.RemoveAttribute('InitialFilterQuery')

$graphs = $graph.ParentNode
$graphs.RemoveAll()
[void]$graphs.AppendChild($oxideGraph)
[void]$graphs.AppendChild($systemGraph)
$profile.Save($derivedProfile)

$exportLog = & $wpaExporter.Source `
    -i $trace `
    -profile $derivedProfile `
    -outputfolder $output `
    -delimiter ';' `
    -outputformat CSV 2>&1
if ($LASTEXITCODE -ne 0) {
    throw "WPA Exporter завершился с кодом $LASTEXITCODE`: $($exportLog | Out-String)"
}

$oxideCsv = Get-ChildItem -LiteralPath $output -Filter '*Oxide_GPU_by_Type.csv' -File |
    Select-Object -First 1
$systemCsv = Get-ChildItem -LiteralPath $output -Filter '*System_GPU_by_Process_and_Type.csv' -File |
    Select-Object -First 1
if ($null -eq $oxideCsv -or $null -eq $systemCsv) {
    throw 'WPA Exporter не создал ожидаемые GPU-таблицы.'
}

$durationSeconds = [double]$metadata.actual_trace_duration_seconds
$oxideRows = @(Import-Csv -LiteralPath $oxideCsv.FullName -Delimiter ';')
$systemRows = @(Import-Csv -LiteralPath $systemCsv.FullName -Delimiter ';')
$summarize = {
    param([object]$Row)

    $microseconds = Convert-WpaNumber ([string]$Row.'GPU Time (µs)')
    $count = [int]$Row.Count
    [ordered]@{
        adapter = [string]$Row.Adapter
        process = [string]$Row.Process
        engine_type = [string]$Row.Type
        task_count = $count
        task_rate_hz = [math]::Round($count / $durationSeconds, 4)
        gpu_time_ms = [math]::Round($microseconds / 1000.0, 4)
        average_task_us = if ($count -gt 0) { [math]::Round($microseconds / $count, 4) } else { 0.0 }
        one_engine_timeline_percent = [math]::Round(
            ($microseconds / ($durationSeconds * 1000000.0)) * 100.0,
            4
        )
    }
}
$oxideEngines = @($oxideRows | Where-Object {
    -not [string]::IsNullOrWhiteSpace([string]$_.Type)
} | ForEach-Object { & $summarize $_ })
$systemEngines = @($systemRows | Where-Object {
    -not [string]::IsNullOrWhiteSpace([string]$_.Type)
} | ForEach-Object { & $summarize $_ })

$decodeObserved = @($systemEngines | Where-Object {
    $_.engine_type -match '(?i)decode|codec'
}).Count -gt 0
$copyObserved = @($systemEngines | Where-Object {
    $_.engine_type -match '(?i)copy|transfer|blt'
}).Count -gt 0
$warnings = [System.Collections.Generic.List[string]]::new()
if ([string]$metadata.decoder_mode -eq 'd3d11va-copy' -and -not $decodeObserved) {
    $warnings.Add(
        'Паспорт заявляет d3d11va-copy, но GPU-таблица не содержит Video Decode. ' +
        'Это может означать software fallback, неподдерживаемый кодек или отсутствие атрибуции драйвером; одна эта трасса не подтверждает аппаратное декодирование без парного software-прогона.'
    )
}

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-gpu-summary'
    source_trace = Split-Path -Leaf $trace
    source_passport = $passport.Name
    git_commit = [string]$metadata.git_commit
    package_version = [string]$metadata.package_version
    executable_sha256 = [string]$metadata.executable_sha256
    mpv_sha256 = [string]$metadata.mpv_sha256
    media = $metadata.media
    process_id = [int]$metadata.process_id
    scenario = [string]$metadata.scenario
    decoder_mode = [string]$metadata.decoder_mode
    renderer_backend = [string]$metadata.renderer_backend
    window_mode = [string]$metadata.window_mode
    theme = [string]$metadata.theme
    trace_duration_seconds = $durationSeconds
    video_decode_observed = $decodeObserved
    copy_or_transfer_observed = $copyObserved
    oxide_engines = $oxideEngines
    system_engines = $systemEngines
    warnings = @($warnings)
    metric_note = 'one_engine_timeline_percent is GPU time divided by trace time for one engine timeline; it is not whole-adapter utilization and overlapping engines must not be summed.'
}
$summaryPath = Join-Path $output 'oxide-gpu-summary.json'
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
Write-Host "GPU-сводка: $summaryPath"
Write-Host "Oxide GPU engines: $($oxideEngines.Count); Video Decode observed: $decodeObserved; Copy/Transfer observed: $copyObserved"
