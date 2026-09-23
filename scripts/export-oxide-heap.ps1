[CmdletBinding()]
param(
    [string]$TracePath,
    [switch]$Symbols,

    [ValidateRange(3, 64)]
    [int]$StackDepth = 20
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

function Convert-MetricToMiB {
    param(
        [Parameter(Mandatory = $true)][string]$Value,
        [Parameter(Mandatory = $true)][string]$Column
    )

    $number = Convert-WpaNumber $Value
    if ($Column -like '* (B)') {
        return $number / 1MB
    }
    if ($Column -like '* (KB)') {
        return $number / 1KB
    }
    if ($Column -like '* (MB)') {
        return $number
    }
    if ($Column -like '* (GB)') {
        return $number * 1KB
    }
    throw "Неизвестная единица измерения WPA-столбца '$Column'."
}

function Get-StackFrame {
    param([Parameter(Mandatory = $true)][string]$Stack)

    $separator = $Stack.LastIndexOfAny([char[]]@('\', '/'))
    if ($separator -lt 0) {
        return $Stack
    }
    return $Stack.Substring($separator + 1)
}

function Get-StackDepth {
    param([Parameter(Mandatory = $true)][string]$Stack)

    return @($Stack -split '[\\/]').Length - 1
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
                (Convert-MetricToMiB `
                    ([string]$_.$ImpactingColumn) $ImpactingColumn),
                4
            )
            total_allocated_mib = [math]::Round(
                (Convert-MetricToMiB ([string]$_.$SizeColumn) $SizeColumn),
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
            outstanding_mib = Convert-MetricToMiB `
                ([string]$row.$ImpactingColumn) $ImpactingColumn
            total_mib = Convert-MetricToMiB `
                ([string]$row.$SizeColumn) $SizeColumn
        }
    }

    return @($landmarks | Group-Object name | ForEach-Object {
        $largest = $_.Group |
            Sort-Object outstanding_mib -Descending |
            Select-Object -First 1
        [pscustomobject][ordered]@{
            name = [string]$largest.name
            maximum_inclusive_outstanding_mib = [math]::Round(
                [double]$largest.outstanding_mib,
                4
            )
            total_allocated_mib_at_maximum = [math]::Round(
                [double]$largest.total_mib,
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
$presetContainer = $graph.SelectSingleNode(
    './*[local-name()="FullHeapGraphTreeItem.HdvViewModelPreset"]'
)
if ($null -eq $presetContainer) {
    throw 'Full Heap graph не содержит контейнер preset.'
}
$presetContainer.RemoveAll()
[xml]$presetDocument = @'
<HdvViewModelPreset
    xmlns="clr-namespace:Microsoft.Performance.UI.DataEngine.ViewModelPresets;assembly=Microsoft.Performance.UI.DataEngine"
    Name="Oxide Heap Totals"
    GraphChartType="StackedBars"
    BarGraphIntervalCount="100"
    GraphColumnCount="5"
    KeyColumnCount="2"
    LeftFrozenColumnCount="2"
    RightFrozenColumnCount="3"
    InitialFilterQuery=""
    GraphFilterColumnGuid="642a8591-f858-4345-b7fa-99e8e4567cca">
  <HdvViewModelPreset.ColumnMetadataEntries />
  <HdvViewModelPreset.HighlightEntries />
  <HdvViewModelPreset.ConfigurableColumns>
    <HdvColumnViewModelPreset Guid="06155709-500b-4182-8944-29665014b5c2" Name="Process" IsVisible="True" SortPriority="1" Width="150" />
    <HdvColumnViewModelPreset Guid="759dc840-5079-49db-bae0-3c54e553b7fd" Name="Stack" IsVisible="True" SortPriority="2" Width="280">
      <HdvColumnViewModelPreset.Parameter>
        <PresetStackOptions xmlns="clr-namespace:Microsoft.Performance.Shell.Stacks;assembly=Microsoft.Performance.Shell" />
      </HdvColumnViewModelPreset.Parameter>
    </HdvColumnViewModelPreset>
    <HdvColumnViewModelPreset Guid="62f5e535-a192-475d-8d4d-a725b8a160bd" Name="Count" AggregationMode="Sum" IsVisible="True" SortPriority="3" Width="80" />
    <HdvColumnViewModelPreset Guid="642a8591-f858-4345-b7fa-99e8e4567cca" Name="Impacting Size" AggregationMode="Sum" IsVisible="True" SortOrder="Descending" SortPriority="4" Width="120" />
    <HdvColumnViewModelPreset Guid="29a4dfc2-cd62-4de8-85c3-f580af6a47dd" Name="Size" AggregationMode="Sum" IsVisible="True" SortPriority="5" Width="120" />
  </HdvViewModelPreset.ConfigurableColumns>
</HdvViewModelPreset>
'@
$preset = $profile.ImportNode($presetDocument.DocumentElement, $true)
[void]$presetContainer.AppendChild($preset)
$processLabel = "oxide.exe ($([int]$metadata.process_id))"
Set-AttributeValue $preset 'InitialFilterQuery' "[Process]:=`"$processLabel`""
Set-AttributeValue $preset 'InitialFilterShouldKeep' 'true'
$stackGraph = $graph.CloneNode($true)
$stackPreset = $stackGraph.SelectSingleNode(
    './/*[local-name()="HdvViewModelPreset"]'
)
Set-AttributeValue $stackPreset 'Name' 'Oxide Heap by Stack'
Set-AttributeValue $stackPreset 'InitialSelectionQuery' `
    '([Series Name]:="Stack")'
Set-AttributeValue $stackPreset 'InitialExpansionQuery' `
    "[Series Name]:=`"Stack`" [Series Depth]:<=$StackDepth"

@($view.ChildNodes) | Where-Object {
    $_ -ne $graph -and
        $_.LocalName -notin @('AnalysisView.Title', 'AnalysisView.UISessionRef')
} | ForEach-Object { [void]$view.RemoveChild($_) }
[void]$view.AppendChild($stackGraph)
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
if ($tables.Count -ne 2) {
    throw "Ожидались две Heap-таблицы, получено $($tables.Count)."
}
$maximumCsvBytes = 512MB
$oversizedTable = $tables | Where-Object Length -GT $maximumCsvBytes |
    Select-Object -First 1
if ($null -ne $oversizedTable) {
    throw (
        'WPA Heap-таблица не была агрегирована и слишком велика для безопасного ' +
        "импорта: $($oversizedTable.Name), " +
        "$([math]::Round($oversizedTable.Length / 1MB, 1)) МиБ."
    )
}
$totalsTable = $tables | Where-Object Name -Like '*_Oxide_Heap_Totals.csv' |
    Select-Object -First 1
$stackTable = $tables | Where-Object Name -Like '*_Oxide_Heap_by_Stack.csv' |
    Select-Object -First 1
if ($null -eq $totalsTable -or $null -eq $stackTable) {
    throw 'WPA Exporter не вернул ожидаемые Totals/Stack Heap-таблицы.'
}
$totalRows = @(Import-Csv -LiteralPath $totalsTable.FullName -Delimiter ';')
$stackRows = @(Import-Csv -LiteralPath $stackTable.FullName -Delimiter ';')
if ($totalRows.Count -ne 1 -or $stackRows.Count -eq 0) {
    throw (
        "Некорректные Heap-таблицы: totals=$($totalRows.Count), " +
        "stack=$($stackRows.Count)."
    )
}
if ('Stack' -notin @($stackRows[0].PSObject.Properties.Name)) {
    throw 'WPA Heap-таблица не содержит Stack.'
}
$impactingColumn = Get-MetricColumn $totalRows 'Impacting Size'
$sizeColumn = Get-MetricColumn $totalRows 'Size'
$stackImpactingColumn = Get-MetricColumn $stackRows 'Impacting Size'
$stackSizeColumn = Get-MetricColumn $stackRows 'Size'
$shallowStackDepth = 3
$total = $totalRows[0]
$outstandingMiB = Convert-MetricToMiB `
    ([string]$total.$impactingColumn) $impactingColumn
$totalAllocatedMiB = Convert-MetricToMiB `
    ([string]$total.$sizeColumn) $sizeColumn
$allocationCount = Convert-WpaNumber ([string]$total.Count)
$stackDataRows = @($stackRows | Where-Object {
    -not [string]::IsNullOrWhiteSpace([string]$_.Stack)
})
$shallowRows = @($stackDataRows | Where-Object {
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
    outstanding_mib = [math]::Round($outstandingMiB, 4)
    total_allocated_mib = [math]::Round($totalAllocatedMiB, 4)
    allocation_count = [long]$allocationCount
    stack_semantics = 'stack hierarchy is inclusive; parent and child rows overlap and must not be summed'
    stack_row_count = $stackDataRows.Count
    exported_stack_depth = $StackDepth
    shallow_stack_depth = $shallowStackDepth
    shallow_stack_branches = @(
        Measure-StackRows $shallowRows $stackImpactingColumn $stackSizeColumn
    )
    app_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $stackDataRows $stackImpactingColumn `
            $stackSizeColumn `
            'Frame' 'oxide.exe!oxide::*' | Select-Object -First 30
    )
    module_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $stackDataRows $stackImpactingColumn `
            $stackSizeColumn `
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
