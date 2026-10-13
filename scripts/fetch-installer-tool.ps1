[CmdletBinding()]
param(
    [string]$OutputDirectory = "artifacts/tooling"
)

$ErrorActionPreference = 'Stop'
$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$artifactRoot = [System.IO.Path]::GetFullPath((Join-Path $repository 'artifacts'))
$output = [System.IO.Path]::GetFullPath((Join-Path $repository $OutputDirectory))
if (-not $output.StartsWith($artifactRoot + [System.IO.Path]::DirectorySeparatorChar, [StringComparison]::OrdinalIgnoreCase)) {
    throw "Каталог installer tooling должен находиться внутри $artifactRoot`: $output"
}

$manifestPath = Join-Path $repository 'release/installer-tool.json'
$tool = Get-Content -LiteralPath $manifestPath -Raw | ConvertFrom-Json
foreach ($field in @('version', 'url', 'sha256', 'size')) {
    if ([string]::IsNullOrWhiteSpace([string]$tool.$field)) {
        throw "Installer tool manifest не содержит поле $field."
    }
}
$uri = $null
if (-not [Uri]::TryCreate([string]$tool.url, [UriKind]::Absolute, [ref]$uri) -or $uri.Scheme -ne 'https') {
    throw 'Installer tool разрешено получать только по закреплённому HTTPS URL.'
}

New-Item -ItemType Directory -Path $output -Force | Out-Null
$installer = Join-Path $output "innosetup-$($tool.version)-x64.exe"
if (-not (Test-Path -LiteralPath $installer -PathType Leaf)) {
    $partial = "$installer.partial"
    if (Test-Path -LiteralPath $partial) {
        Remove-Item -LiteralPath $partial -Force
    }
    try {
        Invoke-WebRequest -Uri $uri -OutFile $partial
        Move-Item -LiteralPath $partial -Destination $installer
    }
    finally {
        if (Test-Path -LiteralPath $partial) {
            Remove-Item -LiteralPath $partial -Force
        }
    }
}

$hash = (Get-FileHash -LiteralPath $installer -Algorithm SHA256).Hash
if ($hash -ne ([string]$tool.sha256).ToUpperInvariant()) {
    throw "SHA-256 Inno Setup не совпадает: $hash"
}
if ((Get-Item -LiteralPath $installer).Length -ne [long]$tool.size) {
    throw 'Размер Inno Setup не совпадает с installer tool manifest.'
}
$signature = Get-AuthenticodeSignature -LiteralPath $installer
if ($signature.Status -ne [System.Management.Automation.SignatureStatus]::Valid -or
    $null -eq $signature.SignerCertificate -or
    $signature.SignerCertificate.Subject -notmatch 'Pyrsys B\.V\.') {
    throw "Authenticode-подпись Inno Setup не прошла проверку: $($signature.Status), $($signature.StatusMessage)"
}
Write-Host "Проверенный Inno Setup installer подготовлен: $installer"
