[CmdletBinding()]
param(
    [ValidateRange(100, 10000)]
    [int]$FileCount = 5100,

    [ValidateRange(10, 600)]
    [int]$DurationSeconds = 30
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$fixtureRoot = [System.IO.Path]::GetFullPath((Join-Path $repository '.tmp\benchmarks'))
if (-not $fixtureRoot.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог fixture оказался вне репозитория: $fixtureRoot"
}
New-Item -ItemType Directory -Path $fixtureRoot -Force | Out-Null

$timestamp = [DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss')
$outputDirectory = Join-Path $fixtureRoot "large-playlist-$timestamp"
if (Test-Path -LiteralPath $outputDirectory) {
    throw "Каталог уже существует и не будет перезаписан: $outputDirectory"
}
New-Item -ItemType Directory -Path $outputDirectory | Out-Null

$sampleRate = 44100
$channels = 1
$bitsPerSample = 16
$bytesPerSample = [int]($bitsPerSample / 8)
$dataLength = [int64]$sampleRate * $channels * $bytesPerSample * $DurationSeconds
if ($dataLength -gt [int]::MaxValue) {
    throw 'WAV fixture превышает размер, поддерживаемый этим генератором'
}

$padding = 'x' * 112
$templateName = "benchmark-track-00000-$padding.wav"
$templatePath = Join-Path $outputDirectory $templateName
$stream = [System.IO.File]::Open($templatePath, [System.IO.FileMode]::CreateNew, [System.IO.FileAccess]::Write, [System.IO.FileShare]::Read)
try {
    $writer = [System.IO.BinaryWriter]::new($stream, [System.Text.Encoding]::ASCII, $true)
    try {
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('RIFF'))
        $writer.Write([int](36 + $dataLength))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('WAVE'))
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('fmt '))
        $writer.Write([int]16)
        $writer.Write([int16]1)
        $writer.Write([int16]$channels)
        $writer.Write([int]$sampleRate)
        $writer.Write([int]($sampleRate * $channels * $bytesPerSample))
        $writer.Write([int16]($channels * $bytesPerSample))
        $writer.Write([int16]$bitsPerSample)
        $writer.Write([System.Text.Encoding]::ASCII.GetBytes('data'))
        $writer.Write([int]$dataLength)

        $remaining = $dataLength
        $zeroes = [byte[]]::new(64KB)
        while ($remaining -gt 0) {
            $length = [int][Math]::Min($remaining, $zeroes.Length)
            $writer.Write($zeroes, 0, $length)
            $remaining -= $length
        }
    }
    finally {
        $writer.Dispose()
    }
}
finally {
    $stream.Dispose()
}

$linksPerPhysicalFile = 800
$linkSource = $templatePath
for ($index = 1; $index -lt $FileCount; $index++) {
    $name = "benchmark-track-{0:D5}-$padding.wav" -f $index
    $path = Join-Path $outputDirectory $name
    if (($index % $linksPerPhysicalFile) -eq 0) {
        [System.IO.File]::Copy($templatePath, $path, $false)
        $linkSource = $path
    }
    else {
        New-Item -ItemType HardLink -Path $path -Target $linkSource | Out-Null
    }
    if (($index % 100) -eq 0) {
        Write-Progress `
            -Activity 'Создание большой очереди' `
            -Status "$index из $FileCount файлов" `
            -PercentComplete (($index / $FileCount) * 100)
    }
}
Write-Progress -Activity 'Создание большой очереди' -Completed

$physicalCopies = [Math]::Ceiling($FileCount / $linksPerPhysicalFile)
Write-Host "Готово: $outputDirectory"
Write-Host "Файлов: $FileCount; физических WAV-шардов: $physicalCopies"
Write-Host "Физические данные: примерно $([Math]::Round($physicalCopies * (44 + $dataLength) / 1MB, 2)) МиБ"
Write-Host 'Каталог не удаляется автоматически. После теста его можно удалить через Проводник.'
