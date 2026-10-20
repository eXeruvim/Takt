[CmdletBinding()]
param(
    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\takt.exe'),

    [string]$MediaPath = '',

    [ValidateRange(1, 30)]
    [int]$StartupTimeoutSeconds = 10,

    [ValidateRange(1, 30)]
    [int]$VisibleSeconds = 3,

    [ValidateRange(1, 30)]
    [int]$ShutdownTimeoutSeconds = 10,

    [ValidateSet('None', 'MpvInit', 'RendererInit', 'DialogOpen')]
    [string]$Fault = 'None',

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$executablePath = [System.IO.Path]::GetFullPath($Executable)
$expectedExecutable = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'target\profiling\takt.exe')
)
$resolvedMediaPath = $null

if (-not [string]::IsNullOrWhiteSpace($MediaPath)) {
    $resolvedMediaPath = [System.IO.Path]::GetFullPath($MediaPath)
    if (-not (Test-Path -LiteralPath $resolvedMediaPath -PathType Leaf)) {
        throw "Медиафайл для smoke-теста не найден: $resolvedMediaPath"
    }
}

if ($Fault -eq 'RendererInit' -and $null -eq $resolvedMediaPath) {
    throw 'Для проверки RendererInit укажите видеофайл через -MediaPath.'
}

if (-not (Test-Path -LiteralPath $executablePath -PathType Leaf)) {
    throw "Сборка Takt не найдена: $executablePath"
}

$runtimePath = Join-Path (Split-Path -Parent $executablePath) 'libmpv-2.dll'
$runtimeSource = Join-Path $repository 'mpv-2.dll'
if (-not (Test-Path -LiteralPath $runtimePath -PathType Leaf) -and
    -not (Test-Path -LiteralPath $runtimeSource -PathType Leaf)) {
    throw 'Не найдена libmpv-2.dll рядом со сборкой или mpv-2.dll в корне проекта.'
}

if ($ValidateOnly) {
    if ($null -ne $resolvedMediaPath) {
        Write-Host "Пути, медиафайл и native runtime smoke-теста: OK"
    }
    else {
        Write-Host 'Пути, параметры и native runtime smoke-теста: OK'
    }
    return
}

if ($executablePath -eq $expectedExecutable) {
    $trackedChanges = (& git -C $repository status --porcelain --untracked-files=no 2>&1 |
        Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось проверить состояние Git: $trackedChanges"
    }
    if (-not [string]::IsNullOrWhiteSpace($trackedChanges)) {
        throw 'Для предрелизного smoke-теста нужны закоммиченные отслеживаемые файлы.'
    }

    $gitCommit = (& git -C $repository rev-parse HEAD 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось определить commit: $gitCommit"
    }
    $buildOutput = (& $executablePath --build-info 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Не удалось прочитать build-info: $buildOutput"
    }
    $build = $buildOutput | ConvertFrom-Json -ErrorAction Stop
    if ([string]$build.git_commit -ne $gitCommit -or
        [string]$build.tracked_state -ne 'true') {
        throw 'Profiling-сборка не соответствует текущему чистому commit. Пересоберите её.'
    }
}
else {
    Write-Warning "Проверяется нестандартная сборка: $executablePath"
}

if (-not (Test-Path -LiteralPath $runtimePath -PathType Leaf)) {
    [System.IO.File]::Copy($runtimeSource, $runtimePath, $false)
}

$process = $null
$standardErrorTask = $null
$startedAt = [Diagnostics.Stopwatch]::StartNew()
try {
    $startInfo = [Diagnostics.ProcessStartInfo]::new()
    $startInfo.FileName = $executablePath
    $startInfo.WorkingDirectory = $repository
    $startInfo.UseShellExecute = $false
    $startInfo.RedirectStandardError = $true
    # A smoke launch must not inherit stale handles or report paths from an
    # earlier profiling session in the caller's terminal.
    [void]$startInfo.Environment.Remove('TAKT_BENCHMARK_DIAGNOSTICS_PATH')
    [void]$startInfo.Environment.Remove('TAKT_BENCHMARK_RENDER_GATE')
    [void]$startInfo.Environment.Remove('TAKT_BENCHMARK_HWDEC')
    [void]$startInfo.Environment.Remove('TAKT_BENCHMARK_FAULT')
    $expectedFaultMarker = $null
    $expectedAdditionalMarker = $null
    if ($Fault -eq 'MpvInit') {
        $startInfo.Environment['TAKT_BENCHMARK_FAULT'] = 'mpv-init'
        $expectedFaultMarker = 'TAKT_DIAGNOSTIC_FAULT:mpv-init'
    }
    elseif ($Fault -eq 'RendererInit') {
        $startInfo.Environment['TAKT_BENCHMARK_FAULT'] = 'renderer-init'
        $expectedFaultMarker = 'TAKT_DIAGNOSTIC_FAULT:renderer-init'
        $expectedAdditionalMarker = 'TAKT_DIAGNOSTIC_STATE:renderer-error'
    }
    elseif ($Fault -eq 'DialogOpen') {
        $startInfo.Environment['TAKT_BENCHMARK_FAULT'] = 'dialog-open'
        $expectedFaultMarker = 'TAKT_DIAGNOSTIC_FAULT:dialog-open'
        $expectedAdditionalMarker = 'TAKT_DIAGNOSTIC_FAULT:dialog-shutdown'
    }
    if ($null -ne $resolvedMediaPath) {
        [void]$startInfo.ArgumentList.Add($resolvedMediaPath)
    }
    $process = [Diagnostics.Process]::Start($startInfo)
    if ($null -eq $process) {
        throw 'Не удалось запустить процесс Takt.'
    }
    $standardErrorTask = $process.StandardError.ReadToEndAsync()

    $startupDeadline = [TimeSpan]::FromSeconds($StartupTimeoutSeconds)
    while ($startedAt.Elapsed -lt $startupDeadline) {
        $process.Refresh()
        if ($process.HasExited) {
            throw "Takt завершился до появления окна с кодом $($process.ExitCode)."
        }
        if ($process.MainWindowHandle -ne [IntPtr]::Zero) {
            break
        }
        Start-Sleep -Milliseconds 100
    }

    $process.Refresh()
    if ($process.MainWindowHandle -eq [IntPtr]::Zero) {
        throw "Главное окно Takt не появилось за $StartupTimeoutSeconds с. Процесс оставлен пользователю."
    }

    $startupMilliseconds = [math]::Round($startedAt.Elapsed.TotalMilliseconds, 1)
    Write-Host "Окно Takt PID $($process.Id) появилось за $startupMilliseconds мс."
    if ($null -ne $resolvedMediaPath) {
        Write-Host "Открыт файл: $([System.IO.Path]::GetFileName($resolvedMediaPath))"
    }
    Start-Sleep -Seconds $VisibleSeconds

    $process.Refresh()
    if ($Fault -eq 'DialogOpen') {
        if (-not $process.HasExited -and
            -not $process.WaitForExit($ShutdownTimeoutSeconds * 1000)) {
            throw "Takt PID $($process.Id) не закрылся при открытом системном диалоге за $ShutdownTimeoutSeconds с. Процесс оставлен пользователю."
        }
    }
    elseif ($process.HasExited) {
        throw "Takt неожиданно завершился с кодом $($process.ExitCode)."
    }
    else {
        if (-not $process.CloseMainWindow()) {
            throw "Takt PID $($process.Id) не принял обычный запрос закрытия. Процесс оставлен пользователю."
        }
        if (-not $process.WaitForExit($ShutdownTimeoutSeconds * 1000)) {
            throw "Takt PID $($process.Id) не закрылся за $ShutdownTimeoutSeconds с. Процесс оставлен пользователю."
        }
    }
    if ($process.ExitCode -ne 0) {
        throw "Takt завершился с кодом $($process.ExitCode)."
    }

    $standardError = $standardErrorTask.GetAwaiter().GetResult()
    if ($null -ne $expectedFaultMarker -and
        -not $standardError.Contains($expectedFaultMarker, [StringComparison]::Ordinal)) {
        throw "Диагностический сценарий $Fault не сработал. Проверьте, что сборка создана с --features benchmark-tools."
    }
    if ($null -ne $expectedAdditionalMarker -and
        -not $standardError.Contains($expectedAdditionalMarker, [StringComparison]::Ordinal)) {
        throw "Диагностический сценарий $Fault не достиг ожидаемого состояния интерфейса."
    }

    if ($null -ne $expectedFaultMarker) {
        Write-Host "Диагностический сценарий $Fault пройден: окно осталось отзывчивым и штатно закрылось."
    }
    elseif ($null -ne $resolvedMediaPath) {
        Write-Host 'Smoke-тест медиа пройден: окно оставалось отзывчивым и штатно закрылось.'
    }
    else {
        Write-Host 'Smoke-тест пройден: окно появилось, оставалось отзывчивым и штатно закрылось.'
    }
}
catch {
    if ($null -ne $process) {
        $process.Refresh()
        if (-not $process.HasExited) {
            Write-Warning "Takt PID $($process.Id) не завершается принудительно; закройте окно вручную."
        }
    }
    throw
}
