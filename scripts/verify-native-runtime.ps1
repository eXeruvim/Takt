[CmdletBinding()]
param(
    [string]$Executable = "target/debug/oxide.exe",
    [string]$RuntimeDirectory = "artifacts/native-runtime",
    [string]$Manifest = "release/native-runtime.json"
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$executablePath = [System.IO.Path]::GetFullPath((Join-Path $repository $Executable))
$runtime = [System.IO.Path]::GetFullPath((Join-Path $repository $RuntimeDirectory))
$manifestPath = [System.IO.Path]::GetFullPath((Join-Path $repository $Manifest))

foreach ($path in @($executablePath, $runtime, $manifestPath)) {
    if (-not $path.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
        throw "Проверяемый путь находится вне репозитория: $path"
    }
}
if (-not (Test-Path -LiteralPath $executablePath -PathType Leaf)) {
    throw "Не найден oxide.exe для native-проверки: $executablePath"
}
if (-not (Test-Path -LiteralPath (Join-Path $runtime 'libmpv-2.dll') -PathType Leaf)) {
    throw "Не найден подготовленный libmpv-2.dll: $runtime"
}

$native = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
$probe = Join-Path $runtime 'oxide-native-info.exe'
Copy-Item -LiteralPath $executablePath -Destination $probe -Force
try {
    $output = (& $probe --native-info 2>&1 | Out-String).Trim()
    if ($LASTEXITCODE -ne 0) {
        throw "Native probe завершился с кодом $LASTEXITCODE`: $output"
    }
    try {
        $info = $output | ConvertFrom-Json
    }
    catch {
        throw "Native probe вернул не JSON: $output"
    }
    foreach ($field in @('mpv', 'ffmpeg', 'libass')) {
        $expected = [string]$native.runtime_versions.$field
        $actual = [string]$info.$field
        if ([string]::IsNullOrWhiteSpace($expected) -or $actual -ne $expected) {
            throw "Native runtime $field не совпадает: ожидалось '$expected', получено '$actual'."
        }
    }
    Write-Host "Native runtime загружен и соответствует manifest: $output"
}
finally {
    if (Test-Path -LiteralPath $probe -PathType Leaf) {
        Remove-Item -LiteralPath $probe -Force
    }
}
