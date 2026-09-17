[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k')]
    [string]$Scenario,

    [ValidateSet('CPU', 'GPU', 'Power', 'GeneralProfile', 'Heap', 'Video')]
    [string]$Profile = 'CPU',

    [ValidateRange(5, 300)]
    [int]$DurationSeconds = 30,

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 5,

    [string]$Executable = (Join-Path $PSScriptRoot '..\target\profiling\oxide.exe'),

    [string]$MediaPath = '',

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
try {
    $process = Start-Process `
        -FilePath $executablePath `
        -WorkingDirectory $repository `
        -WindowStyle Normal `
        -PassThru
    Write-Host "Oxide запущен с PID $($process.Id)."
    if ($Scenario -ne 'idle') {
        Read-Host 'Настройте сценарий, дождитесь стабилизации и нажмите Enter'
    }
    if ($WarmupSeconds -gt 0) {
        Write-Host "Прогрев: $WarmupSeconds с."
        Start-Sleep -Seconds $WarmupSeconds
    }
    $process.Refresh()
    if ($process.HasExited) {
        throw "Oxide завершился до начала трассы с кодом $($process.ExitCode)"
    }

    Write-Warning 'ETL содержит системные события, имена процессов и может содержать локальные пути. Не публикуйте трассу без проверки.'
    $startOutput = Invoke-CheckedWpr -Arguments @('-start', $Profile, '-filemode')
    $recordingStarted = $true
    $startedAt = [DateTimeOffset]::UtcNow
    Write-Host "WPR $Profile записывает $DurationSeconds с. Не взаимодействуйте с компьютером."
    Start-Sleep -Seconds $DurationSeconds
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
        finished_utc = $finishedAt.ToString('O')
        requested_duration_seconds = $DurationSeconds
        git_commit = $gitCommit
        package_version = [string]$build.package_version
        executable_sha256 = (Get-FileHash -LiteralPath $executablePath -Algorithm SHA256).Hash
        mpv_sha256 = (Get-FileHash -LiteralPath $runtimePath -Algorithm SHA256).Hash
        media = $media
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
}
