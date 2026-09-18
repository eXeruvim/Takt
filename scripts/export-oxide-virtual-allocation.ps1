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

function Measure-Dimension {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [Parameter(Mandatory = $true)][string]$Dimension
    )

    if ($Dimension -notin @($Rows[0].PSObject.Properties.Name)) {
        throw "В WPA-таблице отсутствует измерение '$Dimension'."
    }
    $committedColumn = Get-MetricColumn $Rows 'Outstanding Committed'
    $reservedColumn = Get-MetricColumn $Rows 'Outstanding Reserved'
    return @($Rows | Group-Object $Dimension | ForEach-Object {
        $first = $_.Group | Select-Object -First 1
        $committedBytes = ($_.Group | ForEach-Object {
            Convert-WpaNumber ([string]$_.$committedColumn)
        } | Measure-Object -Sum).Sum
        $reservedBytes = ($_.Group | ForEach-Object {
            Convert-WpaNumber ([string]$_.$reservedColumn)
        } | Measure-Object -Sum).Sum
        $count = ($_.Group | ForEach-Object {
            Convert-WpaNumber ([string]$_.Count)
        } | Measure-Object -Sum).Sum
        [pscustomobject][ordered]@{
            name = [string]$first.$Dimension
            allocation_count = [long]$count
            outstanding_committed_mib = [math]::Round($committedBytes / 1MB, 4)
            outstanding_reserved_mib = [math]::Round($reservedBytes / 1MB, 4)
        }
    } | Where-Object { -not [string]::IsNullOrWhiteSpace($_.name) } |
        Sort-Object outstanding_committed_mib -Descending)
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

function Measure-InclusiveStackLandmarks {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [ValidateSet('Frame', 'Module')][string]$GroupBy = 'Frame',
        [string]$FramePattern = '*'
    )

    $committedColumn = Get-MetricColumn $Rows 'Outstanding Committed'
    $reservedColumn = Get-MetricColumn $Rows 'Outstanding Reserved'
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
            committed_bytes = Convert-WpaNumber ([string]$row.$committedColumn)
            reserved_bytes = Convert-WpaNumber ([string]$row.$reservedColumn)
        }
    }

    return @($landmarks | Group-Object name | ForEach-Object {
        $largest = $_.Group |
            Sort-Object committed_bytes -Descending |
            Select-Object -First 1
        [pscustomobject][ordered]@{
            name = [string]$largest.name
            maximum_inclusive_committed_mib = [math]::Round(
                [double]$largest.committed_bytes / 1MB,
                4
            )
            reserved_mib_at_maximum = [math]::Round(
                [double]$largest.reserved_bytes / 1MB,
                4
            )
        }
    } | Sort-Object maximum_inclusive_committed_mib -Descending)
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$traceRoot = [System.IO.Path]::GetFullPath(
    (Join-Path $repository 'benchmark-results\wpr')
)

if ([string]::IsNullOrWhiteSpace($TracePath)) {
    $passport = Get-ChildItem -LiteralPath $traceRoot -Filter '*.json' -File -ErrorAction Stop |
        Sort-Object LastWriteTime -Descending |
        Where-Object {
            $candidate = Get-Content -LiteralPath $_.FullName -Raw -ErrorAction Stop |
                ConvertFrom-Json -ErrorAction Stop
            $candidate.kind -eq 'oxide-wpr-trace' -and
                $candidate.profile -eq 'VirtualAllocation'
        } |
        Select-Object -First 1
    if ($null -eq $passport) {
        throw 'Паспорт VirtualAllocation-трассы не найден.'
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
    $metadata.profile -ne 'VirtualAllocation') {
    throw "Паспорт не описывает поддерживаемую VirtualAllocation-трассу: $($passport.FullName)"
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

$wpaExporter = Get-Command wpaexporter.exe -ErrorAction SilentlyContinue
if ($null -eq $wpaExporter) {
    throw 'WPA Exporter не найден. Установите Windows Performance Analyzer.'
}
$wpaPackage = Get-AppxPackage -Name Microsoft.WindowsPerformanceAnalyzer `
    -ErrorAction SilentlyContinue |
    Sort-Object Version -Descending |
    Select-Object -First 1
if ($null -eq $wpaPackage -or [string]::IsNullOrWhiteSpace($wpaPackage.InstallLocation)) {
    throw 'Не удалось определить каталог установленного Windows Performance Analyzer.'
}
$sourceProfile = Join-Path $wpaPackage.InstallLocation `
    'CustomDataSources\Event_Tracing_for_Windows\Catalog\FullBoot.Boot.wpaProfile'
if (-not (Test-Path -LiteralPath $sourceProfile -PathType Leaf)) {
    throw "Стандартный VirtualAllocation-профиль WPA не найден: $sourceProfile"
}

$traceBaseName = [System.IO.Path]::GetFileNameWithoutExtension($trace)
$analysisRoot = Join-Path $traceRoot 'analysis'
$output = Join-Path $analysisRoot `
    "$traceBaseName-commit-$([DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss'))"
New-Item -ItemType Directory -Path $output -ErrorAction Stop | Out-Null
$derivedProfile = Join-Path $output 'oxide-virtual-allocation.wpaProfile'

[xml]$profile = Get-Content -LiteralPath $sourceProfile -Raw -ErrorAction Stop
$templateGraph = $profile.SelectSingleNode(
    '//*[local-name()="Core4SummaryTableFillerGraphTreeItem"]'
)
if ($null -eq $templateGraph) {
    throw 'Стандартный WPA-профиль не содержит Core4 graph-шаблон.'
}
$view = $templateGraph.ParentNode.CloneNode($true)
Set-AttributeValue $view 'IsSelected' 'true'
Set-AttributeValue $view 'IsVisible' 'true'
$baseGraph = $view.SelectSingleNode(
    './*[local-name()="Core4SummaryTableFillerGraphTreeItem"]'
)
Set-AttributeValue $baseGraph 'ClsID' '0d63da12-a68a-4ea8-8a12-dadd30f0fa1d'
Set-AttributeValue $baseGraph 'LayoutStyleFree' 'All'
Set-AttributeValue $baseGraph 'Color' '#FFCB6D3D'
$baseGraph.RemoveAttribute('GraphHeight')
$baseGraph.RemoveAttribute('DataTableHeight')
$presetContainer = $baseGraph.SelectSingleNode(
    './*[local-name()="Core4SummaryTableFillerGraphTreeItem.HdvViewModelPreset"]'
)
if ($null -eq $presetContainer) {
    throw 'Core4 graph-шаблон не содержит контейнер preset.'
}
$presetContainer.RemoveAll()
[xml]$virtualPresetDocument = @'
<HdvViewModelPreset
    xmlns="clr-namespace:Microsoft.Performance.UI.DataEngine.ViewModelPresets;assembly=Microsoft.Performance.UI.DataEngine"
    Name="Oxide Virtual Allocations"
    GraphChartType="StackedBars"
    GraphColumnCount="8"
    KeyColumnCount="4"
    RightFrozenColumnCount="7"
    InitialFilterQuery=""
    InitialSelectionQuery=""
    GraphFilterColumnGuid="59ad0c27-84dc-5240-7a3a-7df226f35a87">
  <HdvViewModelPreset.ColumnMetadataEntries />
  <HdvViewModelPreset.HighlightEntries />
  <HdvViewModelPreset.ConfigurableColumns>
    <HdvColumnViewModelPreset Guid="0538bf45-0df8-5413-cc78-18af5bf52e67" Name="Process" IsVisible="True" SortPriority="1" Width="180" />
    <HdvColumnViewModelPreset Guid="54f73f7d-2973-55d1-e217-85a215af8b3c" Name="Stack" SortPriority="2" Width="280" />
    <HdvColumnViewModelPreset Guid="90071a3a-e2f5-52cb-a36a-aa391a4c8aba" Name="Flags" SortPriority="3" Width="120" />
    <HdvColumnViewModelPreset Guid="bb7dcbd7-5fe2-58a1-26f8-050a16b3e71c" Name="Commit Type" SortPriority="4" Width="100" />
    <HdvColumnViewModelPreset Guid="9e4060c1-403f-5768-725c-bff338f0f738" Name="Reserve Type" SortPriority="5" Width="100" />
    <HdvColumnViewModelPreset Guid="f63d81d4-d27a-50dc-bdce-d6cef2b34ddb" Name="Count" AggregationMode="Sum" IsVisible="True" SortPriority="6" Width="80" />
    <HdvColumnViewModelPreset Guid="59ad0c27-84dc-5240-7a3a-7df226f35a87" Name="Outstanding Committed (B)" AggregationMode="Sum" IsVisible="True" SortOrder="Descending" SortPriority="7" Width="140" />
    <HdvColumnViewModelPreset Guid="206c68cd-6462-5c23-00cc-6b7dd3f90ce4" Name="Outstanding Reserved (B)" AggregationMode="Sum" IsVisible="True" SortOrder="Descending" SortPriority="8" Width="140" />
  </HdvViewModelPreset.ConfigurableColumns>
</HdvViewModelPreset>
'@
$basePreset = $profile.ImportNode($virtualPresetDocument.DocumentElement, $true)
[void]$presetContainer.AppendChild($basePreset)

function New-CommitGraph {
    param(
        [Parameter(Mandatory = $true)][System.Xml.XmlElement]$BaseGraph,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$KeyColumn,
        [Parameter(Mandatory = $true)][string]$ProcessLabel,
        [Parameter(Mandatory = $true)][System.Xml.XmlDocument]$OwnerDocument
    )

    $graph = $BaseGraph.CloneNode($true)
    $preset = $graph.SelectSingleNode('.//*[local-name()="HdvViewModelPreset"]')
    Set-AttributeValue $preset 'Name' $Name
    Set-AttributeValue $preset 'KeyColumnCount' '4'
    $preset.RemoveAttribute('InitialExpansionQuery')
    if ($KeyColumn -eq 'Stack') {
        Set-AttributeValue $preset 'InitialSelectionQuery' `
            '([Series Name]:="Stack")'
        Set-AttributeValue $preset 'InitialExpansionQuery' `
            '([Series Name]:="Stack")'
    }
    else {
        $preset.RemoveAttribute('InitialSelectionQuery')
    }
    if ($KeyColumn -eq 'Process') {
        $preset.SetAttribute('InitialFilterQuery', '')
        $preset.RemoveAttribute('InitialFilterShouldKeep')
    }
    else {
        Set-AttributeValue $preset 'InitialFilterQuery' `
            "[Process]:=`"$ProcessLabel`""
        Set-AttributeValue $preset 'InitialFilterShouldKeep' 'true'
    }

    $columns = @($preset.SelectNodes(
        './/*[local-name()="HdvColumnViewModelPreset"]'
    ))
    $key = $columns | Where-Object {
        $_.GetAttribute('Name') -eq $KeyColumn
    } | Select-Object -First 1
    if ($null -eq $key) {
        throw "В Virtual Allocations отсутствует столбец '$KeyColumn'."
    }

    foreach ($column in $columns) {
        $column.RemoveAttribute('IsVisible')
        $columnName = $column.GetAttribute('Name')
        if ($column -eq $key) {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '1'
        }
        elseif ($columnName -eq 'Outstanding Committed (B)') {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '2'
        }
        elseif ($columnName -eq 'Outstanding Reserved (B)') {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '3'
        }
        elseif ($columnName -eq 'Count') {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '4'
        }
        else {
            Set-AttributeValue $column 'SortPriority' '4'
        }
    }
    return $OwnerDocument.ImportNode($graph, $true)
}

$processLabel = "oxide.exe ($([int]$metadata.process_id))"
$graphs = @(
    New-CommitGraph $baseGraph 'Oxide Commit Total by Process' `
        'Process' $processLabel $profile
    New-CommitGraph $baseGraph 'Oxide Commit by Type' `
        'Commit Type' $processLabel $profile
    New-CommitGraph $baseGraph 'Oxide Reserve by Type' `
        'Reserve Type' $processLabel $profile
    New-CommitGraph $baseGraph 'Oxide Commit by Stack' `
        'Stack' $processLabel $profile
)
@($view.ChildNodes) | Where-Object {
    $_.LocalName -notin @('AnalysisView.Title', 'AnalysisView.UISessionRef')
} | ForEach-Object { [void]$view.RemoveChild($_) }
foreach ($graph in $graphs) {
    [void]$view.AppendChild($graph)
}

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
    throw "WPA Exporter не смог построить VirtualAllocation-срез: $exportOutput"
}

$tables = @(Get-ChildItem -LiteralPath $output -Filter '*.csv' -File -ErrorAction Stop)
if ($tables.Count -ne 4) {
    throw "Ожидалось четыре commit-таблицы, получено $($tables.Count)."
}

function Import-CommitTable {
    param(
        [Parameter(Mandatory = $true)][System.IO.FileInfo[]]$Tables,
        [Parameter(Mandatory = $true)][string]$Pattern
    )

    $table = $Tables | Where-Object Name -Like $Pattern | Select-Object -First 1
    if ($null -eq $table) {
        throw "Не найдена commit-таблица: $Pattern"
    }
    $rows = @(Import-Csv -LiteralPath $table.FullName -Delimiter ';')
    if ($rows.Count -eq 0) {
        throw "WPA Exporter вернул пустую таблицу: $($table.Name)"
    }
    return $rows
}

$processRows = @(Import-CommitTable $tables '*_Oxide_Commit_Total_by_Process.csv')
$commitTypeRows = @(Import-CommitTable $tables '*_Oxide_Commit_by_Type.csv')
$reserveTypeRows = @(Import-CommitTable $tables '*_Oxide_Reserve_by_Type.csv')
$stackRows = @(Import-CommitTable $tables '*_Oxide_Commit_by_Stack.csv')
$oxideRows = @($processRows | Where-Object { $_.Process -eq $processLabel })
if ($oxideRows.Count -eq 0) {
    $matches = @($processRows.Process | Where-Object {
        $_ -match '(?i)^oxide(?:\.exe)?\b'
    } | Sort-Object -Unique)
    throw "Commit-срез не содержит '$processLabel'. Похожие подписи: $($matches -join ', ')"
}

$committedColumn = Get-MetricColumn $oxideRows 'Outstanding Committed'
$reservedColumn = Get-MetricColumn $oxideRows 'Outstanding Reserved'
$outstandingCommittedBytes = ($oxideRows | ForEach-Object {
    Convert-WpaNumber ([string]$_.$committedColumn)
} | Measure-Object -Sum).Sum
$outstandingReservedBytes = ($oxideRows | ForEach-Object {
    Convert-WpaNumber ([string]$_.$reservedColumn)
} | Measure-Object -Sum).Sum
$allocationCount = ($oxideRows | ForEach-Object {
    Convert-WpaNumber ([string]$_.Count)
} | Measure-Object -Sum).Sum
$shallowStackDepth = 3
$shallowStackRows = @($stackRows | Where-Object {
    (Get-StackDepth ([string]$_.Stack)) -eq $shallowStackDepth
})
if ($shallowStackRows.Count -eq 0) {
    throw "WPA Exporter не вернул Stack-ветви глубины $shallowStackDepth."
}

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-wpa-virtual-allocation-summary'
    trace_file = [System.IO.Path]::GetFileName($trace)
    git_commit = [string]$metadata.git_commit
    package_version = [string]$metadata.package_version
    process_id = [int]$metadata.process_id
    scenario = [string]$metadata.scenario
    aggregation = 'core4-virtual-allocation'
    requested_duration_seconds = [int]$metadata.requested_duration_seconds
    outstanding_committed_mib = [math]::Round($outstandingCommittedBytes / 1MB, 4)
    outstanding_reserved_mib = [math]::Round($outstandingReservedBytes / 1MB, 4)
    allocation_count = [long]$allocationCount
    by_commit_type = @(Measure-Dimension $commitTypeRows 'Commit Type')
    by_reserve_type = @(Measure-Dimension $reserveTypeRows 'Reserve Type')
    stack_semantics = 'inclusive; stack rows and landmarks overlap and must not be summed'
    stack_row_count = $stackRows.Count
    shallow_stack_depth = $shallowStackDepth
    shallow_stack_branches = @(Measure-Dimension $shallowStackRows 'Stack')
    app_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $stackRows 'Frame' 'oxide.exe!oxide::*' |
            Select-Object -First 30
    )
    module_stack_landmarks = @(
        Measure-InclusiveStackLandmarks $stackRows 'Module' |
            Select-Object -First 30
    )
    symbols_requested = [bool]$Symbols
}
$summaryPath = Join-Path $output 'oxide-virtual-allocation-summary.json'
$summary | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $summaryPath -Encoding utf8 -ErrorAction Stop

Write-Host "VirtualAllocation-анализ Oxide: $output"
Write-Host (
    'Outstanding committed: {0} МиБ; reserved: {1} МиБ; событий: {2}.' -f
        $summary.outstanding_committed_mib,
        $summary.outstanding_reserved_mib,
        $summary.allocation_count
)
Write-Host 'Reserved — адресное пространство, а не занятая ОЗУ.'
Write-Host 'Крупнейшие непересекающиеся Stack-ветви на глубине 3:'
$summary.shallow_stack_branches | Select-Object -First 15 `
    @{ Name = 'frame'; Expression = { Get-StackFrame $_.name } }, `
    allocation_count,
    outstanding_committed_mib,
    outstanding_reserved_mib |
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
    maximum_inclusive_committed_mib,
    reserved_mib_at_maximum |
    Format-Table -AutoSize
Write-Host "Сводка: $summaryPath"
