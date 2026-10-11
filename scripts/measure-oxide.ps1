[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'hdd-video', 'paused', 'minimized', 'large-playlist', 'large-artwork', 'artwork-release', 'ui-panels')]
    [string]$Scenario,

    [ValidateRange(10, 3600)]
    [int]$DurationSeconds = 120,

    [ValidateRange(100, 5000)]
    [int]$SampleIntervalMs = 250,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

    [ValidateSet('not-recorded', 'software', 'hardware-auto-copy', 'nvdec', 'nvdec-copy', 'd3d11va-copy')]
    [string]$DecoderMode = 'not-recorded',

    [ValidateSet('not-recorded', 'integrated-opengl', 'native-hwnd-fallback')]
    [string]$RendererBackend = 'not-recorded',

    [ValidateSet('windowed', 'maximized', 'fullscreen', 'picture-in-picture')]
    [string]$WindowMode = 'windowed',

    [ValidateSet('system', 'dark', 'light')]
    [string]$Theme = 'system',

    [ValidateSet('baseline', 'balanced')]
    [string]$LocalCacheMode = 'baseline',

    [string]$MediaPath = '',

    [string]$ComparisonMediaPath = '',

    [string]$WorkloadManifest = '',

    [ValidateLength(0, 1000)]
    [string]$Notes = '',

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 0,

    [ValidateRange(1, 10000)]
    [int]$UiCycleCount = 100,

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

function Get-DecoderValidationError {
    param(
        [Parameter(Mandatory = $true)]
        [string]$RequestedMode,
        [AllowNull()]
        [AllowEmptyString()]
        [string]$ActualDecoder
    )

    if ($RequestedMode -eq 'not-recorded') {
        return $null
    }

    $actual = $ActualDecoder.Trim().ToLowerInvariant()
    $softwareDecoders = @('', 'no', 'false', '0')
    if ($RequestedMode -eq 'software') {
        if ($actual -notin $softwareDecoders) {
            return "Запрошено программное декодирование, но mpv использовал '$ActualDecoder'. Проверьте настройку и повторите замер."
        }
        return $null
    }

    if ($actual -in $softwareDecoders) {
        return "Запрошено аппаратное декодирование '$RequestedMode', но mpv использовал программный декодер ('$ActualDecoder'). Включите аппаратное декодирование и повторите замер."
    }
    if ($RequestedMode -in @('nvdec', 'nvdec-copy', 'd3d11va-copy') -and $actual -ne $RequestedMode) {
        return "Запрошен декодер '$RequestedMode', но mpv выбрал '$ActualDecoder'. Повторите замер с указанным декодером или используйте режим hardware-auto-copy."
    }
    return $null
}

function New-GpuMemorySampler {
    param(
        [Parameter(Mandatory = $true)]
        [int]$ProcessId
    )

    try {
        Add-Type -AssemblyName System.Diagnostics.PerformanceCounter
        $category = [System.Diagnostics.PerformanceCounterCategory]::new(
            'GPU Process Memory'
        )
        $prefix = "pid_$($ProcessId)_"
        $instances = @($category.GetInstanceNames() | Where-Object {
            $_.StartsWith($prefix, [System.StringComparison]::OrdinalIgnoreCase)
        })
        $counterNames = [ordered]@{
            dedicated_usage_mib = 'Dedicated Usage'
            shared_usage_mib = 'Shared Usage'
            local_usage_mib = 'Local Usage'
            non_local_usage_mib = 'Non Local Usage'
            total_committed_mib = 'Total Committed'
        }
        $counters = [System.Collections.Generic.List[object]]::new()
        foreach ($instance in $instances) {
            foreach ($entry in $counterNames.GetEnumerator()) {
                $counter = [System.Diagnostics.PerformanceCounter]::new(
                    'GPU Process Memory',
                    $entry.Value,
                    $instance,
                    $true
                )
                $counters.Add([pscustomobject]@{
                    property = $entry.Key
                    counter = $counter
                })
            }
        }
        return [pscustomobject]@{
            available = $counters.Count -gt 0
            instances = $instances
            counters = $counters
            error = if ($counters.Count -gt 0) {
                $null
            }
            else {
                "Для PID $ProcessId не найдены счётчики GPU Process Memory."
            }
        }
    }
    catch {
        return [pscustomobject]@{
            available = $false
            instances = @()
            counters = @()
            error = $_.Exception.Message
        }
    }
}

function Read-GpuMemorySample {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Sampler
    )

    $result = [ordered]@{
        available = $false
        dedicated_usage_mib = $null
        shared_usage_mib = $null
        local_usage_mib = $null
        non_local_usage_mib = $null
        total_committed_mib = $null
    }
    if (-not $Sampler.available) {
        return [pscustomobject]$result
    }

    try {
        $totals = @{}
        foreach ($entry in $Sampler.counters) {
            if (-not $totals.ContainsKey($entry.property)) {
                $totals[$entry.property] = 0.0
            }
            $totals[$entry.property] += [double]$entry.counter.NextValue()
        }
        foreach ($property in @(
            'dedicated_usage_mib',
            'shared_usage_mib',
            'local_usage_mib',
            'non_local_usage_mib',
            'total_committed_mib'
        )) {
            $result[$property] = [Math]::Round($totals[$property] / 1MB, 3)
        }
        $result.available = $true
    }
    catch {
        $Sampler.error = $_.Exception.Message
    }
    return [pscustomobject]$result
}

function Close-GpuMemorySampler {
    param(
        [Parameter(Mandatory = $true)]
        [object]$Sampler
    )

    foreach ($entry in $Sampler.counters) {
        $entry.counter.Dispose()
    }
}

function Measure-ProcessPhase {
    param(
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Process]$Process,
        [Parameter(Mandatory = $true)]
        [object]$GpuMemorySampler,
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [System.Collections.Generic.List[object]]$Samples,
        [Parameter(Mandatory = $true)]
        [string]$Phase,
        [Parameter(Mandatory = $true)]
        [int]$DurationSeconds,
        [Parameter(Mandatory = $true)]
        [int]$SampleIntervalMs,
        [Parameter(Mandatory = $true)]
        [int]$LogicalProcessors,
        [Parameter(Mandatory = $true)]
        [string]$Scenario,
        [Parameter(Mandatory = $true)]
        [System.Diagnostics.Stopwatch]$TotalStopwatch
    )

    $phaseStopwatch = [Diagnostics.Stopwatch]::StartNew()
    $previousWall = $phaseStopwatch.Elapsed.TotalSeconds
    $previousCpu = $Process.TotalProcessorTime.TotalSeconds
    while ($phaseStopwatch.Elapsed.TotalSeconds -lt $DurationSeconds) {
        Start-Sleep -Milliseconds $SampleIntervalMs
        $Process.Refresh()
        if ($Process.HasExited) {
            Write-Warning 'Oxide завершился во время замера; сохранены полученные образцы.'
            return $false
        }

        $wall = $phaseStopwatch.Elapsed.TotalSeconds
        $cpu = $Process.TotalProcessorTime.TotalSeconds
        $wallDelta = $wall - $previousWall
        $cpuPercent = if ($wallDelta -gt 0.0) {
            (($cpu - $previousCpu) / $wallDelta / $LogicalProcessors) * 100.0
        }
        else {
            0.0
        }
        $gpuMemory = Read-GpuMemorySample -Sampler $GpuMemorySampler
        $Samples.Add([pscustomobject][ordered]@{
            elapsed_seconds = [Math]::Round($TotalStopwatch.Elapsed.TotalSeconds, 3)
            phase = $Phase
            phase_elapsed_seconds = [Math]::Round($wall, 3)
            cpu_percent = [Math]::Round($cpuPercent, 3)
            working_set_mib = [Math]::Round($Process.WorkingSet64 / 1MB, 3)
            private_bytes_mib = [Math]::Round($Process.PrivateMemorySize64 / 1MB, 3)
            gpu_memory_available = $gpuMemory.available
            gpu_dedicated_usage_mib = $gpuMemory.dedicated_usage_mib
            gpu_shared_usage_mib = $gpuMemory.shared_usage_mib
            gpu_local_usage_mib = $gpuMemory.local_usage_mib
            gpu_non_local_usage_mib = $gpuMemory.non_local_usage_mib
            gpu_total_committed_mib = $gpuMemory.total_committed_mib
            handles = $Process.HandleCount
            threads = $Process.Threads.Count
        })
        $percentComplete = [Math]::Min(100.0, ($wall / $DurationSeconds) * 100.0)
        Write-Progress `
            -Activity "Замер Oxide: $Scenario" `
            -Status ("{0}: {1:N1} из {2} секунд" -f $Phase, $wall, $DurationSeconds) `
            -PercentComplete $percentComplete
        $previousWall = $wall
        $previousCpu = $cpu
    }
    return $true
}

function Get-MemoryPhaseSummary {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
        [object[]]$Samples
    )

    if ($Samples.Count -eq 0) {
        return $null
    }
    $gpuSamples = @($Samples | Where-Object { $_.gpu_memory_available })
    $first = $Samples[0]
    $last = $Samples[$Samples.Count - 1]
    return [ordered]@{
        samples = $Samples.Count
        working_set_mib = Get-Stats -Values @($Samples | ForEach-Object { [double]$_.working_set_mib })
        private_bytes_mib = Get-Stats -Values @($Samples | ForEach-Object { [double]$_.private_bytes_mib })
        gpu_dedicated_mib = if ($gpuSamples.Count -gt 0) {
            Get-Stats -Values @($gpuSamples | ForEach-Object { [double]$_.gpu_dedicated_usage_mib })
        } else { $null }
        gpu_shared_mib = if ($gpuSamples.Count -gt 0) {
            Get-Stats -Values @($gpuSamples | ForEach-Object { [double]$_.gpu_shared_usage_mib })
        } else { $null }
        gpu_total_committed_mib = if ($gpuSamples.Count -gt 0) {
            Get-Stats -Values @($gpuSamples | ForEach-Object { [double]$_.gpu_total_committed_mib })
        } else { $null }
        start_to_end = [ordered]@{
            working_set_mib = [Math]::Round($last.working_set_mib - $first.working_set_mib, 3)
            private_bytes_mib = [Math]::Round($last.private_bytes_mib - $first.private_bytes_mib, 3)
            gpu_dedicated_mib = if ($first.gpu_memory_available -and $last.gpu_memory_available) {
                [Math]::Round($last.gpu_dedicated_usage_mib - $first.gpu_dedicated_usage_mib, 3)
            } else { $null }
            gpu_shared_mib = if ($first.gpu_memory_available -and $last.gpu_memory_available) {
                [Math]::Round($last.gpu_shared_usage_mib - $first.gpu_shared_usage_mib, 3)
            } else { $null }
        }
    }
}

function Get-LinearTrendPerMinute {
    param(
        [Parameter(Mandatory = $true)]
        [AllowEmptyCollection()]
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
$videoScenario = $Scenario -in @('video-1080p', 'video-4k', 'hdd-video')
if ($videoScenario -and ($DecoderMode -eq 'not-recorded' -or $RendererBackend -eq 'not-recorded')) {
    throw 'Для video-сценария явно укажите -DecoderMode и -RendererBackend.'
}
if ($LocalCacheMode -ne 'baseline' -and -not $videoScenario) {
    throw '-LocalCacheMode balanced разрешён только для video-сценариев.'
}
if ($AutomaticIdle -and $Scenario -ne 'idle') {
    throw '-AutomaticIdle разрешён только для сценария idle: медиа-сценарии требуют ручной проверки состояния перед замером.'
}
$largePlaylistScenario = $Scenario -eq 'large-playlist'
if ($largePlaylistScenario -and -not $ValidateOnly -and [string]::IsNullOrWhiteSpace($WorkloadManifest)) {
    throw 'Для large-playlist укажите -WorkloadManifest из каталога созданного fixture.'
}
$artworkReleaseScenario = $Scenario -eq 'artwork-release'
$uiPanelsScenario = $Scenario -eq 'ui-panels'
if ($artworkReleaseScenario -and -not $ValidateOnly) {
    if ([string]::IsNullOrWhiteSpace($MediaPath) -or
        [string]::IsNullOrWhiteSpace($ComparisonMediaPath)) {
        throw 'Для artwork-release укажите -MediaPath файла с обложкой и -ComparisonMediaPath файла без обложки.'
    }
}
if ($uiPanelsScenario -and -not $ValidateOnly) {
    if (-not $CloseAfterMeasurement) {
        throw 'Для ui-panels укажите -CloseAfterMeasurement: автоматический цикл синхронизируется с окном замера.'
    }
    if ([string]::IsNullOrWhiteSpace($MediaPath)) {
        throw 'Для ui-panels укажите -MediaPath тестового медиафайла, который будет записан в паспорт замера.'
    }
    $minimumUiCycleDuration = [Math]::Ceiling($UiCycleCount * 3 * 0.38) + 5
    if ($DurationSeconds -lt $minimumUiCycleDuration) {
        throw "Для $UiCycleCount циклов требуется -DurationSeconds не меньше $minimumUiCycleDuration."
    }
}
$mediaDescriptor = if ([string]::IsNullOrWhiteSpace($MediaPath)) {
    $null
}
else {
    Get-MediaDescriptor -Path $MediaPath
}
$comparisonMediaDescriptor = if ([string]::IsNullOrWhiteSpace($ComparisonMediaPath)) {
    $null
}
else {
    Get-MediaDescriptor -Path $ComparisonMediaPath
}
if ($artworkReleaseScenario -and -not $ValidateOnly -and
    $mediaDescriptor.sha256 -eq $comparisonMediaDescriptor.sha256) {
    throw 'Для artwork-release нужны два разных файла.'
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
    $emptyPhaseSamples = [System.Collections.Generic.List[object]]::new()
    $emptyPhaseAccepted = Measure-ProcessPhase `
        -Process ([Diagnostics.Process]::GetCurrentProcess()) `
        -GpuMemorySampler ([pscustomobject]@{ available = $false; counters = @() }) `
        -Samples $emptyPhaseSamples `
        -Phase 'binding-probe' `
        -DurationSeconds 0 `
        -SampleIntervalMs 100 `
        -LogicalProcessors 1 `
        -Scenario 'artwork-release' `
        -TotalStopwatch ([Diagnostics.Stopwatch]::StartNew())
    if (-not $emptyPhaseAccepted -or $emptyPhaseSamples.Count -ne 0) {
        throw 'Самопроверка пустой стартовой коллекции фазового замера завершилась ошибкой'
    }
    $phaseProbe = Get-MemoryPhaseSummary -Samples @(
        [pscustomobject]@{
            working_set_mib = 10.0; private_bytes_mib = 20.0
            gpu_memory_available = $false; gpu_dedicated_usage_mib = $null
            gpu_shared_usage_mib = $null
        },
        [pscustomobject]@{
            working_set_mib = 12.0; private_bytes_mib = 24.0
            gpu_memory_available = $false; gpu_dedicated_usage_mib = $null
            gpu_shared_usage_mib = $null
        }
    )
    if ($phaseProbe.working_set_mib.p50 -ne 10.0 -or
        $phaseProbe.start_to_end.private_bytes_mib -ne 4.0) {
        throw 'Самопроверка фазового замера памяти завершилась ошибкой'
    }
    $gpuCounterSet = Get-Counter -ListSet 'GPU Process Memory' -ErrorAction Stop
    foreach ($counterName in @(
        'Dedicated Usage',
        'Shared Usage',
        'Local Usage',
        'Non Local Usage',
        'Total Committed'
    )) {
        if (-not @($gpuCounterSet.Counter | Where-Object {
            $_ -like "*\$counterName"
        })) {
            throw "Системный счётчик GPU Process Memory/$counterName не найден"
        }
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
    'hdd-video' { 'Откройте видео именно с HDD. После начала замера: пауза на 5 секунд, продолжение, через 10 секунд одна перемотка и затем обычное воспроизведение.' }
    'paused' { 'Откройте медиафайл, поставьте воспроизведение на паузу и не двигайте мышью.' }
    'minimized' { 'Откройте медиафайл, запустите воспроизведение и сверните Oxide на панель задач.' }
    'large-playlist' { 'Откройте подготовленную большую папку и оставьте очередь открытой на время замера.' }
    'large-artwork' { 'Откройте аудиофайл с крупной допустимой обложкой и дождитесь её отображения.' }
    'artwork-release' { 'Откройте указанный аудиофайл с обложкой и дождитесь её отображения.' }
    'ui-panels' { 'Откройте указанный медиафайл и оставьте все боковые панели закрытыми. После Enter Oxide сам выполнит одинаковые циклы панелей.' }
}

$mediaDiagnosticsScenario = $Scenario -ne 'idle'
$playbackDiagnosticsPath = if ($mediaDiagnosticsScenario -and $CloseAfterMeasurement) {
    Join-Path $resultRoot ".mpv-diagnostics-$([Guid]::NewGuid().ToString('N')).json"
}
else {
    $null
}
$diagnosticsVariable = 'OXIDE_BENCHMARK_DIAGNOSTICS_PATH'
$renderGateVariable = 'OXIDE_BENCHMARK_RENDER_GATE'
$localCacheVariable = 'OXIDE_BENCHMARK_LOCAL_CACHE'
$hardwareDecoderVariable = 'OXIDE_BENCHMARK_HWDEC'
$uiCycleVariable = 'OXIDE_BENCHMARK_UI_CYCLES'
$renderGateName = if ($null -ne $playbackDiagnosticsPath) {
    "Local\OxideBenchmarkRender-$([Guid]::NewGuid().ToString('N'))"
}
else {
    $null
}
$renderGate = if ($null -ne $renderGateName) {
    [System.Threading.EventWaitHandle]::new(
        $false,
        [System.Threading.EventResetMode]::ManualReset,
        $renderGateName
    )
}
else {
    $null
}
$oldDiagnosticsPath = [Environment]::GetEnvironmentVariable(
    $diagnosticsVariable,
    [EnvironmentVariableTarget]::Process
)
$oldRenderGateName = [Environment]::GetEnvironmentVariable(
    $renderGateVariable,
    [EnvironmentVariableTarget]::Process
)
$oldLocalCacheMode = [Environment]::GetEnvironmentVariable(
    $localCacheVariable,
    [EnvironmentVariableTarget]::Process
)
$oldHardwareDecoder = [Environment]::GetEnvironmentVariable(
    $hardwareDecoderVariable,
    [EnvironmentVariableTarget]::Process
)
$oldUiCycleCount = [Environment]::GetEnvironmentVariable(
    $uiCycleVariable,
    [EnvironmentVariableTarget]::Process
)
try {
    [Environment]::SetEnvironmentVariable(
        $diagnosticsVariable,
        $playbackDiagnosticsPath,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $renderGateVariable,
        $renderGateName,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $localCacheVariable,
        $(if ($LocalCacheMode -eq 'balanced') { 'balanced' } else { $null }),
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $hardwareDecoderVariable,
        $(if ($DecoderMode -eq 'nvdec') { 'nvdec' } else { $null }),
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $uiCycleVariable,
        $(if ($uiPanelsScenario) { $UiCycleCount.ToString() } else { $null }),
        [EnvironmentVariableTarget]::Process
    )
    $process = Start-Process -FilePath $executablePath -WorkingDirectory $repository -WindowStyle Normal -PassThru
}
finally {
    [Environment]::SetEnvironmentVariable(
        $diagnosticsVariable,
        $oldDiagnosticsPath,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $renderGateVariable,
        $oldRenderGateName,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $localCacheVariable,
        $oldLocalCacheMode,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $hardwareDecoderVariable,
        $oldHardwareDecoder,
        [EnvironmentVariableTarget]::Process
    )
    [Environment]::SetEnvironmentVariable(
        $uiCycleVariable,
        $oldUiCycleCount,
        [EnvironmentVariableTarget]::Process
    )
}
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
$gpuMemorySampler = New-GpuMemorySampler -ProcessId $process.Id
if (-not $gpuMemorySampler.available) {
    Write-Warning "GPU memory не измеряется: $($gpuMemorySampler.error)"
}

$logicalProcessors = [Math]::Max(1, [Environment]::ProcessorCount)
$samples = [System.Collections.Generic.List[object]]::new()
if ($null -ne $renderGate) {
    $renderGate.Set() | Out-Null
}
$startedAt = [DateTimeOffset]::UtcNow
$totalStopwatch = [Diagnostics.Stopwatch]::StartNew()

try {
    if ($artworkReleaseScenario) {
        $phaseDurationSeconds = [Math]::Floor($DurationSeconds / 2)
        $firstPhaseCompleted = Measure-ProcessPhase `
            -Process $process `
            -GpuMemorySampler $gpuMemorySampler `
            -Samples $samples `
            -Phase 'with-artwork' `
            -DurationSeconds $phaseDurationSeconds `
            -SampleIntervalMs $SampleIntervalMs `
            -LogicalProcessors $logicalProcessors `
            -Scenario $Scenario `
            -TotalStopwatch $totalStopwatch
        if ($firstPhaseCompleted) {
            Write-Progress -Activity "Замер Oxide: $Scenario" -Completed
            Write-Host ''
            Write-Host 'Первая фаза готова. Теперь откройте указанный файл БЕЗ обложки.'
            Read-Host 'Дождитесь появления заглушки обложки и нажмите Enter для второй фазы'
            $process.Refresh()
            if (-not $process.HasExited) {
                [void](Measure-ProcessPhase `
                    -Process $process `
                    -GpuMemorySampler $gpuMemorySampler `
                    -Samples $samples `
                    -Phase 'without-artwork' `
                    -DurationSeconds ($DurationSeconds - $phaseDurationSeconds) `
                    -SampleIntervalMs $SampleIntervalMs `
                    -LogicalProcessors $logicalProcessors `
                    -Scenario $Scenario `
                    -TotalStopwatch $totalStopwatch)
            }
        }
    }
    else {
        [void](Measure-ProcessPhase `
            -Process $process `
            -GpuMemorySampler $gpuMemorySampler `
            -Samples $samples `
            -Phase 'measurement' `
            -DurationSeconds $DurationSeconds `
            -SampleIntervalMs $SampleIntervalMs `
            -LogicalProcessors $logicalProcessors `
            -Scenario $Scenario `
            -TotalStopwatch $totalStopwatch)
    }
}
finally {
    if ($null -ne $renderGate) {
        $renderGate.Reset() | Out-Null
    }
    Close-GpuMemorySampler -Sampler $gpuMemorySampler
    Write-Progress -Activity "Замер Oxide: $Scenario" -Completed
}

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
$gpuMemorySamples = @($samples | Where-Object { $_.gpu_memory_available })
$gpuMemorySecondHalfSamples = if ($gpuMemorySamples.Count -gt 1) {
    @($gpuMemorySamples | Select-Object -Skip ([Math]::Floor($gpuMemorySamples.Count / 2)))
}
else {
    @()
}
$withArtworkSamples = @($samples | Where-Object { $_.phase -eq 'with-artwork' })
$withoutArtworkSamples = @($samples | Where-Object { $_.phase -eq 'without-artwork' })
$withArtworkSummary = if ($withArtworkSamples.Count -gt 0) {
    Get-MemoryPhaseSummary -Samples $withArtworkSamples
}
else {
    $null
}
$withoutArtworkSummary = if ($withoutArtworkSamples.Count -gt 0) {
    Get-MemoryPhaseSummary -Samples $withoutArtworkSamples
}
else {
    $null
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
        local_cache_mode = $LocalCacheMode
        media = $mediaDescriptor
        comparison_media = $comparisonMediaDescriptor
        workload = $workloadDescriptor
        ui_cycle_count = if ($uiPanelsScenario) { $UiCycleCount } else { $null }
        notes = $Notes
    }
    cpu_percent_total_capacity = Get-Stats -Values @($samples | ForEach-Object { [double]$_.cpu_percent })
    working_set_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.working_set_mib })
    private_bytes_mib = Get-Stats -Values @($samples | ForEach-Object { [double]$_.private_bytes_mib })
    gpu_process_memory = [ordered]@{
        available = $gpuMemorySamples.Count -gt 0
        sample_count = $gpuMemorySamples.Count
        instance_count = $gpuMemorySampler.instances.Count
        dedicated_usage_mib = if ($gpuMemorySamples.Count -gt 0) {
            Get-Stats -Values @($gpuMemorySamples | ForEach-Object { [double]$_.gpu_dedicated_usage_mib })
        } else { $null }
        shared_usage_mib = if ($gpuMemorySamples.Count -gt 0) {
            Get-Stats -Values @($gpuMemorySamples | ForEach-Object { [double]$_.gpu_shared_usage_mib })
        } else { $null }
        local_usage_mib = if ($gpuMemorySamples.Count -gt 0) {
            Get-Stats -Values @($gpuMemorySamples | ForEach-Object { [double]$_.gpu_local_usage_mib })
        } else { $null }
        non_local_usage_mib = if ($gpuMemorySamples.Count -gt 0) {
            Get-Stats -Values @($gpuMemorySamples | ForEach-Object { [double]$_.gpu_non_local_usage_mib })
        } else { $null }
        total_committed_mib = if ($gpuMemorySamples.Count -gt 0) {
            Get-Stats -Values @($gpuMemorySamples | ForEach-Object { [double]$_.gpu_total_committed_mib })
        } else { $null }
        error = $gpuMemorySampler.error
        metric_note = 'Dedicated/shared and local/non-local are two Windows classifications of the same GPU allocations and must not be added to each other.'
    }
    memory_trend_mib_per_minute = [ordered]@{
        working_set = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples @($samples) -Property 'working_set_mib'
            second_half = Get-LinearTrendPerMinute -Samples $secondHalfSamples -Property 'working_set_mib'
        }
        private_bytes = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples @($samples) -Property 'private_bytes_mib'
            second_half = Get-LinearTrendPerMinute -Samples $secondHalfSamples -Property 'private_bytes_mib'
        }
        gpu_dedicated = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples $gpuMemorySamples -Property 'gpu_dedicated_usage_mib'
            second_half = Get-LinearTrendPerMinute -Samples $gpuMemorySecondHalfSamples -Property 'gpu_dedicated_usage_mib'
        }
        gpu_shared = [ordered]@{
            overall = Get-LinearTrendPerMinute -Samples $gpuMemorySamples -Property 'gpu_shared_usage_mib'
            second_half = Get-LinearTrendPerMinute -Samples $gpuMemorySecondHalfSamples -Property 'gpu_shared_usage_mib'
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
    phase_results = if ($artworkReleaseScenario) {
        [ordered]@{
            with_artwork = $withArtworkSummary
            without_artwork = $withoutArtworkSummary
            without_minus_with_p50_mib = if ($null -ne $withArtworkSummary -and
                $null -ne $withoutArtworkSummary) {
                [ordered]@{
                    working_set = [Math]::Round(
                        $withoutArtworkSummary.working_set_mib.p50 -
                            $withArtworkSummary.working_set_mib.p50,
                        3
                    )
                    private_bytes = [Math]::Round(
                        $withoutArtworkSummary.private_bytes_mib.p50 -
                            $withArtworkSummary.private_bytes_mib.p50,
                        3
                    )
                    gpu_dedicated = if ($null -ne $withArtworkSummary.gpu_dedicated_mib -and
                        $null -ne $withoutArtworkSummary.gpu_dedicated_mib) {
                        [Math]::Round(
                            $withoutArtworkSummary.gpu_dedicated_mib.p50 -
                                $withArtworkSummary.gpu_dedicated_mib.p50,
                            3
                        )
                    } else { $null }
                    gpu_shared = if ($null -ne $withArtworkSummary.gpu_shared_mib -and
                        $null -ne $withoutArtworkSummary.gpu_shared_mib) {
                        [Math]::Round(
                            $withoutArtworkSummary.gpu_shared_mib.p50 -
                                $withArtworkSummary.gpu_shared_mib.p50,
                            3
                        )
                    } else { $null }
                    gpu_total_committed = if ($null -ne $withArtworkSummary.gpu_total_committed_mib -and
                        $null -ne $withoutArtworkSummary.gpu_total_committed_mib) {
                        [Math]::Round(
                            $withoutArtworkSummary.gpu_total_committed_mib.p50 -
                                $withArtworkSummary.gpu_total_committed_mib.p50,
                            3
                        )
                    } else { $null }
                }
            }
            else {
                $null
            }
            metric_note = 'Negative without-minus-with values indicate memory released after switching to a file without artwork.'
        }
    }
    else {
        $null
    }
    handles_max = if ($samples.Count -gt 0) { ($samples.handles | Measure-Object -Maximum).Maximum } else { $null }
    threads_max = if ($samples.Count -gt 0) { ($samples.threads | Measure-Object -Maximum).Maximum } else { $null }
    playback_diagnostics = $null
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
if ($artworkReleaseScenario -and $null -ne $summary.phase_results.without_minus_with_p50_mib) {
    $delta = $summary.phase_results.without_minus_with_p50_mib
    Write-Host ("После снятия обложки, p50 delta: WS {0} МиБ · Private {1} МиБ · GPU total {2} МиБ" -f `
        $delta.working_set, $delta.private_bytes, $delta.gpu_total_committed)
}
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
$decoderValidationError = $null
$uiCycleValidationError = $null
if ($null -ne $playbackDiagnosticsPath) {
    if (Test-Path -LiteralPath $playbackDiagnosticsPath -PathType Leaf) {
        try {
            $playbackDiagnostics = Get-Content `
                -LiteralPath $playbackDiagnosticsPath `
                -Raw `
                -Encoding utf8 | ConvertFrom-Json -ErrorAction Stop
            if ($playbackDiagnostics.schema -ne 1 `
                -or $playbackDiagnostics.kind -ne 'oxide-mpv-playback-diagnostics') {
                throw 'неподдерживаемая схема'
            }
            $summary.playback_diagnostics = $playbackDiagnostics
            $decoderValidationError = Get-DecoderValidationError `
                -RequestedMode $DecoderMode `
                -ActualDecoder ([string]$playbackDiagnostics.decoder)
            $summary | ConvertTo-Json -Depth 8 | Set-Content -LiteralPath $summaryPath -Encoding utf8
            Write-Host ("Кадры mpv: VO dropped={0}; decoder dropped={1}; mistimed={2}; delayed={3}." -f `
                $playbackDiagnostics.frame_drop_count, `
                $playbackDiagnostics.decoder_frame_drop_count, `
                $playbackDiagnostics.mistimed_frame_count, `
                $playbackDiagnostics.vo_delayed_frame_count)
            Write-Host ("I/O mpv: cache pauses={0}, total={1} ms, max={2} ms; seeking max={3} ms." -f `
                $playbackDiagnostics.demuxer_cache.pause_count, `
                $playbackDiagnostics.demuxer_cache.pause_total_ms, `
                $playbackDiagnostics.demuxer_cache.pause_max_ms, `
                $playbackDiagnostics.seeking.max_ms)
            if ($null -ne $playbackDiagnostics.render_timing.scene_by_surface) {
                foreach ($surfaceName in @('empty', 'video', 'audio')) {
                    $surface = $playbackDiagnostics.render_timing.scene_by_surface.$surfaceName
                    if ($null -ne $surface -and [int64]$surface.rendered_frames -gt 0) {
                        Write-Host ("Рендер {0}: кадров={1}; interval p95={2} мс; scene p95={3} мс; Slint+notifier p95={4} мс." -f `
                            $surfaceName, `
                            $surface.rendered_frames, `
                            $surface.frame_interval_ms.p95, `
                            $surface.notifier_to_scene_complete_ms.p95, `
                            $surface.slint_scene_and_notifier_overhead_ms.p95)
                    }
                }
            }
            if ($uiPanelsScenario) {
                $uiCycles = $playbackDiagnostics.render_timing.ui_cycles
                Write-Host ("Циклы панелей: {0}/{1}." -f $uiCycles.completed, $uiCycles.requested)
                if (-not $uiCycles.complete -or
                    [int64]$uiCycles.requested -ne $UiCycleCount -or
                    [int64]$uiCycles.completed -ne $UiCycleCount) {
                    $uiCycleValidationError = "Автоматический цикл панелей не завершён: $($uiCycles.completed)/$UiCycleCount."
                }
            }
        }
        catch {
            Write-Warning "Не удалось добавить диагностику кадров в сводку: $($_.Exception.Message)"
        }
        finally {
            Remove-Item -LiteralPath $playbackDiagnosticsPath -ErrorAction SilentlyContinue
        }
    }
    else {
        Write-Warning 'Oxide не создал диагностический отчёт mpv; основная сводка сохранена без него.'
    }
}
if ($null -ne $renderGate) {
    $renderGate.Dispose()
}
if ($null -ne $decoderValidationError) {
    throw "$decoderValidationError Отчёт сохранён для диагностики, но не является допустимым результатом A/B."
}
if ($null -ne $uiCycleValidationError) {
    throw "$uiCycleValidationError Отчёт сохранён для диагностики, но не является завершённым циклическим замером."
}
