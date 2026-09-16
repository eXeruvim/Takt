[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'large-playlist', 'large-artwork')]
    [string]$Scenario,

    [ValidateRange(2, 30)]
    [int]$MinimumRuns = 5,

    [string]$ResultRoot = (Join-Path $PSScriptRoot '..\benchmark-results'),

    [switch]$IncludeDirty,

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Percentile {
    param(
        [Parameter(Mandatory = $true)]
        [double[]]$Values,
        [Parameter(Mandatory = $true)]
        [ValidateRange(0.0, 1.0)]
        [double]$Percentile
    )

    if ($Values.Count -eq 0) {
        return $null
    }
    $ordered = @($Values | Sort-Object)
    $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $ordered.Count) - 1)
    return $ordered[$index]
}

function Get-AggregateStats {
    param(
        [Parameter(Mandatory = $true)]
        [double[]]$Values
    )

    if ($Values.Count -eq 0) {
        return $null
    }
    return [ordered]@{
        runs = $Values.Count
        min = [Math]::Round(($Values | Measure-Object -Minimum).Minimum, 3)
        p50 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.50), 3)
        p95 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.95), 3)
        max = [Math]::Round(($Values | Measure-Object -Maximum).Maximum, 3)
        values = @($Values | ForEach-Object { [Math]::Round($_, 3) })
    }
}

function Get-NestedValue {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Object,
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $value = $Object
    foreach ($segment in $Path.Split('.')) {
        if ($null -eq $value) {
            return $null
        }
        $property = $value.PSObject.Properties[$segment]
        if ($null -eq $property) {
            return $null
        }
        $value = $property.Value
    }
    return $value
}

function Get-ObjectHash {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Object
    )

    $json = $Object | ConvertTo-Json -Depth 12 -Compress
    $bytes = [System.Text.Encoding]::UTF8.GetBytes($json)
    $algorithm = [System.Security.Cryptography.SHA256]::Create()
    try {
        return [Convert]::ToHexString($algorithm.ComputeHash($bytes))
    }
    finally {
        $algorithm.Dispose()
    }
}

function Get-RunSignature {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Run
    )

    return [ordered]@{
        scenario = $Run.scenario
        requested_duration_seconds = $Run.requested_duration_seconds
        sample_interval_ms = $Run.sample_interval_ms
        git_commit = $Run.environment.git_commit
        cargo_lock_sha256 = $Run.environment.cargo_lock_sha256
        executable_sha256 = $Run.environment.executable_sha256
        mpv_sha256 = $Run.environment.mpv_sha256
        decoder_mode = $Run.run_configuration.decoder_mode
        renderer_backend = $Run.run_configuration.renderer_backend
        window_mode = $Run.run_configuration.window_mode
        theme = $Run.run_configuration.theme
        media_sha256 = Get-NestedValue -Object $Run -Path 'run_configuration.media.sha256'
        workload_sha256 = Get-NestedValue -Object $Run -Path 'run_configuration.workload.sha256'
        os_version = $Run.environment.os_version
        os_build = $Run.environment.os_build
        processor = $Run.environment.processor
        gpu = $Run.environment.gpu
        storage = $Run.environment.storage
        physical_memory_mib = $Run.environment.physical_memory_mib
        active_power_plan = $Run.environment.active_power_plan
        power_line_status = $Run.environment.power_line_status
        window = $Run.environment.oxide_window
    }
}

if ($ValidateOnly) {
    $probe = Get-AggregateStats -Values @(1.0, 2.0, 3.0, 4.0, 5.0)
    if ($probe.min -ne 1.0 -or $probe.p50 -ne 3.0 -or $probe.p95 -ne 5.0 -or $probe.max -ne 5.0) {
        throw 'Самопроверка статистики серий завершилась ошибкой'
    }
    $firstHash = Get-ObjectHash -Object ([ordered]@{ a = 1; b = 2 })
    $sameHash = Get-ObjectHash -Object ([ordered]@{ a = 1; b = 2 })
    $otherHash = Get-ObjectHash -Object ([ordered]@{ a = 2; b = 1 })
    if ($firstHash -ne $sameHash -or $firstHash -eq $otherHash) {
        throw 'Самопроверка сигнатур окружения завершилась ошибкой'
    }
    Write-Host 'Синтаксис, статистика серий и сигнатуры: OK'
    return
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$resultRootPath = [System.IO.Path]::GetFullPath($ResultRoot)
if (-not $resultRootPath.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог результатов оказался вне репозитория: $resultRootPath"
}
if (-not (Test-Path -LiteralPath $resultRootPath -PathType Container)) {
    throw "Каталог результатов не найден: $resultRootPath"
}

$runs = [System.Collections.Generic.List[object]]::new()
$rejected = [System.Collections.Generic.List[object]]::new()
foreach ($file in Get-ChildItem -LiteralPath $resultRootPath -Filter '*.json' -File) {
    try {
        $run = Get-Content -LiteralPath $file.FullName -Raw -Encoding utf8 | ConvertFrom-Json
    }
    catch {
        $rejected.Add([pscustomobject]@{ file = $file.Name; reason = 'invalid JSON' })
        continue
    }
    if ($run.schema -ne 2 -or $run.scenario -ne $Scenario) {
        continue
    }
    if (-not $IncludeDirty -and -not $run.environment.working_tree_clean) {
        $rejected.Add([pscustomobject]@{ file = $file.Name; reason = 'dirty working tree' })
        continue
    }
    if ($run.samples -lt 2) {
        $rejected.Add([pscustomobject]@{ file = $file.Name; reason = 'too few samples' })
        continue
    }
    $expectedSamples = ($run.requested_duration_seconds * 1000.0) / $run.sample_interval_ms
    if ($run.samples -lt [Math]::Floor($expectedSamples * 0.9)) {
        $rejected.Add([pscustomobject]@{ file = $file.Name; reason = 'incomplete measurement' })
        continue
    }

    $started = if ($run.started_utc -is [DateTime]) {
        [DateTimeOffset]$run.started_utc
    }
    else {
        [DateTimeOffset]::Parse(
            [string]$run.started_utc,
            [Globalization.CultureInfo]::InvariantCulture,
            [Globalization.DateTimeStyles]::RoundtripKind
        )
    }
    $signature = Get-RunSignature -Run $run
    $runs.Add([pscustomobject]@{
        file = $file.Name
        started = $started
        data = $run
        signature = $signature
        signature_hash = Get-ObjectHash -Object $signature
    })
}

if ($runs.Count -eq 0) {
    throw "Для сценария $Scenario нет подходящих schema 2 результатов. Отклонено: $($rejected.Count)."
}

$metricPaths = [ordered]@{
    cpu_sample_p50_percent = 'cpu_percent_total_capacity.p50'
    cpu_sample_p95_percent = 'cpu_percent_total_capacity.p95'
    cpu_sample_p99_percent = 'cpu_percent_total_capacity.p99'
    working_set_p50_mib = 'working_set_mib.p50'
    working_set_p95_mib = 'working_set_mib.p95'
    private_bytes_p50_mib = 'private_bytes_mib.p50'
    private_bytes_p95_mib = 'private_bytes_mib.p95'
    working_set_delta_mib = 'start_to_end.working_set_mib'
    private_bytes_delta_mib = 'start_to_end.private_bytes_mib'
    handles_delta = 'start_to_end.handles'
    threads_delta = 'start_to_end.threads'
    handles_max = 'handles_max'
    threads_max = 'threads_max'
}

$written = [System.Collections.Generic.List[string]]::new()
foreach ($group in $runs | Group-Object -Property signature_hash) {
    $groupRuns = @($group.Group | Sort-Object started)
    if ($groupRuns.Count -lt $MinimumRuns) {
        Write-Warning "Группа $($group.Name.Substring(0, 12)) содержит $($groupRuns.Count) из $MinimumRuns требуемых прогонов."
        continue
    }

    $metrics = [ordered]@{}
    foreach ($metric in $metricPaths.GetEnumerator()) {
        $values = @($groupRuns | ForEach-Object {
            $value = Get-NestedValue -Object $_.data -Path $metric.Value
            if ($null -ne $value) { [double]$value }
        })
        $metrics[$metric.Key] = Get-AggregateStats -Values $values
    }

    $first = $groupRuns[0]
    $last = $groupRuns[$groupRuns.Count - 1]
    $shortCommit = ([string]$first.signature.git_commit).Substring(0, [Math]::Min(8, ([string]$first.signature.git_commit).Length))
    $timestamp = $last.started.ToLocalTime().ToString('yyyyMMdd-HHmmss')
    $fileName = "aggregate-$timestamp-$Scenario-$shortCommit-$($groupRuns.Count)runs.json"
    $outputPath = Join-Path $resultRootPath $fileName
    $aggregate = [ordered]@{
        schema = 1
        kind = if ($IncludeDirty) { 'pilot-aggregate' } else { 'baseline-candidate' }
        created_utc = [DateTimeOffset]::UtcNow.ToString('O')
        scenario = $Scenario
        signature_hash = $group.Name
        signature = $first.signature
        run_count = $groupRuns.Count
        first_started_utc = $first.started.ToString('O')
        last_started_utc = $last.started.ToString('O')
        metrics = $metrics
        source_files = @($groupRuns.file)
    }
    $aggregate | ConvertTo-Json -Depth 12 | Set-Content -LiteralPath $outputPath -Encoding utf8
    $written.Add($outputPath)
    Write-Host "Серия готова: $outputPath"
    Write-Host ("CPU p50 по прогонам: {0}% · CPU p95: {1}% · private bytes p50: {2} МиБ" -f `
        $metrics.cpu_sample_p50_percent.p50, $metrics.cpu_sample_p95_percent.p50, `
        $metrics.private_bytes_p50_mib.p50)
}

if ($written.Count -eq 0) {
    throw "Ни одна сопоставимая группа не набрала минимум $MinimumRuns прогонов."
}
