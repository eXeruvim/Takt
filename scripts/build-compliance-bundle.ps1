[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts/compliance",
    [string]$NativeManifest = "release/native-runtime.json",
    [string]$NativeRuntimeDirectory = "artifacts/native-runtime",
    [switch]$RequireReleaseRuntime
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))
$temporarySbom = Join-Path $repository '.oxide-cdx-tmp.json'

if (-not $output.StartsWith($repository + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог compliance должен находиться внутри репозитория: $output"
}

foreach ($required in @('Cargo.toml', 'Cargo.lock', 'LICENSE', 'THIRD_PARTY_NOTICES.md', 'dependency review', 'deny.toml')) {
    $requiredPath = Join-Path $repository $required
    if (-not (Test-Path -LiteralPath $requiredPath -PathType Leaf)) {
        throw "Отсутствует обязательный compliance-файл: $required"
    }
}

$manifestText = Get-Content -LiteralPath (Join-Path $repository 'Cargo.toml') -Raw
if ($manifestText -notmatch '(?m)^license\s*=\s*"GPL-3\.0-or-later"\s*$') {
    throw 'Cargo.toml должен явно объявлять GPL-3.0-or-later.'
}

$denyVersion = (& cargo deny --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $denyVersion -notmatch '0\.20\.2') {
    throw "Требуется cargo-deny 0.20.2, получено: $denyVersion"
}
$cycloneVersion = (& cargo cyclonedx --version 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or $cycloneVersion -notmatch '0\.5\.9') {
    throw "Требуется cargo-cyclonedx 0.5.9, получено: $cycloneVersion"
}

Push-Location $repository
try {
    & cargo deny check
    if ($LASTEXITCODE -ne 0) {
        throw 'Проверка advisory, bans, лицензий или источников зависимостей завершилась ошибкой.'
    }

    $commitTime = (& git log -1 --format=%ct 2>$null | Out-String).Trim()
    if ($LASTEXITCODE -eq 0 -and $commitTime -match '^\d+$') {
        $env:SOURCE_DATE_EPOCH = $commitTime
    }

    & cargo cyclonedx --all-features --target x86_64-pc-windows-msvc `
        --format json --spec-version 1.5 --override-filename .oxide-cdx-tmp
    if ($LASTEXITCODE -ne 0 -or -not (Test-Path -LiteralPath $temporarySbom -PathType Leaf)) {
        throw 'Не удалось создать CycloneDX SBOM.'
    }
}
finally {
    Pop-Location
}

try {
    $bom = Get-Content -LiteralPath $temporarySbom -Raw | ConvertFrom-Json -Depth 100
    # A release commit is already captured by the package/source archive. Drop
    # wall-clock metadata so the same Cargo.lock produces byte-identical SBOMs
    # on developer and CI machines.
    $bom.metadata.PSObject.Properties.Remove('timestamp')
    $version = [string]$bom.metadata.component.version
    $rootRef = [string]$bom.metadata.component.'bom-ref'
    $portableRootRef = "pkg:cargo/oxide@$version"

    function ConvertTo-PortableSbomValue {
        param([object]$Value)

        if ($null -eq $Value) {
            return $null
        }
        if ($Value -is [string]) {
            $text = [string]$Value
            if ($text.StartsWith($rootRef, [StringComparison]::Ordinal)) {
                $suffix = $text.Substring($rootRef.Length)
                if ($suffix.StartsWith(' bin-target-', [StringComparison]::Ordinal)) {
                    $suffix = '#' + $suffix.TrimStart()
                }
                return $portableRootRef + $suffix
            }
            if ($text -match '^path\+file:///.+/vendor/libmpv#2\.0\.1$') {
                return 'pkg:cargo/libmpv@2.0.1?repository_url=https%3A%2F%2Fgithub.com%2FParadoxSpiral%2Flibmpv-rs'
            }
            if ($text -match '^path\+file:///.+/vendor/libmpv-sys#3\.1\.0$') {
                return 'pkg:cargo/libmpv-sys@3.1.0?repository_url=https%3A%2F%2Fgithub.com%2FParadoxSpiral%2Flibmpv-rs'
            }
            if ($text -match '^pkg:cargo/(oxide|libmpv|libmpv-sys)@([^?]+)\?download_url=file://[^#]*(#.*)?$') {
                $fragment = if ($null -eq $Matches[3]) { '' } else { $Matches[3] }
                return "pkg:cargo/$($Matches[1])@$($Matches[2])$fragment"
            }
            return $text
        }
        if ($Value -is [System.Collections.IList]) {
            for ($index = 0; $index -lt $Value.Count; $index++) {
                $Value[$index] = ConvertTo-PortableSbomValue $Value[$index]
            }
            return $Value
        }
        if ($Value -is [pscustomobject]) {
            foreach ($property in $Value.PSObject.Properties) {
                $property.Value = ConvertTo-PortableSbomValue $property.Value
            }
            return $Value
        }
        return $Value
    }

    $bom = ConvertTo-PortableSbomValue $bom
    New-Item -ItemType Directory -Force -Path $output | Out-Null
    $sbomPath = Join-Path $output 'oxide.cdx.json'
    $bom | ConvertTo-Json -Depth 100 | Set-Content -LiteralPath $sbomPath -Encoding utf8

    $serializedBom = Get-Content -LiteralPath $sbomPath -Raw
    if ($serializedBom -match '(?i)([A-Z]:\\|path\+file:|Users[/\\])') {
        throw 'SBOM содержит локальный абсолютный путь.'
    }

    foreach ($name in @('LICENSE', 'THIRD_PARTY_NOTICES.md', 'dependency review', 'CODEC_MATRIX.md')) {
        Copy-Item -LiteralPath (Join-Path $repository $name) -Destination (Join-Path $output $name) -Force
    }
    Copy-Item -LiteralPath (Join-Path $repository 'sbom/native-runtime.cdx.json') -Destination (Join-Path $output 'native-runtime.cdx.json') -Force
    Copy-Item -LiteralPath (Join-Path $repository 'release/native-runtime-build.md') -Destination (Join-Path $output 'native-runtime-build.md') -Force

    $nativeManifestPath = [System.IO.Path]::GetFullPath((Join-Path $repository $NativeManifest))
    if (Test-Path -LiteralPath $nativeManifestPath -PathType Leaf) {
        $native = Get-Content -LiteralPath $nativeManifestPath -Raw | ConvertFrom-Json
        if ($RequireReleaseRuntime -and $native.release_eligible -ne $true) {
            throw 'Native runtime не помечен как release_eligible.'
        }
        foreach ($field in @('url', 'sha256', 'size', 'github_asset_id', 'github_actions_run_id')) {
            if ($RequireReleaseRuntime -and [string]::IsNullOrWhiteSpace([string]$native.upstream_asset.$field)) {
                throw "Native manifest: не заполнено upstream_asset.$field."
            }
        }
        foreach ($field in @('mpv', 'ffmpeg', 'libass')) {
            if ($RequireReleaseRuntime -and [string]::IsNullOrWhiteSpace([string]$native.runtime_versions.$field)) {
                throw "Native manifest: не заполнено runtime_versions.$field."
            }
        }
        foreach ($source in @($native.embedded_sources)) {
            foreach ($field in @('name', 'version', 'license_expression', 'source_commit', 'source_archive_url', 'source_archive_sha256')) {
                if ($RequireReleaseRuntime -and [string]::IsNullOrWhiteSpace([string]$source.$field)) {
                    throw "Native manifest: для embedded source $($source.name) не заполнено поле $field."
                }
            }
        }
        foreach ($component in @($native.components)) {
            foreach ($field in @('name', 'binary', 'sha256', 'version', 'license_expression', 'source_repository', 'source_commit', 'source_archive_url', 'source_archive_sha256', 'build_workflow_url', 'build_commands_file', 'corresponding_source_url')) {
                $value = [string]$component.$field
                if ($RequireReleaseRuntime -and ([string]::IsNullOrWhiteSpace($value) -or $value -eq 'REQUIRED' -or $value -like 'REQUIRED_*')) {
                    throw "Native manifest: для $($component.name) не заполнено поле $field."
                }
            }
            if ($RequireReleaseRuntime) {
                if (@($component.license_files).Count -eq 0) {
                    throw "Native manifest: для $($component.name) не указаны license_files."
                }
                foreach ($urlField in @('source_repository', 'source_archive_url', 'build_workflow_url', 'corresponding_source_url')) {
                    $uri = $null
                    if (-not [Uri]::TryCreate([string]$component.$urlField, [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -ne 'https') {
                        throw "Native manifest: поле $urlField должно быть абсолютным HTTPS URL."
                    }
                }
                $commandsPath = Join-Path $repository ([string]$component.build_commands_file)
                if (-not (Test-Path -LiteralPath $commandsPath -PathType Leaf)) {
                    throw "Не найдено описание native-сборки: $commandsPath"
                }
                $runtimeDirectory = [System.IO.Path]::GetFullPath((Join-Path $repository $NativeRuntimeDirectory))
                $binaryPath = Join-Path $runtimeDirectory ([string]$component.binary)
                if (-not (Test-Path -LiteralPath $binaryPath -PathType Leaf)) {
                    throw "Native runtime отсутствует: $binaryPath"
                }
                $actualHash = (Get-FileHash -LiteralPath $binaryPath -Algorithm SHA256).Hash
                if ($actualHash -ne ([string]$component.sha256).ToUpperInvariant()) {
                    throw "SHA-256 не совпадает для $($component.binary)."
                }
                foreach ($licenseFile in @($component.license_files)) {
                    $licensePath = Join-Path $runtimeDirectory ([string]$licenseFile)
                    if (-not (Test-Path -LiteralPath $licensePath -PathType Leaf)) {
                        throw "Native license отсутствует: $licensePath"
                    }
                    $nativeLicenseOutput = Join-Path $output 'native-licenses'
                    New-Item -ItemType Directory -Path $nativeLicenseOutput -Force | Out-Null
                    Copy-Item -LiteralPath $licensePath -Destination (Join-Path $nativeLicenseOutput ([string]$licenseFile)) -Force
                }
            }
        }
        Copy-Item -LiteralPath $nativeManifestPath -Destination (Join-Path $output 'native-runtime.json') -Force
    }
    elseif ($RequireReleaseRuntime) {
        throw "Не найден release native manifest: $nativeManifestPath"
    }

    $hashFiles = Get-ChildItem -LiteralPath $output -File -Recurse | Where-Object Name -ne 'SHA256SUMS.txt' | Sort-Object FullName
    $hashLines = foreach ($file in $hashFiles) {
        $hash = (Get-FileHash -LiteralPath $file.FullName -Algorithm SHA256).Hash.ToLowerInvariant()
        $relative = [System.IO.Path]::GetRelativePath($output, $file.FullName).Replace('\', '/')
        "$hash  $relative"
    }
    $hashLines | Set-Content -LiteralPath (Join-Path $output 'SHA256SUMS.txt') -Encoding ascii

    Write-Host "Compliance bundle создан: $output"
    if (-not $RequireReleaseRuntime) {
        Write-Warning 'Native runtime не утверждён: этот bundle предназначен для разработки, не для публичного бинарного релиза.'
    }
}
finally {
    if (Test-Path -LiteralPath $temporarySbom -PathType Leaf) {
        Remove-Item -LiteralPath $temporarySbom -Force
    }
}
