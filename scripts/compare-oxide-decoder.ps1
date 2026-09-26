[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SoftwareResultPath,

    [Parameter(Mandatory = $true)]
    [string]$HardwareResultPath,

    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-Measurement {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Результат измерения не найден: $resolved"
    }
    $measurement = Get-Content -LiteralPath $resolved -Raw -Encoding utf8 |
        ConvertFrom-Json -ErrorAction Stop
    if ($measurement.schema -ne 2) {
        throw "Неподдерживаемый результат измерения: $resolved"
    }
    if ($measurement.samples -lt 2) {
        throw "В результате слишком мало образцов: $resolved"
    }
    $expectedSamples = ($measurement.requested_duration_seconds * 1000.0) /
        $measurement.sample_interval_ms
    if ($measurement.samples -lt [Math]::Floor($expectedSamples * 0.9)) {
        throw "Измерение завершилось раньше времени: $resolved"
    }
    if (-not $measurement.environment.working_tree_clean) {
        throw "Измерение выполнено с изменённым рабочим деревом: $resolved"
    }
    return [pscustomobject]@{
        path = $resolved
        data = $measurement
    }
}

function Get-NestedValue {
    param(
        [Parameter(Mandatory = $true)][object]$Object,
        [Parameter(Mandatory = $true)][string]$Path
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

function ConvertTo-ComparableString {
    param($Value)

    if ($null -eq $Value) {
        return '<null>'
    }
    if ($Value -is [string] -or $Value -is [ValueType]) {
        return [string]$Value
    }
    return ($Value | ConvertTo-Json -Depth 10 -Compress)
}

function Assert-SameValue {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        $SoftwareValue,
        $HardwareValue
    )

    $softwareText = ConvertTo-ComparableString $SoftwareValue
    $hardwareText = ConvertTo-ComparableString $HardwareValue
    if ($softwareText -ne $hardwareText) {
        throw "A/B несопоставим: $Name отличается ('$softwareText' и '$hardwareText')."
    }
}

function Compare-Metric {
    param(
        [Parameter(Mandatory = $true)][double]$Software,
        [Parameter(Mandatory = $true)][double]$Hardware
    )

    $delta = $Hardware - $Software
    return [ordered]@{
        software = [Math]::Round($Software, 3)
        hardware = [Math]::Round($Hardware, 3)
        delta = [Math]::Round($delta, 3)
        delta_percent = if ([Math]::Abs($Software) -gt 0.000001) {
            [Math]::Round(($delta / $Software) * 100.0, 2)
        }
        else {
            $null
        }
    }
}

function Compare-OptionalMetric {
    param(
        $Software,
        $Hardware
    )

    if ($null -eq $Software -or $null -eq $Hardware) {
        return [ordered]@{
            available = $false
            software = $Software
            hardware = $Hardware
            delta = $null
            delta_percent = $null
        }
    }
    $comparison = Compare-Metric -Software ([double]$Software) -Hardware ([double]$Hardware)
    return [ordered]@{
        available = $true
        software = $comparison.software
        hardware = $comparison.hardware
        delta = $comparison.delta
        delta_percent = $comparison.delta_percent
    }
}

$software = Read-Measurement -Path $SoftwareResultPath
$hardware = Read-Measurement -Path $HardwareResultPath
if ([string]$software.data.run_configuration.decoder_mode -ne 'software') {
    throw "Первый результат имеет decoder_mode '$($software.data.run_configuration.decoder_mode)', ожидался software."
}
if ([string]$hardware.data.run_configuration.decoder_mode -ne 'd3d11va-copy') {
    throw "Второй результат имеет decoder_mode '$($hardware.data.run_configuration.decoder_mode)', ожидался d3d11va-copy."
}

$identityPaths = [ordered]@{
    scenario = 'scenario'
    requested_duration_seconds = 'requested_duration_seconds'
    sample_interval_ms = 'sample_interval_ms'
    git_commit = 'environment.git_commit'
    executable_sha256 = 'environment.executable_sha256'
    cargo_lock_sha256 = 'environment.cargo_lock_sha256'
    mpv_sha256 = 'environment.mpv_sha256'
    renderer_backend = 'run_configuration.renderer_backend'
    window_mode = 'run_configuration.window_mode'
    theme = 'run_configuration.theme'
    media_sha256 = 'run_configuration.media.sha256'
    media_size_bytes = 'run_configuration.media.size_bytes'
    os_build = 'environment.os_build'
    logical_processors = 'environment.logical_processors'
    processor = 'environment.processor'
    gpu = 'environment.gpu'
    active_power_plan = 'environment.active_power_plan'
    power_line_status = 'environment.power_line_status'
    oxide_window = 'environment.oxide_window'
}
foreach ($identity in $identityPaths.GetEnumerator()) {
    Assert-SameValue `
        -Name $identity.Key `
        -SoftwareValue (Get-NestedValue -Object $software.data -Path $identity.Value) `
        -HardwareValue (Get-NestedValue -Object $hardware.data -Path $identity.Value)
}

$metricPaths = [ordered]@{
    cpu_p50_percent = 'cpu_percent_total_capacity.p50'
    cpu_p95_percent = 'cpu_percent_total_capacity.p95'
    cpu_p99_percent = 'cpu_percent_total_capacity.p99'
    working_set_p50_mib = 'working_set_mib.p50'
    working_set_p95_mib = 'working_set_mib.p95'
    private_bytes_p50_mib = 'private_bytes_mib.p50'
    private_bytes_p95_mib = 'private_bytes_mib.p95'
    working_set_second_half_mib_per_minute = 'memory_trend_mib_per_minute.working_set.second_half'
    private_bytes_second_half_mib_per_minute = 'memory_trend_mib_per_minute.private_bytes.second_half'
    handles_max = 'handles_max'
    threads_max = 'threads_max'
}
$metrics = [ordered]@{}
foreach ($metric in $metricPaths.GetEnumerator()) {
    $softwareValue = Get-NestedValue -Object $software.data -Path $metric.Value
    $hardwareValue = Get-NestedValue -Object $hardware.data -Path $metric.Value
    if ($null -eq $softwareValue -or $null -eq $hardwareValue) {
        throw "В паре отсутствует метрика $($metric.Key)."
    }
    $metrics[$metric.Key] = Compare-Metric `
        -Software ([double]$softwareValue) `
        -Hardware ([double]$hardwareValue)
}

$hardwareImprovesCpu = $metrics.cpu_p95_percent.delta -lt 0
$hardwareImprovesMemory = $metrics.working_set_p95_mib.delta -lt 0
$interpretation = if ($hardwareImprovesCpu -and $hardwareImprovesMemory) {
    'В этом парном прогоне аппаратный режим снизил CPU p95 и Working Set p95.'
}
elseif (-not $hardwareImprovesCpu -and -not $hardwareImprovesMemory) {
    'В этом парном прогоне аппаратный режим не снизил CPU p95 или Working Set p95.'
}
else {
    'В этом парном прогоне аппаратный режим дал разнонаправленный результат по CPU p95 и Working Set p95.'
}

$softwareDiagnostics = Get-NestedValue -Object $software.data -Path 'playback_diagnostics'
$hardwareDiagnostics = Get-NestedValue -Object $hardware.data -Path 'playback_diagnostics'
$frameDiagnostics = $null
if ($null -ne $softwareDiagnostics -and $null -ne $hardwareDiagnostics) {
    if ($softwareDiagnostics.schema -ne 1 -or $hardwareDiagnostics.schema -ne 1 `
        -or $softwareDiagnostics.kind -ne 'oxide-mpv-playback-diagnostics' `
        -or $hardwareDiagnostics.kind -ne 'oxide-mpv-playback-diagnostics') {
        throw 'A/B содержит неподдерживаемую схему playback_diagnostics.'
    }
    $frameDiagnostics = [ordered]@{
        available = $true
        software_actual_decoder = $softwareDiagnostics.decoder
        hardware_actual_decoder = $hardwareDiagnostics.decoder
        hardware_decoder_confirmed = -not [string]::IsNullOrWhiteSpace(
            [string]$hardwareDiagnostics.decoder
        ) -and [string]$hardwareDiagnostics.decoder -notin @('no', 'false')
        video_sync = [ordered]@{
            software = $softwareDiagnostics.video_sync
            hardware = $hardwareDiagnostics.video_sync
        }
        frame_drop_count = Compare-OptionalMetric `
            $softwareDiagnostics.frame_drop_count `
            $hardwareDiagnostics.frame_drop_count
        decoder_frame_drop_count = Compare-OptionalMetric `
            $softwareDiagnostics.decoder_frame_drop_count `
            $hardwareDiagnostics.decoder_frame_drop_count
        mistimed_frame_count = Compare-OptionalMetric `
            $softwareDiagnostics.mistimed_frame_count `
            $hardwareDiagnostics.mistimed_frame_count
        vo_delayed_frame_count = Compare-OptionalMetric `
            $softwareDiagnostics.vo_delayed_frame_count `
            $hardwareDiagnostics.vo_delayed_frame_count
        container_fps = Compare-OptionalMetric `
            $softwareDiagnostics.container_fps `
            $hardwareDiagnostics.container_fps
        estimated_vf_fps = Compare-OptionalMetric `
            $softwareDiagnostics.estimated_vf_fps `
            $hardwareDiagnostics.estimated_vf_fps
        avsync_seconds = Compare-OptionalMetric `
            $softwareDiagnostics.avsync_seconds `
            $hardwareDiagnostics.avsync_seconds
        total_avsync_change_seconds = Compare-OptionalMetric `
            $softwareDiagnostics.total_avsync_change_seconds `
            $hardwareDiagnostics.total_avsync_change_seconds
    }
}

$comparison = [ordered]@{
    schema = 1
    kind = 'oxide-decoder-comparison'
    created_utc = [DateTimeOffset]::UtcNow.ToString('O')
    software_result = [System.IO.Path]::GetFileName($software.path)
    hardware_result = [System.IO.Path]::GetFileName($hardware.path)
    git_commit = [string]$hardware.data.environment.git_commit
    executable_sha256 = [string]$hardware.data.environment.executable_sha256
    media_sha256 = [string]$hardware.data.run_configuration.media.sha256
    scenario = [string]$hardware.data.scenario
    duration_seconds = [double]$hardware.data.requested_duration_seconds
    sample_interval_ms = [int]$hardware.data.sample_interval_ms
    renderer_backend = [string]$hardware.data.run_configuration.renderer_backend
    window_mode = [string]$hardware.data.run_configuration.window_mode
    theme = [string]$hardware.data.run_configuration.theme
    metrics = $metrics
    playback_diagnostics = $frameDiagnostics
    interpretation = $interpretation
    limitation = 'Одна пара показывает этот файл и запуск, но не заменяет серию из пяти и более прогонов, PresentMon/WPA frame-time, GPU/VRAM и energy-профиль. mpv delayed count является эвристикой.'
}

$resolvedOutput = if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    Join-Path (Split-Path -Parent $hardware.path) 'oxide-decoder-comparison.json'
}
else {
    [System.IO.Path]::GetFullPath($OutputPath)
}
$comparison | ConvertTo-Json -Depth 10 | Set-Content -LiteralPath $resolvedOutput -Encoding utf8

Write-Host "Decoder A/B: $resolvedOutput"
Write-Host $interpretation
Write-Host ("CPU p95: {0}% -> {1}% ({2:+0.###;-0.###;0} п.п.)" -f `
    $metrics.cpu_p95_percent.software, $metrics.cpu_p95_percent.hardware, `
    $metrics.cpu_p95_percent.delta)
Write-Host ("Working Set p95: {0} -> {1} МиБ ({2:+0.###;-0.###;0} МиБ)" -f `
    $metrics.working_set_p95_mib.software, $metrics.working_set_p95_mib.hardware, `
    $metrics.working_set_p95_mib.delta)
if ($null -ne $frameDiagnostics) {
    Write-Host ("Декодеры mpv: {0} -> {1}; VO dropped: {2} -> {3}; delayed: {4} -> {5}." -f `
        $frameDiagnostics.software_actual_decoder, `
        $frameDiagnostics.hardware_actual_decoder, `
        $frameDiagnostics.frame_drop_count.software, `
        $frameDiagnostics.frame_drop_count.hardware, `
        $frameDiagnostics.vo_delayed_frame_count.software, `
        $frameDiagnostics.vo_delayed_frame_count.hardware)
}
