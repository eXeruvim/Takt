[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'hdd-video', 'paused', 'minimized')]
    [string]$Scenario,

    [ValidateSet(
        'CPU', 'GPU', 'Power', 'GeneralProfile', 'Heap', 'Video',
        'VirtualAllocation', 'ResidentSet', 'ReferenceSet'
    )]
    [string]$Profile = 'CPU',

    [ValidateRange(5, 300)]
    [int]$DurationSeconds = 30,

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 5,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

    [string]$MediaPath = '',

    [ValidateSet('not-recorded', 'software', 'hardware-auto-copy', 'nvdec', 'nvdec-copy', 'd3d11va-copy')]
    [string]$DecoderMode = 'not-recorded',

    [ValidateSet('not-recorded', 'integrated-opengl', 'native-hwnd-fallback')]
    [string]$RendererBackend = 'not-recorded',

    [ValidateSet('windowed', 'maximized', 'fullscreen', 'picture-in-picture')]
    [string]$WindowMode = 'windowed',

    [ValidateSet('system', 'dark', 'light')]
    [string]$Theme = 'system',

    [ValidateLength(0, 1000)]
    [string]$Notes = '',

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Invoke-CheckedWpr {
    param([Parameter(Mandatory = $true)][string[]]$Arguments)

    $output = (& wpr.exe @Arguments 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        if ($output -match '(?i)0xc5585011|profile system performance') {
            throw 'Windows не разрешила системное профилирование. Запустите PowerShell от имени администратора либо выдайте учётной записи право «Profile system performance». WPR-сессия не начата.'
        }
        throw "WPR завершился с кодом $LASTEXITCODE`: $output"
    }
    return $output
}

function Close-TraceProcess {
    param([Parameter(Mandatory = $true)][System.Diagnostics.Process]$Process)

    $Process.Refresh()
    if ($Process.HasExited) {
        return
    }
    if (-not $Process.CloseMainWindow()) {
        throw "Oxide PID $($Process.Id) не принял обычный запрос закрытия. Процесс оставлен пользователю."
    }
    if (-not $Process.WaitForExit(10000)) {
        throw "Oxide PID $($Process.Id) не закрылся за 10 секунд. Процесс оставлен пользователю."
    }
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$executablePath = [System.IO.Path]::GetFullPath($Executable)
$expectedExecutable = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'target\profiling\oxide.exe')
)
$resultRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'benchmark-results\wpr'))
if (-not $resultRoot.StartsWith(
    $repository + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Каталог WPR-результатов оказался вне репозитория: $resultRoot"
}
$videoScenario = $Scenario -in @('video-1080p', 'video-4k', 'hdd-video')
if ($videoScenario -and ($DecoderMode -eq 'not-recorded' -or $RendererBackend -eq 'not-recorded')) {
    throw 'Для video-сценария явно укажите -DecoderMode и -RendererBackend.'
}

$wpr = Get-Command wpr.exe -ErrorAction Stop
$profiles = (& $wpr.Source -profiles 2>&1 | Out-String)
if ($LASTEXITCODE -ne 0 -or $profiles -notmatch "(?m)^\s*$([Regex]::Escape($Profile))\s") {
    throw "Профиль WPR '$Profile' недоступен на этой системе."
}

if ($ValidateOnly) {
    Write-Host "WPR, профиль $Profile и безопасный каталог результатов: OK"
    return
}

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
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось определить commit: $gitCommit"
}
$trackedChanges = (& git -C $repository status --porcelain --untracked-files=no 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось проверить рабочее дерево: $trackedChanges"
}
if (-not [string]::IsNullOrWhiteSpace($trackedChanges)) {
    throw 'Для WPR-трассы нужны закоммиченные отслеживаемые файлы.'
}

$buildOutput = (& $executablePath --build-info 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось прочитать build-info: $buildOutput"
}
$build = $buildOutput | ConvertFrom-Json -ErrorAction Stop
if ([string]$build.git_commit -ne $gitCommit -or [string]$build.tracked_state -ne 'true') {
    throw 'Profiling-сборка не соответствует текущему чистому commit. Пересоберите её.'
}

$media = $null
if (-not [string]::IsNullOrWhiteSpace($MediaPath)) {
    $resolvedMedia = [System.IO.Path]::GetFullPath($MediaPath)
    if (-not (Test-Path -LiteralPath $resolvedMedia -PathType Leaf)) {
        throw "Тестовый медиафайл не найден: $resolvedMedia"
    }
    $mediaItem = Get-Item -LiteralPath $resolvedMedia -ErrorAction Stop
    $media = [ordered]@{
        extension = $mediaItem.Extension.ToLowerInvariant()
        size_bytes = $mediaItem.Length
        sha256 = (Get-FileHash -LiteralPath $resolvedMedia -Algorithm SHA256).Hash
    }
}

$status = (& $wpr.Source -status 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось проверить состояние WPR: $status"
}
if ($status -notmatch '(?i)not recording') {
    throw 'WPR уже записывает другую трассу. Oxide не будет вмешиваться в эту сессию.'
}

New-Item -ItemType Directory -Path $resultRoot -Force | Out-Null
$timestamp = [DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss')
$baseName = "$timestamp-$Scenario-$($Profile.ToLowerInvariant())-$($gitCommit.Substring(0, 8))"
$tracePath = Join-Path $resultRoot "$baseName.etl"
$metadataPath = Join-Path $resultRoot "$baseName.json"
if ((Test-Path -LiteralPath $tracePath) -or (Test-Path -LiteralPath $metadataPath)) {
    throw "Файл трассы уже существует: $baseName"
}

$process = $null
$recordingStarted = $false
$recordingStopped = $false
$heapTracingChanged = $false
$startOutput = ''
$captureBeforeLaunch = $Profile -in @('Heap', 'ReferenceSet', 'VirtualAllocation')
$startedAt = $null
$scenarioStartedAt = $null
$stopRequestedAt = $null
try {
    if ($Profile -eq 'Heap') {
        $heapTracingStatus = Invoke-CheckedWpr -Arguments @(
            '-HeapTracingConfig',
            'oxide.exe'
        )
        if ($heapTracingStatus -notmatch '(?i)heap tracing is enabled') {
            [void](Invoke-CheckedWpr -Arguments @(
                '-HeapTracingConfig',
                'oxide.exe',
                'enable'
            ))
            $heapTracingChanged = $true
            Write-Host 'Heap tracing для oxide.exe временно включён.'
        }
    }
    Write-Warning 'ETL содержит системные события, имена процессов и может содержать локальные пути. Не публикуйте трассу без проверки.'
    if ($captureBeforeLaunch) {
        $startOutput = Invoke-CheckedWpr -Arguments @('-start', $Profile, '-filemode')
        $recordingStarted = $true
        $startedAt = [DateTimeOffset]::UtcNow
        Write-Host "WPR $Profile запущен до Oxide, чтобы увидеть стартовые выделения."
    }

    $hardwareDecoderVariable = 'OXIDE_BENCHMARK_HWDEC'
    $oldHardwareDecoder = [Environment]::GetEnvironmentVariable(
        $hardwareDecoderVariable,
        [EnvironmentVariableTarget]::Process
    )
    try {
        [Environment]::SetEnvironmentVariable(
            $hardwareDecoderVariable,
            $(if ($DecoderMode -eq 'nvdec') { 'nvdec' } else { $null }),
            [EnvironmentVariableTarget]::Process
        )
        $process = Start-Process `
            -FilePath $executablePath `
            -WorkingDirectory $repository `
            -WindowStyle Normal `
            -PassThru
    }
    finally {
        [Environment]::SetEnvironmentVariable(
            $hardwareDecoderVariable,
            $oldHardwareDecoder,
            [EnvironmentVariableTarget]::Process
        )
    }
    Write-Host "Oxide запущен с PID $($process.Id)."
    if ($Scenario -ne 'idle') {
        $scenarioInstruction = switch ($Scenario) {
            'audio' { 'Запустите аудиофайл и дождитесь стабильного воспроизведения.' }
            'video-1080p' { 'Запустите видео 1080p и дождитесь стабильного воспроизведения.' }
            'video-4k' { 'Запустите видео 4K и дождитесь стабильного воспроизведения.' }
            'hdd-video' { 'Откройте видео именно с HDD. Во время записи: пауза на 5 секунд, продолжение, через 10 секунд одна перемотка и затем обычное воспроизведение.' }
            'paused' { 'Запустите медиафайл, поставьте воспроизведение на паузу и не двигайте мышь.' }
            'minimized' { 'Запустите медиафайл и сверните окно Oxide на панель задач.' }
        }
        Write-Host $scenarioInstruction
        Read-Host 'Когда сценарий готов, вернитесь в терминал и нажмите Enter'
    }
    if ($WarmupSeconds -gt 0) {
        Write-Host "Прогрев: $WarmupSeconds с."
        Start-Sleep -Seconds $WarmupSeconds
    }
    $process.Refresh()
    if ($process.HasExited) {
        throw "Oxide завершился до начала трассы с кодом $($process.ExitCode)"
    }

    if (-not $captureBeforeLaunch) {
        $startOutput = Invoke-CheckedWpr -Arguments @('-start', $Profile, '-filemode')
        $recordingStarted = $true
        $startedAt = [DateTimeOffset]::UtcNow
    }
    $scenarioStartedAt = [DateTimeOffset]::UtcNow
    if ($Scenario -eq 'hdd-video') {
        Write-Host "WPR $Profile записывает $DurationSeconds с. Выполните указанные действия в Oxide."
    }
    else {
        Write-Host "WPR $Profile записывает $DurationSeconds с. Не взаимодействуйте с компьютером."
    }
    Start-Sleep -Seconds $DurationSeconds
    $stopRequestedAt = [DateTimeOffset]::UtcNow
    $stopOutput = Invoke-CheckedWpr -Arguments @(
        '-stop',
        $tracePath,
        "Oxide $Scenario $Profile"
    )
    $recordingStopped = $true
    $finishedAt = [DateTimeOffset]::UtcNow

    $metadata = [ordered]@{
        schema = 1
        kind = 'oxide-wpr-trace'
        scenario = $Scenario
        profile = $Profile
        started_utc = $startedAt.ToString('O')
        scenario_started_utc = $scenarioStartedAt.ToString('O')
        trace_stop_requested_utc = $stopRequestedAt.ToString('O')
        finished_utc = $finishedAt.ToString('O')
        requested_duration_seconds = $DurationSeconds
        actual_trace_duration_seconds = [math]::Round(
            ($stopRequestedAt - $startedAt).TotalSeconds,
            3
        )
        export_flush_seconds = [math]::Round(
            ($finishedAt - $stopRequestedAt).TotalSeconds,
            3
        )
        capture_started_before_process = $captureBeforeLaunch
        git_commit = $gitCommit
        package_version = [string]$build.package_version
        executable_sha256 = (Get-FileHash -LiteralPath $executablePath -Algorithm SHA256).Hash
        mpv_sha256 = (Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash
        media = $media
        decoder_mode = $DecoderMode
        renderer_backend = $RendererBackend
        window_mode = $WindowMode
        theme = $Theme
        notes = $Notes
        process_id = $process.Id
        wpr_profile = $Profile
        trace_file = Split-Path -Leaf $tracePath
        trace_size_bytes = (Get-Item -LiteralPath $tracePath -ErrorAction Stop).Length
        system_wide = $true
        contains_sensitive_system_data = $true
        analysis_status = 'WPA analysis pending'
    }
    $metadata | ConvertTo-Json -Depth 6 | Set-Content -LiteralPath $metadataPath -Encoding utf8
    Write-Host "Трасса: $tracePath"
    Write-Host "Паспорт: $metadataPath"
    Write-Verbose $startOutput
    Write-Verbose $stopOutput
}
finally {
    if ($recordingStarted -and -not $recordingStopped) {
        try {
            $cancelOutput = Invoke-CheckedWpr -Arguments @('-cancel')
            Write-Warning "Незавершённая WPR-сессия отменена. $cancelOutput"
        }
        catch {
            Write-Warning "Не удалось отменить WPR-сессию: $($_.Exception.Message)"
        }
    }
    if ($null -ne $process) {
        try {
            Close-TraceProcess -Process $process
        }
        catch {
            Write-Warning $_.Exception.Message
        }
    }
    if ($heapTracingChanged) {
        try {
            [void](Invoke-CheckedWpr -Arguments @(
                '-HeapTracingConfig',
                'oxide.exe',
                'disable'
            ))
            Write-Host 'Временная конфигурация heap tracing отключена.'
        }
        catch {
            Write-Warning (
                'Не удалось отключить heap tracing для oxide.exe: ' +
                $_.Exception.Message
            )
        }
    }
}
