[CmdletBinding()]
param(
    [ValidateSet('audio_metadata', 'artwork')]
    [string]$Target = 'audio_metadata',

    [ValidateRange(1, 86400)]
    [int]$DurationSeconds = 300,

    [ValidateRange(1, 67108864)]
    [int]$MaxInputBytes = 16777216
)

Set-StrictMode -Version Latest
$ErrorActionPreference = 'Stop'

$repositoryRoot = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$fuzzTargetDirectory = Join-Path $repositoryRoot 'fuzz\target'
$vswhere = 'C:\Program Files (x86)\Microsoft Visual Studio\Installer\vswhere.exe'

if (-not (Test-Path -LiteralPath $vswhere -PathType Leaf)) {
    throw 'Не найден Visual Studio Installer (vswhere.exe). Установите MSVC v143 x64/x86 build tools.'
}

$visualStudioRoot = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath | Select-Object -First 1)
if ([string]::IsNullOrWhiteSpace($visualStudioRoot)) {
    throw 'Не найдены MSVC v143 x64/x86 build tools.'
}

$msvcToolsRoot = Join-Path $visualStudioRoot 'VC\Tools\MSVC'
$msvcBin = Get-ChildItem -LiteralPath $msvcToolsRoot -Directory -ErrorAction Stop |
    Sort-Object Name -Descending |
    ForEach-Object { Join-Path $_.FullName 'bin\Hostx64\x64' } |
    Where-Object { Test-Path -LiteralPath (Join-Path $_ 'link.exe') -PathType Leaf } |
    Select-Object -First 1

if ([string]::IsNullOrWhiteSpace($msvcBin)) {
    throw 'Не найден 64-битный MSVC linker (link.exe).'
}

$asanLibraryName = 'clang_rt.asan_dynamic_runtime_thunk-x86_64.lib'
$asanSearchRoots = @(
    'C:\Program Files\LLVM\lib\clang',
    (Join-Path $visualStudioRoot 'VC\Tools\MSVC')
)
$asanLibrary = $null
foreach ($searchRoot in $asanSearchRoots) {
    if (-not (Test-Path -LiteralPath $searchRoot -PathType Container)) {
        continue
    }

    $asanLibrary = Get-ChildItem -LiteralPath $searchRoot -Recurse -File -Filter $asanLibraryName -ErrorAction SilentlyContinue |
        Sort-Object LastWriteTimeUtc -Descending |
        Select-Object -First 1
    if ($null -ne $asanLibrary) {
        break
    }
}

if ($null -eq $asanLibrary) {
    throw 'Не найдена x64-библиотека AddressSanitizer. Установите C++ AddressSanitizer в Visual Studio Installer или LLVM с ASan runtime.'
}

$asanDirectory = $asanLibrary.DirectoryName
$env:PATH = "$msvcBin;$asanDirectory;$env:PATH"
$env:LIB = if ([string]::IsNullOrWhiteSpace($env:LIB)) {
    $asanDirectory
} else {
    "$asanDirectory;$env:LIB"
}

& cargo +nightly fuzz --version
if ($LASTEXITCODE -ne 0) {
    throw 'cargo-fuzz не установлен. Выполните: cargo +nightly install cargo-fuzz --locked'
}

Push-Location $repositoryRoot
try {
    $fuzzArguments = @(
        '+nightly',
        'fuzz',
        'run',
        $Target,
        '--target-dir',
        $fuzzTargetDirectory,
        '--',
        "-max_total_time=$DurationSeconds",
        '-timeout=10',
        "-max_len=$MaxInputBytes",
        '-verbosity=0',
        '-print_final_stats=1'
    )
    & cargo @fuzzArguments
    if ($LASTEXITCODE -ne 0) {
        throw "Fuzz-target '$Target' завершился с кодом $LASTEXITCODE."
    }
} finally {
    Pop-Location
}
