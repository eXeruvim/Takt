[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'paused', 'minimized', 'large-playlist', 'large-artwork')]
    [string]$Scenario,

    [ValidateRange(10, 3600)]
    [int]$DurationSeconds = 120,

    [ValidateRange(100, 5000)]
    [int]$SampleIntervalMs = 250,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

    [ValidateSet('not-recorded', 'software', 'd3d11va-copy')]
    [string]$DecoderMode = 'not-recorded',

    [ValidateSet('not-recorded', 'integrated-opengl', 'native-hwnd-fallback')]
    [string]$RendererBackend = 'not-recorded',

    [ValidateSet('windowed', 'maximized', 'fullscreen', 'picture-in-picture')]
    [string]$WindowMode = 'windowed',

    [ValidateSet('system', 'dark', 'light')]
    [string]$Theme = 'system',

    [string]$MediaPath = '',

    [string]$WorkloadManifest = '',

    [ValidateLength(0, 1000)]
    [string]$Notes = '',

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 0,

    [switch]$AutomaticIdle,

    [switch]$CloseAfterMeasurement,

    [switch]$AllowUnverifiedBuild,

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

function Get-LinearTrendPerMinute {
    param(
        [Parameter(Mandatory = $true)]
        [object[]]$Samples,
        [Parameter(Mandatory = $true)]
        [string]$Property
    )

    if ($Samples.Count -lt 2) {
        return $null
    }
    $meanX = ($Samples | Measure-Object -Property elapsed_seconds -Average).Average
    $values = @($Samples | ForEach-Object {
        [double]$_.PSObject.Properties[$Property].Value
    })
    $meanY = ($values | Measure-Object -Average).Average
    $numerator = 0.0
    $denominator = 0.0
    for ($index = 0; $index -lt $Samples.Count; $index++) {
        $offsetX = [double]$Samples[$index].elapsed_seconds - $meanX
        $numerator += $offsetX * ($values[$index] - $meanY)
        $denominator += $offsetX * $offsetX
    }
    if ($denominator -le 0.0) {
        return $null
    }
    return [Math]::Round(($numerator / $denominator) * 60.0, 4)
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

function Get-OxideBuildDescriptor {
    param(
        [Parameter(Mandatory = $true)]
        [string]$FilePath,
        [Parameter(Mandatory = $true)]
        [string]$WorkingDirectory
    )

    $startInfo = [System.Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $FilePath
    $startInfo.WorkingDirectory = $WorkingDirectory
    $startInfo.UseShellExecute = $false
    $startInfo.CreateNoWindow = $true
    $startInfo.RedirectStandardOutput = $true
    $startInfo.RedirectStandardError = $true
    $startInfo.ArgumentList.Add('--build-info')

    $probe = [System.Diagnostics.Process]::new()
    $probe.StartInfo = $startInfo
    try {
        if (-not $probe.Start()) {
            throw 'процесс проверки не запущен'
        }
        $stdout = $probe.StandardOutput.ReadToEndAsync()
        $stderr = $probe.StandardError.ReadToEndAsync()
        if (-not $probe.WaitForExit(5000)) {
            $probe.Kill($true)
            $probe.WaitForExit()
            throw 'проверка версии не завершилась за 5 секунд; возможно, executable собран без поддержки --build-info'
        }
        $output = $stdout.Result.Trim()
        $errorOutput = $stderr.Result.Trim()
        if ($probe.ExitCode -ne 0) {
            throw "код $($probe.ExitCode): $errorOutput"
        }
        try {
            $descriptor = $output | ConvertFrom-Json -ErrorAction Stop
        }
        catch {
            throw "некорректный JSON build-info: $output"
        }
        if ($descriptor.schema -ne 1 -or [string]::IsNullOrWhiteSpace([string]$descriptor.git_commit)) {
            throw 'неподдерживаемая схема build-info'
        }
        return $descriptor
    }
    catch {
        throw "Не удалось проверить происхождение Oxide: $($_.Exception.Message)"
    }
    finally {
        $probe.Dispose()
    }
}

function Close-BenchmarkProcess {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process
    )

    $Process.Refresh()
    if ($Process.HasExited) {
        return
    }

    if (-not $Process.CloseMainWindow()) {
        throw "Не удалось запросить обычное закрытие Oxide PID $($Process.Id). Серия остановлена; закройте только это окно вручную."
    }

    if (-not $Process.WaitForExit(10000)) {
        throw "Oxide PID $($Process.Id) не завершился за 10 секунд. Серия остановлена, процесс принудительно не завершён."
    }

    Write-Host "Oxide PID $($Process.Id) корректно закрыт."
}

function Get-WindowSnapshot {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process
    )

    try {
        if (-not ('OxideBenchmark.NativeWindow' -as [type])) {
            Add-Type -TypeDefinition @'
using System;
using System.Runtime.InteropServices;

namespace OxideBenchmark {
    public static class NativeWindow {
        [StructLayout(LayoutKind.Sequential)]
        public struct Rect {
            public int Left;
            public int Top;
            public int Right;
            public int Bottom;
        }

        [DllImport("user32.dll")]
        public static extern uint GetDpiForWindow(IntPtr window);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool GetClientRect(IntPtr window, out Rect rect);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool IsZoomed(IntPtr window);

        [DllImport("user32.dll")]
        [return: MarshalAs(UnmanagedType.Bool)]
        public static extern bool IsIconic(IntPtr window);
    }
}
'@
        }

        $Process.Refresh()
        $handle = $Process.MainWindowHandle
        if ($handle -eq [IntPtr]::Zero) {
            return [ordered]@{ available = $false; reason = 'main window handle is unavailable' }
        }

        $rect = [OxideBenchmark.NativeWindow+Rect]::new()
        if (-not [OxideBenchmark.NativeWindow]::GetClientRect($handle, [ref]$rect)) {
            return [ordered]@{ available = $false; reason = 'GetClientRect failed' }
        }
        $dpi = [OxideBenchmark.NativeWindow]::GetDpiForWindow($handle)
        if ($dpi -eq 0) {
            $dpi = 96
        }
        $width = [Math]::Max(0, $rect.Right - $rect.Left)
        $height = [Math]::Max(0, $rect.Bottom - $rect.Top)
        return [ordered]@{
            available = $true
            dpi = $dpi
            scale_percent = [Math]::Round(($dpi / 96.0) * 100.0, 1)
            client_width_physical = $width
            client_height_physical = $height
            client_width_logical = [Math]::Round(($width * 96.0) / $dpi, 1)
            client_height_logical = [Math]::Round(($height * 96.0) / $dpi, 1)
            maximized = [OxideBenchmark.NativeWindow]::IsZoomed($handle)
            minimized = [OxideBenchmark.NativeWindow]::IsIconic($handle)
        }
    }
    catch {
        return [ordered]@{ available = $false; reason = $_.Exception.Message }
    }
}

function Get-MediaDescriptor {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    if ([string]::IsNullOrWhiteSpace($Path)) {
        return $null
    }
    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Тестовый медиафайл не найден: $resolved"
    }
    $item = Get-Item -LiteralPath $resolved -ErrorAction Stop
    return [ordered]@{
        extension = $item.Extension.ToLowerInvariant()
        size_bytes = $item.Length
        sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
    }
}

function Get-WorkloadDescriptor {
    param(
        [Parameter(Mandatory = $true)]
        [string]$Path
    )

    $resolved = [System.IO.Path]::GetFullPath($Path)
    if (-not (Test-Path -LiteralPath $resolved -PathType Leaf)) {
        throw "Паспорт тестовой нагрузки не найден: $resolved"
    }
    try {
        $manifest = Get-Content -LiteralPath $resolved -Raw -Encoding utf8 | ConvertFrom-Json -ErrorAction Stop
    }
    catch {
        throw "Не удалось прочитать паспорт тестовой нагрузки: $($_.Exception.Message)"
    }
    if ($manifest.schema -ne 1 -or $manifest.kind -ne 'oxide-large-playlist' -or $manifest.file_count -lt 1) {
        throw 'Неподдерживаемый паспорт тестовой нагрузки'
    }
    $fixtureDirectory = Split-Path -Parent $resolved
    $actualFileCount = @(Get-ChildItem -LiteralPath $fixtureDirectory -Filter '*.wav' -File -ErrorAction Stop).Count
    if ($actualFileCount -ne [int]$manifest.file_count) {
        throw "Паспорт ожидает $($manifest.file_count) WAV-файлов, найдено $actualFileCount. Fixture изменён или выбран не тот паспорт."
    }
    return [ordered]@{
        kind = [string]$manifest.kind
        file_count = [int]$manifest.file_count
        duration_seconds = [int]$manifest.duration_seconds
        sha256 = (Get-FileHash -LiteralPath $resolved -Algorithm SHA256).Hash
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
$videoScenario = $Scenario -eq 'video-1080p' -or $Scenario -eq 'video-4k'
if ($videoScenario -and ($DecoderMode -eq 'not-recorded' -or $RendererBackend -eq 'not-recorded')) {
    throw 'Для video-сценария явно укажите -DecoderMode и -RendererBackend.'
}
if ($AutomaticIdle -and $Scenario -ne 'idle') {
    throw '-AutomaticIdle разрешён только для сценария idle: медиа-сценарии требуют ручной проверки состояния перед замером.'
}
$largePlaylistScenario = $Scenario -eq 'large-playlist'
if ($largePlaylistScenario -and -not $ValidateOnly -and [string]::IsNullOrWhiteSpace($WorkloadManifest)) {
    throw 'Для large-playlist укажите -WorkloadManifest из каталога созданного fixture.'
}
$mediaDescriptor = if ([string]::IsNullOrWhiteSpace($MediaPath)) {
    $null
}
else {
    Get-MediaDescriptor -Path $MediaPath
}
$workloadDescriptor = if ([string]::IsNullOrWhiteSpace($WorkloadManifest)) {
    $null
}
else {
    Get-WorkloadDescriptor -Path $WorkloadManifest
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
$gitCommit = Invoke-TextCommand -FilePath 'git' -Arguments @('-C', $repository, 'rev-parse', 'HEAD')
$buildDescriptor = Get-OxideBuildDescriptor -FilePath $executablePath -WorkingDirectory $repository
if (-not $ValidateOnly -and -not $AllowUnverifiedBuild) {
    if ([string]$buildDescriptor.git_commit -ne $gitCommit) {
        throw "Profiling-сборка относится к commit $($buildDescriptor.git_commit), а рабочее дерево — к $gitCommit. Пересоберите после commit."
    }
    if ([string]$buildDescriptor.tracked_state -ne 'true') {
        throw "Profiling-сборка создана из состояния '$($buildDescriptor.tracked_state)', а не из чистого commit. Пересоберите чистое дерево."
    }
}
if ($ValidateOnly) {
    $probe = Get-Stats -Values @(1.0, 2.0, 3.0, 4.0)
    if ($probe.min -ne 1.0 -or $probe.p50 -ne 2.0 -or $probe.p95 -ne 4.0 -or $probe.max -ne 4.0) {
        throw 'Самопроверка расчёта статистики завершилась ошибкой'
    }
    $trendProbe = @(
        [pscustomobject]@{ elapsed_seconds = 0.0; working_set_mib = 10.0 },
        [pscustomobject]@{ elapsed_seconds = 30.0; working_set_mib = 11.0 },
        [pscustomobject]@{ elapsed_seconds = 60.0; working_set_mib = 12.0 }
    )
    if ((Get-LinearTrendPerMinute -Samples $trendProbe -Property 'working_set_mib') -ne 2.0) {
        throw 'Самопроверка тренда памяти завершилась ошибкой'
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
    'paused' { 'Откройте медиафайл, поставьте воспроизведение на паузу и не двигайте мышью.' }
    'minimized' { 'Откройте медиафайл, запустите воспроизведение и сверните Oxide на панель задач.' }
    'large-playlist' { 'Откройте подготовленную большую папку и оставьте очередь открытой на время замера.' }
    'large-artwork' { 'Откройте аудиофайл с крупной допустимой обложкой и дождитесь её отображения.' }
}

$process = Start-Process -FilePath $executablePath -WorkingDirectory $repository -WindowStyle Normal -PassThru
Write-Host "Oxide запущен с PID $($process.Id)."
Write-Host $instructions
if (-not $AutomaticIdle) {
    Read-Host 'Настройте сценарий, дождитесь стабилизации и нажмите Enter для начала замера'
}
if ($WarmupSeconds -gt 0) {
    Write-Host "Прогрев перед замером: $WarmupSeconds с."
    Start-Sleep -Seconds $WarmupSeconds
}
$process.Refresh()
if ($process.HasExited) {
    throw "Oxide завершился до начала замера с кодом $($process.ExitCode)"
}
$windowSnapshot = Get-WindowSnapshot -Process $process

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
$computer = Get-CimInstance Win32_ComputerSystem
$processors = @(Get-CimInstance Win32_Processor | ForEach-Object {
    [ordered]@{
        name = $_.Name
        physical_cores = $_.NumberOfCores
        logical_processors = $_.NumberOfLogicalProcessors
        max_clock_mhz = $_.MaxClockSpeed
    }
})
$gpus = @(Get-CimInstance Win32_VideoController | ForEach-Object {
    [ordered]@{
        name = $_.Name
        driver_version = $_.DriverVersion
        adapter_ram_mib = if ($null -ne $_.AdapterRAM) { [Math]::Round($_.AdapterRAM / 1MB, 0) } else { $null }
        current_resolution = if ($null -ne $_.CurrentHorizontalResolution -and $null -ne $_.CurrentVerticalResolution) {
            "$($_.CurrentHorizontalResolution)x$($_.CurrentVerticalResolution)"
        } else { $null }
        current_refresh_hz = $_.CurrentRefreshRate
    }
})
$drives = @(Get-CimInstance Win32_DiskDrive | ForEach-Object {
    [ordered]@{
        model = $_.Model
        media_type = $_.MediaType
        interface_type = $_.InterfaceType
        firmware_revision = $_.FirmwareRevision
        size_gib = if ($null -ne $_.Size) { [Math]::Round($_.Size / 1GB, 1) } else { $null }
    }
})
$powerPlan = Invoke-TextCommand -FilePath 'powercfg.exe' -Arguments @('/getactivescheme')
$powerLineStatus = try {
    Add-Type -AssemblyName System.Windows.Forms
    [System.Windows.Forms.SystemInformation]::PowerStatus.PowerLineStatus.ToString()
}
catch {
    "unavailable: $($_.Exception.Message)"
}
$workingTree = Invoke-TextCommand -FilePath 'git' -Arguments @('-C', $repository, 'status', '--porcelain', '--untracked-files=no')
$firstSample = if ($samples.Count -gt 0) { $samples[0] } else { $null }
$lastSample = if ($samples.Count -gt 0) { $samples[$samples.Count - 1] } else { $null }
$secondHalfSamples = if ($samples.Count -gt 1) {
    @($samples | Select-Object -Skip ([Math]::Floor($samples.Count / 2)))
}
else {
    @()
}
$summary = [ordered]@{
    schema = 2
    scenario = $Scenario
    started_utc = $startedAt.ToString('O')
    finished_utc = $finishedAt.ToString('O')
    requested_duration_seconds = $DurationSeconds
    sample_interval_ms = $SampleIntervalMs
    samples = $samples.Count
    run_configuration = [ordered]@{
        decoder_mode = $DecoderMode
        renderer_backend = $RendererBackend
        window_mode = $WindowMode
        theme = $Theme
        media = $mediaDescriptor
        workload = $workloadDescriptor
        notes = $Notes
    }
    cpu_percent_total_capacity = Get-Stats -Values @($samples | ForEach-Object { [double]$_.cpu_percent })
    working_set_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.working_set_mib })
    private_bytes_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.private_bytes_mib })
    memory_trend_mib_per_minute = [ordered]@{
        working_set = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples @($samples) -Property 'working_set_mib'
            second_half = Get-LinearTrendPerMinute -Samples $secondHalfSamples -Property 'working_set_mib'
        }
        private_bytes = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples @($samples) -Property 'private_bytes_mib'
            second_half = Get-LinearTrendPerMinute -Samples $secondHalfSamples -Property 'private_bytes_mib'
        }
    }
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
        executable_sha256 = (Get-FileHash -LiteralPath $executablePath -Algorithm SHA256).Hash
        logical_processors = $logicalProcessors
        processor = $processors
        gpu = $gpus
        storage = $drives
        os = $os.Caption
        os_version = $os.Version
        os_build = $os.BuildNumber
        os_architecture = $os.OSArchitecture
        physical_memory_mib = [Math]::Round($computer.TotalPhysicalMemory / 1MB, 0)
        active_power_plan = $powerPlan
        power_line_status = $powerLineStatus
        oxide_window = $windowSnapshot
        git_commit = $gitCommit
        build_provenance = $buildDescriptor
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
if ($windowSnapshot.available) {
    Write-Host ("Окружение: DPI {0}, масштаб {1}%, клиентская область {2}x{3} px." -f `
        $windowSnapshot.dpi, $windowSnapshot.scale_percent, `
        $windowSnapshot.client_width_physical, $windowSnapshot.client_height_physical)
}
else {
    Write-Warning "Параметры окна не зафиксированы: $($windowSnapshot.reason)"
}
if (-not $process.HasExited) {
    if ($CloseAfterMeasurement) {
        Close-BenchmarkProcess -Process $process
    }
    else {
        Write-Host 'Oxide оставлен открытым — закройте его вручную после нужных проверок.'
    }
}
