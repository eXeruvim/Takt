[CmdletBinding()]
param(
    [string]$Helper = "target/release/oxide-updater.exe",
    [string]$OutputDirectory = "artifacts/updater-test",
    [switch]$Keep
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'artifacts'))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))
$helperPath = [System.IO.Path]::GetFullPath((Join-Path $repository $Helper))
if (-not $output.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Updater test должен работать только внутри $artifactRoot`: $output"
}
if (-not (Test-Path -LiteralPath $helperPath -PathType Leaf)) {
    throw "Updater helper не найден: $helperPath"
}
if (Test-Path -LiteralPath $output) {
    $item = Get-Item -LiteralPath $output -Force
    if (($item.Attributes -band [System.IO.FileAttributes]::ReparsePoint) -ne 0) {
        throw "Updater test path не должен быть reparse point: $output"
    }
    Remove-Item -LiteralPath $output -Recurse -Force
}
New-Item -ItemType Directory -Path $output | Out-Null

function New-FixtureArchive {
    param([string]$Name, [bool]$ValidExecutable)
    $source = Join-Path $output "$Name-source"
    $payload = Join-Path $source "Oxide-fixture"
    New-Item -ItemType Directory -Path $payload -Force | Out-Null
    if ($ValidExecutable) {
        Copy-Item -LiteralPath $helperPath -Destination (Join-Path $payload 'oxide.exe')
    }
    else {
        [System.IO.File]::WriteAllBytes((Join-Path $payload 'oxide.exe'), [byte[]](1, 2, 3, 4))
    }
    Copy-Item -LiteralPath $helperPath -Destination (Join-Path $payload 'oxide-updater.exe')
    Set-Content -LiteralPath (Join-Path $payload 'generation.txt') -Value 'new' -NoNewline
    $archive = Join-Path $output "$Name.zip"
    Add-Type -AssemblyName System.IO.Compression.FileSystem
    [System.IO.Compression.ZipFile]::CreateFromDirectory($source, $archive)
    return $archive
}

function New-OldTarget {
    param([string]$Name)
    $target = Join-Path $output $Name
    New-Item -ItemType Directory -Path $target | Out-Null
    Copy-Item -LiteralPath $helperPath -Destination (Join-Path $target 'oxide.exe')
    Copy-Item -LiteralPath $helperPath -Destination (Join-Path $target 'oxide-updater.exe')
    Set-Content -LiteralPath (Join-Path $target 'generation.txt') -Value 'old' -NoNewline
    Set-Content -LiteralPath (Join-Path $target 'user-file.txt') -Value 'preserve' -NoNewline
    return $target
}

function Start-ShortProcess {
    return Start-Process -FilePath (Get-Process -Id $PID).Path `
        -ArgumentList @('-NoProfile', '-Command', 'Start-Sleep -Milliseconds 250') `
        -WindowStyle Hidden -PassThru
}

$previousProbe = $env:OXIDE_UPDATER_TEST_PROBE
$previousTestRoot = $env:OXIDE_UPDATER_TEST_ROOT
$env:OXIDE_UPDATER_TEST_PROBE = 'enabled'
$env:OXIDE_UPDATER_TEST_ROOT = Join-Path $output 'state'
try {
    $probeRoot = Join-Path $env:OXIDE_UPDATER_TEST_ROOT 'update-probes'
    New-Item -ItemType Directory -Path $probeRoot -Force | Out-Null

    $successTarget = New-OldTarget 'success-target'
    $successArchive = New-FixtureArchive 'success' $true
    $successWait = Start-ShortProcess
    $successMarker = Join-Path $probeRoot "updater-success-$([guid]::NewGuid().ToString('N')).ready"
    & $helperPath apply portable $successArchive $successTarget $successWait.Id $successMarker
    if ($LASTEXITCODE -ne 0) { throw "Portable update fixture завершился с кодом $LASTEXITCODE" }
    if ((Get-Content -LiteralPath (Join-Path $successTarget 'generation.txt') -Raw) -ne 'new' -or
        (Get-Content -LiteralPath (Join-Path $successTarget 'user-file.txt') -Raw) -ne 'preserve') {
        throw 'Portable update не применил новую версию или потерял пользовательский файл.'
    }

    $rollbackTarget = New-OldTarget 'rollback-target'
    $rollbackArchive = New-FixtureArchive 'rollback' $false
    $rollbackWait = Start-ShortProcess
    $rollbackMarker = Join-Path $probeRoot "updater-rollback-$([guid]::NewGuid().ToString('N')).ready"
    & $helperPath apply portable $rollbackArchive $rollbackTarget $rollbackWait.Id $rollbackMarker
    if ($LASTEXITCODE -eq 0) { throw 'Повреждённая новая версия неожиданно была принята.' }
    if ((Get-Content -LiteralPath (Join-Path $rollbackTarget 'generation.txt') -Raw) -ne 'old' -or
        (Get-Content -LiteralPath (Join-Path $rollbackTarget 'user-file.txt') -Raw) -ne 'preserve') {
        throw 'Rollback не восстановил прежнюю версию без потери пользовательского файла.'
    }
    Write-Host 'Updater E2E: применение, первый запуск, сохранение файлов и rollback — OK'
}
finally {
    $env:OXIDE_UPDATER_TEST_PROBE = $previousProbe
    $env:OXIDE_UPDATER_TEST_ROOT = $previousTestRoot
    if (-not $Keep -and (Test-Path -LiteralPath $output)) {
        Remove-Item -LiteralPath $output -Recurse -Force
    }
}
