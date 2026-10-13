[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts/native-runtime",
    [string]$Manifest = "release/native-runtime.json",
    [string]$ArchivePath
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$manifestPath = [System.IO.Path]::GetFullPath((Join-Path $repository $Manifest))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))

if (-not $output.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог native runtime должен находиться внутри репозитория: $output"
}
if (-not (Test-Path -LiteralPath $manifestPath -PathType Leaf)) {
    throw "Не найден native manifest: $manifestPath"
}

$native = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
if ($native.release_eligible -ne $true -or @($native.components).Count -ne 1) {
    throw 'Native manifest не утверждён или содержит неожиданный набор компонентов.'
}
$component = @($native.components)[0]
$asset = $native.upstream_asset
foreach ($value in @($asset.url, $asset.sha256, $component.binary, $component.sha256,
        $component.import_library_archive_member, $component.import_library_sha256)) {
    if ([string]::IsNullOrWhiteSpace([string]$value)) {
        throw 'Native manifest содержит незаполненное обязательное поле.'
    }
}
if ([Uri]$asset.url -isnot [Uri] -or -not ([Uri]$asset.url).IsAbsoluteUri -or ([Uri]$asset.url).Scheme -ne 'https') {
    throw 'Native runtime разрешено получать только по закреплённому HTTPS URL.'
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
$archive = if ([string]::IsNullOrWhiteSpace($ArchivePath)) {
    Join-Path $output 'libmpv-upstream.zip'
} else {
    [System.IO.Path]::GetFullPath((Join-Path $repository $ArchivePath))
}

if (-not (Test-Path -LiteralPath $archive -PathType Leaf)) {
    $partial = "$archive.partial"
    if (Test-Path -LiteralPath $partial) {
        Remove-Item -LiteralPath $partial -Force
    }
    try {
        Invoke-WebRequest -Uri ([string]$asset.url) -OutFile $partial
        Move-Item -LiteralPath $partial -Destination $archive
    }
    finally {
        if (Test-Path -LiteralPath $partial) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

$archiveHash = (Get-FileHash -LiteralPath $archive -Algorithm SHA256).Hash
if ($archiveHash -ne ([string]$asset.sha256).ToUpperInvariant()) {
    throw "SHA-256 архива libmpv не совпадает: $archiveHash"
}
if ((Get-Item -LiteralPath $archive).Length -ne [long]$asset.size) {
    throw 'Размер архива libmpv не совпадает с native manifest.'
}

$extract = Join-Path $output '.extract'
if (Test-Path -LiteralPath $extract) {
    Remove-Item -LiteralPath $extract -Recurse -Force
}
New-Item -ItemType Directory -Path $extract | Out-Null
try {
    Expand-Archive -LiteralPath $archive -DestinationPath $extract

    $copies = @(
        @([string]$component.archive_member, [string]$component.binary, [string]$component.sha256, [long]$component.size),
        @([string]$component.import_library_archive_member, [string]$component.import_library, [string]$component.import_library_sha256, 0),
        @([string]@($component.license_files)[0], [string]@($component.license_files)[0], '', 0)
    )
    foreach ($copy in $copies) {
        $source = Join-Path $extract $copy[0]
        if (-not (Test-Path -LiteralPath $source -PathType Leaf)) {
            throw "В upstream-архиве отсутствует обязательный файл: $($copy[0])"
        }
        if (-not [string]::IsNullOrWhiteSpace([string]$copy[2])) {
            $memberHash = (Get-FileHash -LiteralPath $source -Algorithm SHA256).Hash
            if ($memberHash -ne ([string]$copy[2]).ToUpperInvariant()) {
                throw "SHA-256 не совпадает для $($copy[0])."
            }
        }
        if ([long]$copy[3] -gt 0 -and (Get-Item -LiteralPath $source).Length -ne [long]$copy[3]) {
            throw "Размер не совпадает для $($copy[0])."
        }
        Copy-Item -LiteralPath $source -Destination (Join-Path $output $copy[1]) -Force
    }
    Copy-Item -LiteralPath $manifestPath -Destination (Join-Path $output 'native-runtime.json') -Force
}
finally {
    if (Test-Path -LiteralPath $extract) {
        Remove-Item -LiteralPath $extract -Recurse -Force
    }
}

Write-Host "Проверенный native runtime подготовлен: $output"
