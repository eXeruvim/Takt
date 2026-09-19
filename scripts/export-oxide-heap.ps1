[CmdletBinding()]
param(
    [string]$TracePath,
    [switch]$Symbols
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

function Set-AttributeValue {
    param(
        [Parameter(Mandatory = $true)][System.Xml.XmlElement]$Element,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )

    $Element.SetAttribute($Name, $Value)
}

function Convert-WpaNumber {
    param([Parameter(Mandatory = $true)][string]$Value)

    if ([string]::IsNullOrWhiteSpace($Value)) {
        return 0.0
    }
    return [double]::Parse(
        $Value,
        [System.Globalization.NumberStyles]::Float -bor
            [System.Globalization.NumberStyles]::AllowThousands,
        [System.Globalization.CultureInfo]::CurrentCulture
    )
}

function Get-MetricColumn {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [Parameter(Mandatory = $true)][string]$Prefix
    )

    $column = @($Rows[0].PSObject.Properties.Name) |
        Where-Object { $_ -eq $Prefix -or $_ -like "$Prefix (*" } |
        Select-Object -First 1
    if ([string]::IsNullOrWhiteSpace($column)) {
        throw "В WPA-таблице отсутствует столбец '$Prefix'."
    }
    return $column
}

function Get-StackFrame {
    param([Parameter(Mandatory = $true)][string]$Stack)

    $separator = $Stack.LastIndexOf('\')
    if ($separator -lt 0) {
        return $Stack
    }
    return $Stack.Substring($separator + 1)
}

function Get-StackDepth {
    param([Parameter(Mandatory = $true)][string]$Stack)

    return $Stack.Split('\').Length - 1
}

function Measure-StackRows {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [Parameter(Mandatory = $true)][string]$ImpactingColumn,
        [Parameter(Mandatory = $true)][string]$SizeColumn
    )

    return @($Rows | ForEach-Object {
        [pscustomobject][ordered]@{
            name = [string]$_.Stack
            allocation_count = [long](Convert-WpaNumber ([string]$_.Count))
            outstanding_mib = [math]::Round(
                (Convert-WpaNumber ([string]$_.$ImpactingColumn)) / 1MB,
                4
            )
            total_allocated_mib = [math]::Round(
                (Convert-WpaNumber ([string]$_.$SizeColumn)) / 1MB,
                4
            )
        }
    } | Sort-Object outstanding_mib -Descending)
}

function Measure-InclusiveStackLandmarks {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [Parameter(Mandatory = $true)][string]$ImpactingColumn,
        [Parameter(Mandatory = $true)][string]$SizeColumn,
        [ValidateSet('Frame', 'Module')][string]$GroupBy = 'Frame',
        [string]$FramePattern = '*'
    )

    $landmarks = foreach ($row in $Rows) {
        $frame = Get-StackFrame ([string]$row.Stack)
        if ($frame -notlike $FramePattern) {
            continue
        }
        $name = if ($GroupBy -eq 'Module') {
            ($frame -split '!', 2)[0]
        }
        else {
            $frame
        }
        if ([string]::IsNullOrWhiteSpace($name) -or $name -eq '[Root]') {
            continue
        }
        [pscustomobject]@{
            name = $name
            outstanding_bytes = Convert-WpaNumber ([string]$row.$ImpactingColumn)
            total_bytes = Convert-WpaNumber ([string]$row.$SizeColumn)
        }
    }

    return @($landmarks | Group-Object name | ForEach-Object {
        $largest = $_.Group |
            Sort-Object outstanding_bytes -Descending |
            Select-Object -First 1
        [pscustomobject][ordered]@{
            name = [string]$largest.name
            maximum_inclusive_outstanding_mib = [math]::Round(
                [double]$largest.outstanding_bytes / 1MB,
                4
            )
            total_allocated_mib_at_maximum = [math]::Round(
                [double]$largest.total_bytes / 1MB,
                4
            )
        }
    } | Sort-Object maximum_inclusive_outstanding_mib -Descending)
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$traceRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'benchmark-results\wpr')
)

if ([string]::IsNullOrWhiteSpace($TracePath)) {
    $passport = Get-ChildItem -LiteralPath $traceRoot -Filter '*.json' -File `
        -ErrorAction Stop |
        Sort-Object LastWriteTime -Descending |
        Where-Object {
            $candidate = Get-Content -LiteralPath $_.FullName -Raw `
                -ErrorAction Stop | ConvertFrom-Json -ErrorAction Stop
            $candidate.kind -eq 'oxide-wpr-trace' -and
                $candidate.profile -eq 'Heap'
        } |
        Select-Object -First 1
    if ($null -eq $passport) {
        throw 'Паспорт Heap-трассы не найден.'
    }
}
else {
    $trace = [System.IO.Path]::GetFullPath($TracePath)
    if (-not (Test-Path -LiteralPath $trace -PathType Leaf)) {
        throw "ETL-трасса не найдена: $trace"
    }
    $passportPath = [System.IO.Path]::ChangeExtension($trace, '.json')
    if (-not (Test-Path -LiteralPath $passportPath -PathType Leaf)) {
        throw "Рядом с ETL нет паспорта: $passportPath"
    }
    $passport = Get-Item -LiteralPath $passportPath -ErrorAction Stop
}

$metadata = Get-Content -LiteralPath $passport.FullName -Raw -ErrorAction Stop |
    ConvertFrom-Json -ErrorAction Stop
if ($metadata.kind -ne 'oxide-wpr-trace' -or
    $metadata.schema -ne 1 -or
    $metadata.profile -ne 'Heap') {
    throw "Паспорт не описывает поддерживаемую Heap-трассу: $($passport.FullName)"
}
if ([int]$metadata.process_id -le 0) {
    throw 'В паспорте отсутствует корректный PID Oxide.'
}

$trace = [System.IO.Path]::GetFullPath(
    (Join-Path $passport.DirectoryName ([string]$metadata.trace_file))
)
if (-not $trace.StartsWith(
    $traceRoot + [System.IO.Path]::DirectorySeparatorChar,
    [System.StringComparison]::OrdinalIgnoreCase
)) {
    throw "Трасса должна находиться внутри $traceRoot"
}
if (-not (Test-Path -LiteralPath $trace -PathType Leaf)) {
    throw "ETL-трасса не найдена: $trace"
}

$wpaPackage = Get-AppxPackage -Name Microsoft.WindowsPerformanceAnalyzer `
    -ErrorAction SilentlyContinue |
    Sort-Object Version -Descending |
    Select-Object -First 1
if ($null -eq $wpaPackage -or
    [string]::IsNullOrWhiteSpace($wpaPackage.InstallLocation)) {
    throw 'Не удалось определить каталог установленного Windows Performance Analyzer.'
}
$wpaExporter = Get-Command wpaexporter.exe -ErrorAction SilentlyContinue
if ($null -eq $wpaExporter) {
    $packagedExporter = Join-Path $wpaPackage.InstallLocation 'wpaexporter.exe'
    if (Test-Path -LiteralPath $packagedExporter -PathType Leaf) {
        $wpaExporter = Get-Item -LiteralPath $packagedExporter -ErrorAction Stop
    }
}
if ($null -eq $wpaExporter) {
    throw 'WPA Exporter не найден. Установите Windows Performance Analyzer.'
}
$sourceProfile = Join-Path $wpaPackage.InstallLocation `
    'CustomDataSources\Event_Tracing_for_Windows\Catalog\WindowsStoreAppMemoryAnalysis.wpaProfile'
if (-not (Test-Path -LiteralPath $sourceProfile -PathType Leaf)) {
    throw "Стандартный Heap-профиль WPA не найден: $sourceProfile"
}

$traceBaseName = [System.IO.Path]::GetFileNameWithoutExtension($trace)
$analysisRoot = Join-Path $traceRoot 'analysis'
$output = Join-Path $analysisRoot `
    "$traceBaseName-heap-$([DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss'))"
New-Item -ItemType Directory -Path $output -ErrorAction Stop | Out-Null
$derivedProfile = Join-Path $output 'oxide-heap.wpaProfile'

[xml]$profile = Get-Content -LiteralPath $sourceProfile -Raw -ErrorAction Stop
$sourceGraph = $profile.SelectSingleNode(
    '//*[local-name()="FullHeapGraphTreeItem"]'
)
if ($null -eq $sourceGraph) {
    throw 'Стандартный WPA-профиль не содержит Full Heap graph.'
}
$view = $sourceGraph.ParentNode.CloneNode($true)
Set-AttributeValue $view 'IsSelected' 'true'
Set-AttributeValue $view 'IsVisible' 'true'
$graph = $view.SelectSingleNode(
    './*[local-name()="FullHeapGraphTreeItem"]'
)
$preset = $graph.SelectSingleNode('.//*[local-name()="HdvViewModelPreset"]')
if ($null -eq $preset) {
    throw 'Full Heap graph не содержит preset.'
}

Set-AttributeValue $preset 'Name' 'Oxide Heap Outstanding by Stack'
Set-AttributeValue $preset 'GraphColumnCount' '4'
Set-AttributeValue $preset 'KeyColumnCount' '1'
Set-AttributeValue $preset 'LeftFrozenColumnCount' '1'
Set-AttributeValue $preset 'RightFrozenColumnCount' '3'
Set-AttributeValue $preset 'InitialSelectionQuery' '([Series Name]:="Stack")'
Set-AttributeValue $preset 'InitialExpansionQuery' '([Series Name]:="Stack")'
$processLabel = "oxide.exe ($([int]$metadata.process_id))"
Set-AttributeValue $preset 'InitialFilterQuery' "[Process]:=`"$processLabel`""
Set-AttributeValue $preset 'InitialFilterShouldKeep' 'true'

$columns = @($preset.SelectNodes(
    './/*[local-name()="HdvColumnViewModelPreset"]'
))
$rawStack = $columns | Where-Object {
    if ($_.GetAttribute('Name') -ne 'Stack') {
        return $false
    }
    $options = $_.SelectSingleNode('.//*[local-name()="PresetStackOptions"]')
    return $null -ne $options -and -not $options.HasAttribute('Mode')
} | Select-Object -First 1
if ($null -eq $rawStack) {
    throw 'Full Heap graph не содержит raw Stack-столбец.'
}

foreach ($column in $columns) {
    $column.RemoveAttribute('IsVisible')
    $column.RemoveAttribute('SortOrder')
}
Set-AttributeValue $rawStack 'IsVisible' 'true'
Set-AttributeValue $rawStack 'SortPriority' '1'

$visibleMetrics = @(
    @{ Name = 'Count'; Priority = '2' },
    @{ Name = 'Impacting Size'; Priority = '3' },
    @{ Name = 'Size'; Priority = '4' }
)
foreach ($metric in $visibleMetrics) {
    $column = $columns | Where-Object {
        $_.GetAttribute('Name') -eq $metric.Name
    } | Select-Object -First 1
    if ($null -eq $column) {
        throw "Full Heap graph не содержит столбец '$($metric.Name)'."
    }
    Set-AttributeValue $column 'IsVisible' 'true'
    Set-AttributeValue $column 'SortPriority' $metric.Priority
    if ($metric.Name -eq 'Impacting Size') {
        Set-AttributeValue $column 'SortOrder' 'Descending'
    }
}

@($view.ChildNodes) | Where-Object {
    $_ -ne $graph -and
        $_.LocalName -notin @('AnalysisView.Title', 'AnalysisView.UISessionRef')
} | ForEach-Object { [void]$view.RemoveChild($_) }
$views = $profile.SelectSingleNode('//*[local-name()="WpaProfile.Views"]')
if ($null -eq $views) {
    throw 'Стандартный WPA-профиль не содержит коллекцию представлений.'
}
$views.RemoveAll()
[void]$views.AppendChild($view)
$modifiedGraphs = $profile.SelectSingleNode('//*[local-name()="ModifiedGraphs"]')
if ($null -ne $modifiedGraphs) {
    $modifiedGraphs.RemoveAll()
}
$profile.SelectNodes('//*[local-name()="FileReferences"]') |
    ForEach-Object { $_.RemoveAll() }

$settings = [System.Xml.XmlWriterSettings]::new()
$settings.Encoding = [System.Text.UTF8Encoding]::new($false)
$settings.Indent = $true
$writer = [System.Xml.XmlWriter]::Create($derivedProfile, $settings)
try {
    $profile.Save($writer)
}
finally {
    $writer.Dispose()
}

$arguments = @(
    '-i', $trace,
    '-profile', $derivedProfile,
    '-outputfolder', $output,
    '-outputformat', 'CSV',
    '-delimiter', ';'
)
$originalSymbolPath = $env:_NT_SYMBOL_PATH
try {
    if ($Symbols) {
        $env:_NT_SYMBOL_PATH = Join-Path $repository 'target\profiling'
        $arguments += '-symbols'
    }
    else {
        $arguments += '-symcacheonly'
    }
    $exportOutput = (& $wpaExporter.Source @arguments 2>&1 | Out-String).Trim()
}
finally {
    $env:_NT_SYMBOL_PATH = $originalSymbolPath
}
if ($LASTEXITCODE -ne 0 -or
    $exportOutput -match '(?i)filter query.+invalid|error exporting profile') {
    throw "WPA Exporter не смог построить Heap-срез: $exportOutput"
}

$tables = @(Get-ChildItem -LiteralPath $output -Filter '*.csv' -File `
    -ErrorAction Stop)
if ($tables.Count -ne 1) {
    throw "Ожидалась одна Heap-таблица, получено $($tables.Count)."
}
$rows = @(Import-Csv -LiteralPath $tables[0].FullName -Delimiter ';')
if ($rows.Count -eq 0) {
    throw 'WPA Exporter вернул пустую Heap-таблицу.'
}
if ('Stack' -notin @($rows[0].PSObject.Properties.Name)) {
    throw 'WPA Heap-таблица не содержит Stack.'
}
$impactingColumn = Get-MetricColumn $rows 'Impacting Size'
$sizeColumn = Get-MetricColumn $rows 'Size'
$root = $rows | Where-Object { $_.Stack -eq '[Root]' } | Select-Object -First 1
if ($null -eq $root) {
    throw 'WPA Heap-таблица не содержит корневой итог.'
}
$shallowStackDepth = 3
$shallowRows = @($rows | Where-Object {
    (Get-StackDepth ([string]$_.Stack)) -eq $shallowStackDepth
})

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-wpa-heap-summary'
    trace_file = [System.IO.Path]::GetFileName($trace)
    git_commit = [string]$metadata.git_commit
    package_version = [string]$metadata.package_version
    process_id = [int]$metadata.process_id
    scenario = [string]$metadata.scenario
    requested_duration_seconds = [int]$metadata.requested_duration_seconds
    outstanding_mib = [math]::Round(
        (Convert-WpaNumber ([string]$root.$impactingColumn)) / 1MB,
        4
    )
    total_allocated_mib = [math]::Round(
        (Convert-WpaNumber ([string]$root.$sizeColumn)) / 1MB,
        4
    )
    allocation_count = [long](Convert-WpaNumber ([string]$root.Count))
    stack_semantics = 'inclusive; stack rows and landmarks overlap and must not be summed'
    stack_row_count = $rows.Count
    shallow_stack_depth = $shallowStackDepth
    shallow_stack_branches = @(
        Measure-StackRows $shallowRows $impactingColumn $sizeColumn
    )
    app_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $rows $impactingColumn $sizeColumn `
            'Frame' 'oxide.exe!oxide::*' | Select-Object -First 30
    )
    module_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $rows $impactingColumn $sizeColumn `
            'Module' | Select-Object -First 30
    )
    symbols_requested = [bool]$Symbols
}
$summaryPath = Join-Path $output 'oxide-heap-summary.json'
$summary | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $summaryPath -Encoding utf8 -ErrorAction Stop

Write-Host "Heap-анализ Oxide: $output"
Write-Host (
    'Outstanding: {0} МиБ; всего выделено: {1} МиБ; событий: {2}.' -f
        $summary.outstanding_mib,
        $summary.total_allocated_mib,
        $summary.allocation_count
)
Write-Host 'Крупнейшие непересекающиеся Stack-ветви на глубине 3:'
$summary.shallow_stack_branches | Select-Object -First 15 `
    @{ Name = 'frame'; Expression = { Get-StackFrame $_.name } }, `
    allocation_count,
    outstanding_mib,
    total_allocated_mib |
    Format-Table -AutoSize
Write-Host 'Ориентиры в стеках Oxide (включающие, не складывать):'
$summary.app_stack_landmarks | Select-Object -First 15 `
    @{ Name = 'frame'; Expression = {
        $frame = $_.name -replace '^oxide\.exe!', ''
        if ($frame.Length -gt 80) {
            return $frame.Substring(0, 77) + '...'
        }
        return $frame
    } }, `
    maximum_inclusive_outstanding_mib,
    total_allocated_mib_at_maximum |
    Format-Table -AutoSize
Write-Host "Сводка: $summaryPath"
