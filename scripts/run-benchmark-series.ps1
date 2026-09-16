[CmdletBinding()]
param(
    [Parameter(Mandatory = $true)]
    [ValidateSet('idle', 'audio', 'video-1080p', 'video-4k', 'large-playlist', 'large-artwork')]
    [string]$Scenario,

    [ValidateRange(2, 30)]
    [int]$Runs = 5,

    [ValidateRange(10, 3600)]
    [int]$DurationSeconds = 120,

    [ValidateRange(100, 5000)]
    [int]$SampleIntervalMs = 250,

    [ValidateRange(0, 300)]
    [int]$WarmupSeconds = 5,

    [ValidateRange(0, 60)]
    [int]$CooldownSeconds = 3,

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

    [switch]$AllowDirty,

    [switch]$ValidateOnly
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$measureScript = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'measure-oxide.ps1'))
$summaryScript = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot 'summarize-benchmark-results.ps1'))
foreach ($script in @($measureScript, $summaryScript)) {
    if (-not (Test-Path -LiteralPath $script -PathType Leaf)) {
        throw "Не найден обязательный скрипт: $script"
    }
}

$measureArguments = @{
    Scenario = $Scenario
    DurationSeconds = $DurationSeconds
    SampleIntervalMs = $SampleIntervalMs
    Executable = $Executable
    DecoderMode = $DecoderMode
    RendererBackend = $RendererBackend
    WindowMode = $WindowMode
    Theme = $Theme
    MediaPath = $MediaPath
    WorkloadManifest = $WorkloadManifest
    Notes = $Notes
    WarmupSeconds = $WarmupSeconds
}

if ($ValidateOnly) {
    & $measureScript @measureArguments -ValidateOnly
    & $summaryScript -Scenario $Scenario -ValidateOnly
    Write-Host 'Синтаксис и зависимости серийного замера: OK'
    return
}

$trackedChanges = (& git -C $repository status --porcelain --untracked-files=no 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0) {
    throw "Не удалось проверить состояние Git: $trackedChanges"
}
if (-not [string]::IsNullOrWhiteSpace($trackedChanges) -and -not $AllowDirty) {
    throw 'Для сопоставимой серии нужны закоммиченные отслеживаемые файлы. Завершите commit либо используйте -AllowDirty только для чернового прогона.'
}
if ($AllowDirty) {
    $measureArguments.AllowUnverifiedBuild = $true
}

$automaticIdle = $Scenario -eq 'idle'
Write-Host "Серия ${Scenario}: $Runs прогонов по $DurationSeconds с, прогрев $WarmupSeconds с."
if ($automaticIdle) {
    Write-Host 'Idle-сценарий не требует действий: не перемещайте мышь над окнами Oxide до завершения серии.'
}
else {
    Write-Host 'В каждом прогоне настройте сценарий в новом окне Oxide и подтвердите готовность в терминале.'
}

for ($run = 1; $run -le $Runs; $run++) {
    Write-Host ''
    Write-Host "Прогон $run из $Runs"
    & $measureScript @measureArguments -AutomaticIdle:$automaticIdle -CloseAfterMeasurement

    if ($run -lt $Runs -and $CooldownSeconds -gt 0) {
        Write-Host "Пауза между прогонами: $CooldownSeconds с."
        Start-Sleep -Seconds $CooldownSeconds
    }
}

$summaryArguments = @{
    Scenario = $Scenario
    MinimumRuns = $Runs
}
if ($AllowDirty) {
    $summaryArguments.IncludeDirty = $true
}

Write-Host ''
Write-Host 'Агрегация сопоставимых результатов...'
& $summaryScript @summaryArguments
