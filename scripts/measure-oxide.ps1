[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'large-playlist', 'large-artwork')]
    [string]$Scenario,

    [ValidateRange(10, 3600)]
    [int]$DurationSeconds = 120,

    [ValidateRange(100, 5000)]
    [int]$SampleIntervalMs = 250,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

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

function Get-Stats {
    param(
        [Parameter(Mandatory = $true)]
        [double[]]$Values
    )

    if ($Values.Count -eq 0) {
        return $null
    }
    return [ordered]@{
        min = [Math]::Round(($Values | Measure-Object -Minimum).Minimum, 3)
        p50 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.50), 3)
        p95 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.95), 3)
        p99 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.99), 3)
        max = [Math]::Round(($Values | Measure-Object -Maximum).Maximum, 3)
    }
}

function Invoke-TextCommand {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string[]]$Arguments
    )

    try {
        return (& $FilePath @Arguments 2>&1 | Out-String).Trim()
    }
    catch {
        return "unavailable: $($_.Exception.Message)"
    }
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$executablePath = [System.IO.Path]::GetFullPath($Executable)
$expectedExecutable = [System.IO.Path]::GetFullPath((Join-Path $repository 'target\profiling\oxide.exe'))
if ($executablePath -ne $expectedExecutable) {
    Write-Warning "Используется нестандартный executable: $executablePath"
}
if (-not (Test-Path -LiteralPath $executablePath -PathType Leaf)) {
    throw "Сборка не найдена. Выполните: cargo build --profile profiling --locked --offline"
}
$runtimePath = Join-Path (Split-Path -Parent $executablePath) 'libmpv-2.dll'
if (-not (Test-Path -LiteralPath $runtimePath -PathType Leaf)) {
    if ($executablePath -ne $expectedExecutable) {
        throw "Рядом с нестандартной сборкой отсутствует libmpv-2.dll: $runtimePath"
    }
    $runtimeSource = Join-Path $repository 'mpv-2.dll'
    if (-not (Test-Path -LiteralPath $runtimeSource -PathType Leaf)) {
        throw "Нативный runtime не найден: $runtimeSource"
    }
    [System.IO.File]::Copy($runtimeSource, $runtimePath, $false)
    Write-Host "Runtime подготовлен: $runtimePath"
}
if ($ValidateOnly) {
    $probe = Get-Stats -Values @(1.0, 2.0, 3.0, 4.0)
    if ($probe.min -ne 1.0 -or $probe.p50 -ne 2.0 -or $probe.p95 -ne 4.0 -or $probe.max -ne 4.0) {
        throw 'Самопроверка расчёта статистики завершилась ошибкой'
    }
    Write-Host 'Синтаксис, пути и расчёт статистики: OK'
    return
}

$resultRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'benchmark-results'))
if (-not $resultRoot.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог результатов оказался вне репозитория: $resultRoot"
}
New-Item -ItemType Directory -Path $resultRoot -Force | Out-Null

$instructions = switch ($Scenario) {
    'idle' { 'Не открывайте медиа и не двигайте мышью над окном.' }
    'audio' { 'Откройте обычный аудиофайл, запустите воспроизведение и дождитесь появления обложки.' }
    'video-1080p' { 'Откройте 1080p-видео, включите нужный режим декодирования и запустите воспроизведение.' }
    'video-4k' { 'Откройте 4K-видео, включите нужный режим декодирования и запустите воспроизведение.' }
    'large-playlist' { 'Откройте подготовленную большую папку и оставьте очередь открытой на время замера.' }
    'large-artwork' { 'Откройте аудиофайл с крупной допустимой обложкой и дождитесь её отображения.' }
}

$process = Start-Process -FilePath $executablePath -WorkingDirectory $repository -WindowStyle Normal -PassThru
Write-Host "Oxide запущен с PID $($process.Id)."
Write-Host $instructions
Read-Host 'Настройте сценарий, дождитесь стабилизации и нажмите Enter для начала замера'
$process.Refresh()
if ($process.HasExited) {
    throw "Oxide завершился до начала замера с кодом $($process.ExitCode)"
}

$logicalProcessors = [Math]::Max(1, [Environment]::ProcessorCount)
$samples = [System.Collections.Generic.List[object]]::new()
$startedAt = [DateTimeOffset]::UtcNow
$deadline = [Diagnostics.Stopwatch]::StartNew()
$previousWall = $deadline.Elapsed.TotalSeconds
$previousCpu = $process.TotalProcessorTime.TotalSeconds

while ($deadline.Elapsed.TotalSeconds -lt $DurationSeconds) {
    Start-Sleep -Milliseconds $SampleIntervalMs
    $process.Refresh()
    if ($process.HasExited) {
        Write-Warning 'Oxide завершился во время замера; сохранены полученные образцы.'
        break
    }

    $wall = $deadline.Elapsed.TotalSeconds
    $cpu = $process.TotalProcessorTime.TotalSeconds
    $wallDelta = $wall - $previousWall
    $cpuPercent = if ($wallDelta -gt 0.0) {
        (($cpu - $previousCpu) / $wallDelta / $logicalProcessors) * 100.0
    }
    else {
        0.0
    }
    $samples.Add([pscustomobject][ordered]@{
        elapsed_seconds = [Math]::Round($wall, 3)
        cpu_percent = [Math]::Round($cpuPercent, 3)
        working_set_mib = [Math]::Round($process.WorkingSet64 / 1MB, 3)
        private_bytes_mib = [Math]::Round($process.PrivateMemorySize64 / 1MB, 3)
        handles = $process.HandleCount
        threads = $process.Threads.Count
    })
    $percentComplete = [Math]::Min(100.0, ($wall / $DurationSeconds) * 100.0)
    Write-Progress `
        -Activity "Замер Oxide: $Scenario" `
        -Status ("{0:N1} из {1} секунд" -f $wall, $DurationSeconds) `
        -PercentComplete $percentComplete
    $previousWall = $wall
    $previousCpu = $cpu
}
Write-Progress -Activity "Замер Oxide: $Scenario" -Completed

$finishedAt = [DateTimeOffset]::UtcNow
$timestamp = $startedAt.ToLocalTime().ToString('yyyyMMdd-HHmmss')
$baseName = "$timestamp-$Scenario"
$csvPath = Join-Path $resultRoot "$baseName.csv"
$summaryPath = Join-Path $resultRoot "$baseName.json"
$samples | Export-Csv -LiteralPath $csvPath -NoTypeInformation -Encoding utf8

$cargoLockPath = Join-Path $repository 'Cargo.lock'
$os = Get-CimInstance Win32_OperatingSystem
$processors = @(Get-CimInstance Win32_Processor | Select-Object -ExpandProperty Name)
$gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object {
    [ordered]@{
        name = $_.Name
        driver_version = $_.DriverVersion
    }
})
$workingTree = Invoke-TextCommand -FilePath 'git' -Arguments @('-C', $repository, 'status', '--porcelain')
$firstSample = if ($samples.Count -gt 0) { $samples[0] } else { $null }
$lastSample = if ($samples.Count -gt 0) { $samples[$samples.Count - 1] } else { $null }
$summary = [ordered]@{
    schema = 1
    scenario = $Scenario
    started_utc = $startedAt.ToString('O')
    finished_utc = $finishedAt.ToString('O')
    requested_duration_seconds = $DurationSeconds
    sample_interval_ms = $SampleIntervalMs
    samples = $samples.Count
    cpu_percent_total_capacity = Get-Stats -Values @($samples | ForEach-Object { [double]$_.cpu_percent })
    working_set_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.working_set_mib })
    private_bytes_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.private_bytes_mib })
    start_to_end = if ($null -ne $firstSample -and $null -ne $lastSample) {
        [ordered]@{
            working_set_mib = [Math]::Round($lastSample.working_set_mib - $firstSample.working_set_mib, 3)
            private_bytes_mib = [Math]::Round($lastSample.private_bytes_mib - $firstSample.private_bytes_mib, 3)
            handles = $lastSample.handles - $firstSample.handles
            threads = $lastSample.threads - $firstSample.threads
        }
    }
    else {
        $null
    }
    handles_max = if ($samples.Count -gt 0) { ($samples.handles | Measure-Object -Maximum).Maximum } else { $null }
    threads_max = if ($samples.Count -gt 0) { ($samples.threads | Measure-Object -Maximum).Maximum } else { $null }
    environment = [ordered]@{
        executable = $executablePath
        logical_processors = $logicalProcessors
        processor = $processors
        gpu = $gpus
        os = $os.Caption
        os_version = $os.Version
        physical_memory_mib = [Math]::Round($os.TotalVisibleMemorySize / 1KB, 0)
        git_commit = Invoke-TextCommand -FilePath 'git' -Arguments @('-C', $repository, 'rev-parse', 'HEAD')
        working_tree_clean = [string]::IsNullOrWhiteSpace($workingTree)
        working_tree_changes = $workingTree
        rustc = Invoke-TextCommand -FilePath 'rustc' -Arguments @('-Vv')
        cargo = Invoke-TextCommand -FilePath 'cargo' -Arguments @('-V')
        cargo_lock_sha256 = (Get-FileHash -LiteralPath $cargoLockPath -Algorithm SHA256).Hash
        mpv_runtime = $runtimePath
        mpv_sha256 = (Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash
    }
}
$summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8

Write-Host "Готово: $csvPath"
Write-Host "Сводка: $summaryPath"
if (-not $process.HasExited) {
    Write-Host 'Oxide оставлен открытым — закройте его вручную после нужных проверок.'
}
