[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [string]$SoftwareSummaryPath,

    [Parameter(Mandatory = $true)]
    [string]$HardwareSummaryPath,

    [string]$OutputPath = ''
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Read-GpuSummary {
    param([Parameter(Mandatory = $true)][string]$Path)

    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "GPU-сводка не найдена: $resolved"
    }
    $summary = Get-Content -LiteralPath $resolved -Raw -Encoding utf8 |
        ConvertFrom-Json -ErrorAction Stop
    if ($summary.schema -ne 1 -or $summary.kind -ne 'oxide-gpu-summary') {
        throw "Неподдерживаемая GPU-сводка: $resolved"
    }
    return [pscustomobject]@{
        path = $resolved
        data = $summary
    }
}

function Assert-SameValue {
    param(
        [Parameter(Mandatory = $true)][string]$Name,
        $SoftwareValue,
        $HardwareValue
    )

    if ([string]$SoftwareValue -ne [string]$HardwareValue) {
        throw "A/B несопоставим: $Name отличается ('$SoftwareValue' и '$HardwareValue')."
    }
}

function Get-EngineTotals {
    param([object[]]$Engines)

    $totals = @{}
    foreach ($engine in $Engines) {
        $type = [string]$engine.engine_type
        if ([string]::IsNullOrWhiteSpace($type)) {
            continue
        }
        if (-not $totals.ContainsKey($type)) {
            $totals[$type] = [ordered]@{
                task_count = 0
                gpu_time_ms = 0.0
            }
        }
        $totals[$type].task_count += [int]$engine.task_count
        $totals[$type].gpu_time_ms += [double]$engine.gpu_time_ms
    }
    return $totals
}

function Compare-EngineTotals {
    param(
        [hashtable]$Software,
        [hashtable]$Hardware,
        [double]$SoftwareDuration,
        [double]$HardwareDuration
    )

    $types = @($Software.Keys) + @($Hardware.Keys) | Sort-Object -Unique
    return @($types | ForEach-Object {
        $type = $_
        $softwareCount = if ($Software.ContainsKey($type)) { $Software[$type].task_count } else { 0 }
        $hardwareCount = if ($Hardware.ContainsKey($type)) { $Hardware[$type].task_count } else { 0 }
        $softwareTime = if ($Software.ContainsKey($type)) { $Software[$type].gpu_time_ms } else { 0.0 }
        $hardwareTime = if ($Hardware.ContainsKey($type)) { $Hardware[$type].gpu_time_ms } else { 0.0 }
        [ordered]@{
            engine_type = $type
            software_task_count = $softwareCount
            hardware_task_count = $hardwareCount
            delta_task_count = $hardwareCount - $softwareCount
            software_task_rate_hz = [math]::Round($softwareCount / $SoftwareDuration, 4)
            hardware_task_rate_hz = [math]::Round($hardwareCount / $HardwareDuration, 4)
            software_gpu_time_ms = [math]::Round($softwareTime, 4)
            hardware_gpu_time_ms = [math]::Round($hardwareTime, 4)
            delta_gpu_time_ms = [math]::Round($hardwareTime - $softwareTime, 4)
        }
    })
}

$software = Read-GpuSummary -Path $SoftwareSummaryPath
$hardware = Read-GpuSummary -Path $HardwareSummaryPath
if ([string]$software.data.decoder_mode -ne 'software') {
    throw "Первая сводка имеет decoder_mode '$($software.data.decoder_mode)', ожидался software."
}
$hardwareDecoderModes = @('hardware-auto-copy', 'nvdec', 'nvdec-copy', 'd3d11va-copy')
if ([string]$hardware.data.decoder_mode -notin $hardwareDecoderModes) {
    throw "Вторая сводка имеет decoder_mode '$($hardware.data.decoder_mode)', ожидался аппаратный copy-back режим."
}

Assert-SameValue 'git_commit' $software.data.git_commit $hardware.data.git_commit
Assert-SameValue 'executable_sha256' $software.data.executable_sha256 $hardware.data.executable_sha256
Assert-SameValue 'mpv_sha256' $software.data.mpv_sha256 $hardware.data.mpv_sha256
Assert-SameValue 'scenario' $software.data.scenario $hardware.data.scenario
Assert-SameValue 'renderer_backend' $software.data.renderer_backend $hardware.data.renderer_backend
Assert-SameValue 'window_mode' $software.data.window_mode $hardware.data.window_mode
Assert-SameValue 'theme' $software.data.theme $hardware.data.theme
Assert-SameValue 'media.sha256' $software.data.media.sha256 $hardware.data.media.sha256
Assert-SameValue 'media.size_bytes' $software.data.media.size_bytes $hardware.data.media.size_bytes

$softwareDuration = [double]$software.data.trace_duration_seconds
$hardwareDuration = [double]$hardware.data.trace_duration_seconds
$oxideComparison = Compare-EngineTotals `
    -Software (Get-EngineTotals @($software.data.oxide_engines)) `
    -Hardware (Get-EngineTotals @($hardware.data.oxide_engines)) `
    -SoftwareDuration $softwareDuration `
    -HardwareDuration $hardwareDuration
$systemComparison = Compare-EngineTotals `
    -Software (Get-EngineTotals @($software.data.system_engines)) `
    -Hardware (Get-EngineTotals @($hardware.data.system_engines)) `
    -SoftwareDuration $softwareDuration `
    -HardwareDuration $hardwareDuration

$softwareOxideTasks = 0
foreach ($engine in @($software.data.oxide_engines)) {
    $softwareOxideTasks += [int]$engine.task_count
}
$hardwareOxideTasks = 0
foreach ($engine in @($hardware.data.oxide_engines)) {
    $hardwareOxideTasks += [int]$engine.task_count
}
$directXHardwareActivityObserved = $hardwareOxideTasks -gt 0 -and `
    $hardwareOxideTasks -gt $softwareOxideTasks

$comparison = [ordered]@{
    schema = 1
    kind = 'oxide-gpu-comparison'
    software_summary = [System.IO.Path]::GetFileName($software.path)
    hardware_summary = [System.IO.Path]::GetFileName($hardware.path)
    software_trace = [string]$software.data.source_trace
    hardware_trace = [string]$hardware.data.source_trace
    git_commit = [string]$hardware.data.git_commit
    executable_sha256 = [string]$hardware.data.executable_sha256
    media_sha256 = [string]$hardware.data.media.sha256
    scenario = [string]$hardware.data.scenario
    renderer_backend = [string]$hardware.data.renderer_backend
    window_mode = [string]$hardware.data.window_mode
    theme = [string]$hardware.data.theme
    software_duration_seconds = $softwareDuration
    hardware_duration_seconds = $hardwareDuration
    directx_hardware_path_activity_observed = $directXHardwareActivityObserved
    video_decode_engine_label_observed = [bool]$hardware.data.video_decode_observed
    oxide_engine_deltas = @($oxideComparison)
    system_engine_deltas = @($systemComparison)
    interpretation = if ($directXHardwareActivityObserved) {
        'Парный прогон подтверждает дополнительную GPU-работу в аппаратном copy-back режиме. Метка движка отражает атрибуцию драйвера, а не обязательно физический блок декодирования.'
    }
    else {
        'Парный прогон не подтверждает дополнительную GPU-работу в аппаратном copy-back режиме.'
    }
    limitation = 'Сравнение не содержит CPU, dropped frames, энергии и dedicated/shared GPU memory.'
}

$resolvedOutput = if ([string]::IsNullOrWhiteSpace($OutputPath)) {
    Join-Path (Split-Path -Parent $hardware.path) 'oxide-gpu-comparison.json'
}
else {
    [System.IO.Path]::GetFullPath($OutputPath)
}
$comparison | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $resolvedOutput -Encoding utf8
Write-Host "GPU A/B: $resolvedOutput"
Write-Host "Дополнительная DirectX GPU-активность аппаратного режима: $directXHardwareActivityObserved"
