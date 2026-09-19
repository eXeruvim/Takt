[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'paused', 'minimized')]
    [string]$Scenario,

    [ValidateRange(10, 300)]
    [int]$DurationSeconds = 30,

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 5,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

    [switch]$AutomaticIdle,

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Get-Percentile {
    param(
        [Parameter(Mandatory = $true)][double[]]$Values,
        [Parameter(Mandatory = $true)][ValidateRange(0.0, 1.0)][double]$Percentile
    )

    if ($Values.Count -eq 0) { return $null }
    $ordered = @($Values | Sort-Object)
    $index = [Math]::Max(0, [Math]::Ceiling($Percentile * $ordered.Count) - 1)
    return $ordered[$index]
}

function Get-Stats {
    param([Parameter(Mandatory = $true)][double[]]$Values)

    if ($Values.Count -eq 0) { return $null }
    return [ordered]@{
        min = [Math]::Round(($Values | Measure-Object -Minimum).Minimum, 3)
        p50 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.50), 3)
        p95 = [Math]::Round((Get-Percentile -Values $Values -Percentile 0.95), 3)
        max = [Math]::Round(($Values | Measure-Object -Maximum).Maximum, 3)
        mean = [Math]::Round(($Values | Measure-Object -Average).Average, 3)
    }
}

function Get-FrameSamples {
    param(
        [Parameter(Mandatory = $true)][string[]]$Lines,
        [Parameter(Mandatory = $true)][int]$Limit
    )

    $samples = @($Lines | ForEach-Object {
        if ($_ -match 'average frames per second:\s+(\d+)') {
            [int]$Matches[1]
        }
    })
    if ($samples.Count -gt $Limit) {
        return @($samples | Select-Object -Last $Limit)
    }
    return $samples
}

function Close-OxideProcess {
    param(
        [Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process,
        [switch]$RestoreFirst
    )

    $Process.Refresh()
    if ($Process.HasExited) { return 'already-exited' }
    if ($RestoreFirst -and $Process.MainWindowHandle -ne [IntPtr]::Zero) {
        if (-not ('OxideRefreshWindow' -as [type])) {
            Add-Type @'
using System;
using System.Runtime.InteropServices;

public static class OxideRefreshWindow {
    [DllImport("user32.dll")]
    [return: MarshalAs(UnmanagedType.Bool)]
    public static extern bool ShowWindowAsync(IntPtr window, int command);
}
'@
        }
        [void][OxideRefreshWindow]::ShowWindowAsync($Process.MainWindowHandle, 9)
        Start-Sleep -Milliseconds 300
        $Process.Refresh()
    }
    if (-not $Process.CloseMainWindow()) {
        Write-Warning "Oxide PID $($Process.Id) не принял обычный запрос закрытия. Процесс оставлен пользователю."
        return 'request-rejected'
    }
    if (-not $Process.WaitForExit(10000)) {
        Write-Warning "Oxide PID $($Process.Id) не закрылся за 10 секунд. Процесс оставлен пользователю."
        return 'timeout'
    }
    return 'closed'
}

function Get-SharedLogLines {
    param([Parameter(Mandatory = $true)][string]$Path)

    if (-not (Test-Path -LiteralPath $Path -PathType Leaf)) { return @() }
    $stream = [System.IO.File]::Open(
        $Path,
        [System.IO.FileMode]::Open,
        [System.IO.FileAccess]::Read,
        [System.IO.FileShare]::ReadWrite
    )
    try {
        $reader = [System.IO.StreamReader]::new($stream, [System.Text.Encoding]::UTF8, $true)
        try {
            $text = $reader.ReadToEnd()
        }
        finally {
            $reader.Dispose()
        }
    }
    finally {
        $stream.Dispose()
    }
    if ([string]::IsNullOrEmpty($text)) { return @() }
    return @($text -split "`r?`n")
}

if ($ValidateOnly) {
    $probe = Get-FrameSamples -Lines @(
        'average frames per second: 0',
        'unrelated line',
        'average frames per second: 30',
        'average frames per second: 60'
    ) -Limit 2
    $stats = Get-Stats -Values @($probe | ForEach-Object { [double]$_ })
    if ($probe.Count -ne 2 -or $probe[0] -ne 30 -or $probe[1] -ne 60 `
        -or $stats.p50 -ne 30 -or $stats.p95 -ne 60) {
        throw 'Самопроверка разбора refresh_lazy завершилась ошибкой'
    }
    Write-Host 'Синтаксис и разбор Slint refresh_lazy: OK'
    return
}

if ($AutomaticIdle -and $Scenario -ne 'idle') {
    throw '-AutomaticIdle разрешён только для сценария idle.'
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$executablePath = [System.IO.Path]::GetFullPath($Executable)
$expectedExecutable = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'target\profiling\oxide.exe')
)
if ($executablePath -ne $expectedExecutable) {
    Write-Warning "Используется нестандартный executable: $executablePath"
}
if (-not (Test-Path -LiteralPath $executablePath -PathType Leaf)) {
    throw 'Profiling-сборка не найдена. Выполните: cargo build --profile profiling --locked --offline'
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
}

$gitCommit = (& git -C $repository rev-parse HEAD 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw "Не удалось определить commit: $gitCommit" }
$trackedChanges = (& git -C $repository status --porcelain --untracked-files=no 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw "Не удалось проверить рабочее дерево: $trackedChanges" }
if (-not [string]::IsNullOrWhiteSpace($trackedChanges)) {
    throw 'Для refresh_lazy-замера нужны закоммиченные отслеживаемые файлы.'
}

$buildOutput = (& $executablePath --build-info 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) { throw "Не удалось прочитать build-info: $buildOutput" }
$build = $buildOutput | ConvertFrom-Json -ErrorAction Stop
if ([string]$build.git_commit -ne $gitCommit -or [string]$build.tracked_state -ne 'true') {
    throw 'Profiling-сборка не соответствует текущему чистому commit. Пересоберите её.'
}

$resultRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'benchmark-results\slint-refresh')
)
if (-not $resultRoot.StartsWith(
    $repository + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Каталог результатов оказался вне репозитория: $resultRoot"
}
New-Item -ItemType Directory -Path $resultRoot -Force | Out-Null

$timestamp = [DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss')
$baseName = "$timestamp-$Scenario"
$stdoutPath = Join-Path $resultRoot "$baseName.stdout.log"
$stderrPath = Join-Path $resultRoot "$baseName.stderr.log"
$summaryPath = Join-Path $resultRoot "$baseName.json"

$instructions = switch ($Scenario) {
    'idle' { 'Не открывайте медиа и не двигайте мышью над окном.' }
    'audio' { 'Откройте аудиофайл, запустите воспроизведение и дождитесь появления обложки.' }
    'video-1080p' { 'Откройте 1080p-видео и дождитесь стабильного воспроизведения.' }
    'video-4k' { 'Откройте 4K-видео и дождитесь стабильного воспроизведения.' }
    'paused' { 'Откройте медиафайл, поставьте его на паузу и не двигайте мышью.' }
    'minimized' { 'Откройте медиафайл, запустите его и сверните Oxide на панель задач.' }
}

$oldPerformanceSetting = [Environment]::GetEnvironmentVariable(
    'SLINT_DEBUG_PERFORMANCE',
    [EnvironmentVariableTarget]::Process
)
try {
    [Environment]::SetEnvironmentVariable(
        'SLINT_DEBUG_PERFORMANCE',
        'refresh_lazy,console',
        [EnvironmentVariableTarget]::Process
    )
    $process = Start-Process `
        -FilePath $executablePath `
        -WorkingDirectory $repository `
        -WindowStyle Normal `
        -RedirectStandardOutput $stdoutPath `
        -RedirectStandardError $stderrPath `
        -PassThru
}
finally {
    [Environment]::SetEnvironmentVariable(
        'SLINT_DEBUG_PERFORMANCE',
        $oldPerformanceSetting,
        [EnvironmentVariableTarget]::Process
    )
}

Write-Host "Oxide запущен с PID $($process.Id)."
Write-Host $instructions
if (-not $AutomaticIdle) {
    Read-Host 'Настройте сценарий, дождитесь стабилизации и нажмите Enter'
}
if ($WarmupSeconds -gt 0) {
    Write-Host "Прогрев: $WarmupSeconds с."
    Start-Sleep -Seconds $WarmupSeconds
}

$startedAt = [DateTimeOffset]::UtcNow
Write-Host "Запись перерисовок: $DurationSeconds с. Не взаимодействуйте с Oxide."
for ($elapsed = 0; $elapsed -lt $DurationSeconds; $elapsed++) {
    Start-Sleep -Seconds 1
    $process.Refresh()
    if ($process.HasExited) {
        Write-Warning 'Oxide завершился до конца замера; сохранены доступные образцы.'
        break
    }
    Write-Progress `
        -Activity "Slint refresh_lazy: $Scenario" `
        -Status "$($elapsed + 1) из $DurationSeconds секунд" `
        -PercentComplete ((($elapsed + 1) / $DurationSeconds) * 100)
}
Write-Progress -Activity "Slint refresh_lazy: $Scenario" -Completed
$finishedAt = [DateTimeOffset]::UtcNow

$lines = @(Get-SharedLogLines -Path $stdoutPath)
$lines += @(Get-SharedLogLines -Path $stderrPath)
$samples = @(Get-FrameSamples -Lines $lines -Limit $DurationSeconds)
$closeResult = Close-OxideProcess `
    -Process $process `
    -RestoreFirst:($Scenario -eq 'minimized')
if ($samples.Count -eq 0) {
    throw "Slint не записал refresh_lazy-образцы. Проверьте журнал: $stderrPath"
}

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-slint-refresh-lazy'
    scenario = $Scenario
    started_utc = $startedAt.ToString('O')
    finished_utc = $finishedAt.ToString('O')
    requested_duration_seconds = $DurationSeconds
    samples = $samples.Count
    close_result = $closeResult
    frames_per_second = Get-Stats -Values @($samples | ForEach-Object { [double]$_ })
    seconds_with_redraw = @($samples | Where-Object { $_ -gt 0 }).Count
    seconds_without_redraw = @($samples | Where-Object { $_ -eq 0 }).Count
    raw_samples = $samples
    environment = [ordered]@{
        executable = $executablePath
        executable_sha256 = (Get-FileHash -LiteralPath $executablePath -Algorithm SHA256).Hash
        git_commit = $gitCommit
        build_provenance = $build
        slint_debug_performance = 'refresh_lazy,console'
        stdout_log = $stdoutPath
        stderr_log = $stderrPath
    }
}
$summary | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $summaryPath -Encoding utf8

Write-Host "Готово: $summaryPath"
Write-Host ("FPS p50/p95: {0}/{1}; секунд с перерисовкой: {2} из {3}." -f `
    $summary.frames_per_second.p50, $summary.frames_per_second.p95, `
    $summary.seconds_with_redraw, $summary.samples)
