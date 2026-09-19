[CmdletBinding()]
param(
    [string]$TracePath
)

$ErrorActionPreference = 'Stop'
Set-StrictMode -Version Latest

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

function Set-AttributeValue {
    param(
        [Parameter(Mandatory = $true)][System.Xml.XmlElement]$Element,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$Value
    )

    $Element.SetAttribute($Name, $Value)
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
                $candidate.profile -in @('ResidentSet', 'ReferenceSet')
        } |
        Select-Object -First 1
    if ($null -eq $passport) {
        throw 'Паспорт ResidentSet/ReferenceSet-трассы не найден.'
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
if ($metadata.kind -ne 'oxide-wpr-trace' -or $metadata.schema -ne 1) {
    throw "Неизвестный формат паспорта: $($passport.FullName)"
}
if ($metadata.profile -notin @('ResidentSet', 'ReferenceSet')) {
    throw "Для memory-экспорта нужна ResidentSet/ReferenceSet-трасса, получена $($metadata.profile)."
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
if ($null -eq $wpaPackage -or [string]::IsNullOrWhiteSpace($wpaPackage.InstallLocation)) {
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
    'CustomDataSources\Event_Tracing_for_Windows\Catalog\FullBoot.Boot.wpaProfile'
if (-not (Test-Path -LiteralPath $sourceProfile -PathType Leaf)) {
    throw "Стандартный WPA-профиль-шаблон не найден: $sourceProfile"
}

$traceBaseName = [System.IO.Path]::GetFileNameWithoutExtension($trace)
$analysisRoot = Join-Path $traceRoot 'analysis'
$output = Join-Path $analysisRoot `
    "$traceBaseName-memory-$([DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss'))"
New-Item -ItemType Directory -Path $output -ErrorAction Stop | Out-Null
$derivedProfile = Join-Path $output 'oxide-resident-memory.wpaProfile'

[xml]$profile = Get-Content -LiteralPath $sourceProfile -Raw -ErrorAction Stop
$templateGraph = $profile.SelectSingleNode(
    '//*[local-name()="Core4SummaryTableFillerGraphTreeItem"]'
)
if ($null -eq $templateGraph) {
    throw 'Стандартный WPA-профиль не содержит Core4 graph-шаблон.'
}
$view = $templateGraph.ParentNode.CloneNode($true)
$graph = $view.SelectSingleNode(
    './*[local-name()="Core4SummaryTableFillerGraphTreeItem"]'
)
Set-AttributeValue $graph 'ClsID' '00C2282D-6788-482D-8528-36C079EABBD5'
Set-AttributeValue $graph 'LayoutStyleFree' 'All'
Set-AttributeValue $graph 'Color' '#FFCB6D3D'
$graph.RemoveAttribute('GraphHeight')
$graph.RemoveAttribute('DataTableHeight')

$presetContainer = $graph.SelectSingleNode(
    './*[local-name()="Core4SummaryTableFillerGraphTreeItem.HdvViewModelPreset"]'
)
if ($null -eq $presetContainer) {
    throw 'Core4 graph-шаблон не содержит контейнер preset.'
}
$presetContainer.RemoveAll()
[xml]$residentPresetDocument = @'
<HdvViewModelPreset
    xmlns="clr-namespace:Microsoft.Performance.UI.DataEngine.ViewModelPresets;assembly=Microsoft.Performance.UI.DataEngine"
    Name="Oxide Resident Memory by Category"
    GraphChartType="StackedBars"
    GraphColumnCount="6"
    KeyColumnCount="4"
    RightFrozenColumnCount="5"
    InitialFilterQuery=""
    InitialSelectionQuery=""
    GraphFilterColumnName="PageCount"
    GraphFilterColumnGuid="6e63a987-cd12-5a9a-5e70-9088e2257c22">
  <HdvViewModelPreset.ColumnMetadataEntries />
  <HdvViewModelPreset.HighlightEntries />
  <HdvViewModelPreset.ConfigurableColumns>
    <HdvColumnViewModelPreset Guid="60269832-0d29-5f0f-a9b2-c65ee9d0b1cf" Name="Process" IsVisible="True" SortPriority="1" Width="180" />
    <HdvColumnViewModelPreset Guid="15a3f76b-6dc3-5d57-4b45-39f86b2c4314" Name="MMList" IsVisible="True" SortPriority="2" Width="100" />
    <HdvColumnViewModelPreset Guid="681c68ba-3371-59d2-d155-641aa268d2a9" Name="Page Category" IsVisible="True" SortPriority="3" Width="160" />
    <HdvColumnViewModelPreset Guid="c7de38ec-9f55-55ef-e8f9-8a031772672e" Name="Description" IsVisible="True" SortPriority="4" Width="280" />
    <HdvColumnViewModelPreset Guid="5af4cb99-abdf-4ced-b092-440160c940c2" Name="Size" AggregationMode="Sum" IsVisible="True" SortOrder="Descending" SortPriority="5" TextAlignment="Right" Width="100" />
    <HdvColumnViewModelPreset Guid="6e63a987-cd12-5a9a-5e70-9088e2257c22" Name="Page Count" AggregationMode="Count" IsVisible="True" SortOrder="Descending" SortPriority="6" TextAlignment="Right" Width="100" />
  </HdvViewModelPreset.ConfigurableColumns>
</HdvViewModelPreset>
'@
$preset = $profile.ImportNode($residentPresetDocument.DocumentElement, $true)
[void]$presetContainer.AppendChild($preset)

function New-ResidentGraph {
    param(
        [Parameter(Mandatory = $true)][System.Xml.XmlElement]$BaseGraph,
        [Parameter(Mandatory = $true)][string]$Name,
        [Parameter(Mandatory = $true)][string]$GroupColumn,
        [Parameter(Mandatory = $true)][string]$ProcessLabel,
        [Parameter(Mandatory = $true)][System.Xml.XmlDocument]$OwnerDocument
    )

    $clone = $BaseGraph.CloneNode($true)
    $clonePreset = $clone.SelectSingleNode(
        './/*[local-name()="HdvViewModelPreset"]'
    )
    Set-AttributeValue $clonePreset 'Name' $Name
    # The Resident Set provider has four key columns even when a flat export
    # displays only one of them. Reducing KeyColumnCount makes WPA emit one row
    # per 4 KiB page instead of aggregating the selected dimension.
    Set-AttributeValue $clonePreset 'KeyColumnCount' '4'
    Set-AttributeValue $clonePreset 'GraphColumnCount' '6'
    Set-AttributeValue $clonePreset 'RightFrozenColumnCount' '5'
    if ($GroupColumn -eq 'Process') {
        $clonePreset.SetAttribute('InitialFilterQuery', '')
    }
    else {
        Set-AttributeValue $clonePreset 'InitialFilterQuery' `
            "[Process]:=`"$ProcessLabel`""
        Set-AttributeValue $clonePreset 'InitialFilterShouldKeep' 'true'
    }

    $columns = @($clonePreset.SelectNodes(
        './/*[local-name()="HdvColumnViewModelPreset"]'
    ))
    foreach ($column in $columns) {
        $column.RemoveAttribute('IsVisible')
        $columnName = $column.GetAttribute('Name')
        if ($columnName -eq $GroupColumn) {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '1'
        }
        elseif ($columnName -eq 'Size') {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '2'
        }
        elseif ($columnName -eq 'Page Count') {
            Set-AttributeValue $column 'IsVisible' 'true'
            Set-AttributeValue $column 'SortPriority' '3'
        }
        else {
            Set-AttributeValue $column 'SortPriority' '4'
        }
    }
    return $OwnerDocument.ImportNode($clone, $true)
}

$processLabel = "oxide.exe ($([int]$metadata.process_id))"
$generatedGraphs = @(
    New-ResidentGraph $graph 'Oxide Resident Total by Process' `
        'Process' $processLabel $profile
    New-ResidentGraph $graph 'Oxide Resident by Memory List' `
        'MMList' $processLabel $profile
    New-ResidentGraph $graph 'Oxide Resident by Page Category' `
        'Page Category' $processLabel $profile
    New-ResidentGraph $graph 'Oxide Resident by Description' `
        'Description' $processLabel $profile
)
@($view.ChildNodes) | Where-Object {
    $_.LocalName -notin @('AnalysisView.Title', 'AnalysisView.UISessionRef')
} | ForEach-Object { [void]$view.RemoveChild($_) }
foreach ($generatedGraph in $generatedGraphs) {
    [void]$view.AppendChild($generatedGraph)
}

$views = $profile.SelectSingleNode('//*[local-name()="WpaProfile.Views"]')
if ($null -eq $views) {
    throw 'Стандартный профиль WPA не содержит коллекцию представлений.'
}
$views.RemoveAll()
[void]$views.AppendChild($view)
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
    '-delimiter', ';',
    '-symcacheonly'
)
$exportOutput = (& $wpaExporter.Source @arguments 2>&1 | Out-String).Trim()
if ($LASTEXITCODE -ne 0 -or
    $exportOutput -match '(?i)filter query.+invalid|error exporting profile') {
    throw "WPA Exporter не смог построить Resident Set-срез: $exportOutput"
}

$tables = @(Get-ChildItem -LiteralPath $output -Filter '*.csv' -File -ErrorAction Stop)
if ($tables.Count -ne 4) {
    throw "Ожидалось четыре memory-таблицы, получено $($tables.Count)."
}

function Get-ResidentRows {
    param(
        [Parameter(Mandatory = $true)][System.IO.FileInfo[]]$Tables,
        [Parameter(Mandatory = $true)][string]$NamePattern
    )

    $table = $Tables | Where-Object Name -Like $NamePattern | Select-Object -First 1
    if ($null -eq $table) {
        throw "Не найдена memory-таблица: $NamePattern"
    }
    $rows = @(Import-Csv -LiteralPath $table.FullName -Delimiter ';')
    if ($rows.Count -eq 0) {
        throw "WPA Exporter вернул пустую таблицу: $($table.Name)"
    }
    return $rows
}

$processRows = @(Get-ResidentRows $tables '*_Oxide_Resident_Total_by_Process.csv')
$memoryListRows = @(Get-ResidentRows $tables '*_Oxide_Resident_by_Memory_List.csv')
$pageCategoryRows = @(Get-ResidentRows $tables '*_Oxide_Resident_by_Page_Category.csv')
$descriptionRows = @(Get-ResidentRows $tables '*_Oxide_Resident_by_Description.csv')

function Convert-ResidentDimension {
    param(
        [Parameter(Mandatory = $true)][object[]]$Rows,
        [Parameter(Mandatory = $true)][string]$Dimension
    )

    $headers = @($Rows[0].PSObject.Properties.Name)
    $sizeColumn = $headers | Where-Object { $_ -like 'Size*' } | Select-Object -First 1
    if ($Dimension -notin $headers -or
        'Page Count' -notin $headers -or
        [string]::IsNullOrWhiteSpace($sizeColumn)) {
        throw "Некорректные столбцы Resident Set для измерения '$Dimension'."
    }
    return @($Rows | Group-Object $Dimension | ForEach-Object {
        $first = $_.Group | Select-Object -First 1
        $sizeBytes = ($_.Group | ForEach-Object {
            Convert-WpaNumber ([string]$_.$sizeColumn)
        } | Measure-Object -Sum).Sum
        $pageCount = ($_.Group | ForEach-Object {
            Convert-WpaNumber ([string]$_.'Page Count')
        } | Measure-Object -Sum).Sum
        [pscustomobject][ordered]@{
            name = [string]$first.$Dimension
            page_count = [long]$pageCount
            resident_mib = [math]::Round($sizeBytes / 1MB, 4)
        }
    } | Where-Object { -not [string]::IsNullOrWhiteSpace($_.name) } |
        Sort-Object resident_mib -Descending)
}

$processHeaders = @($processRows[0].PSObject.Properties.Name)
$processSizeColumn = $processHeaders | Where-Object { $_ -like 'Size*' } |
    Select-Object -First 1
$oxideRows = @($processRows | Where-Object { $_.Process -eq $processLabel })
if ($oxideRows.Count -eq 0) {
    $matchingProcesses = @($processRows.Process | Where-Object {
        $_ -match '(?i)^oxide(?:\.exe)?\b'
    } | Sort-Object -Unique)
    throw (
        "Resident Set-срез не содержит точный процесс '$processLabel'. " +
        "Похожие подписи: $($matchingProcesses -join ', ')"
    )
}
if ([string]::IsNullOrWhiteSpace($processSizeColumn) -or
    'Page Count' -notin $processHeaders) {
    throw 'Некорректные столбцы итоговой Resident Set-таблицы.'
}
$residentBytes = ($oxideRows | ForEach-Object {
    Convert-WpaNumber ([string]$_.$processSizeColumn)
} | Measure-Object -Sum).Sum
$pageCount = ($oxideRows | ForEach-Object {
    Convert-WpaNumber ([string]$_.'Page Count')
} | Measure-Object -Sum).Sum
$memoryLists = @(Convert-ResidentDimension $memoryListRows 'MMList')
$pageCategories = @(Convert-ResidentDimension $pageCategoryRows 'Page Category')
$descriptions = @(Convert-ResidentDimension $descriptionRows 'Description')

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-wpa-resident-memory-summary'
    trace_file = [System.IO.Path]::GetFileName($trace)
    git_commit = [string]$metadata.git_commit
    package_version = [string]$metadata.package_version
    process_id = [int]$metadata.process_id
    scenario = [string]$metadata.scenario
    wpr_profile = [string]$metadata.profile
    snapshot_kind = 'end-of-trace-resident-set'
    requested_duration_seconds = [int]$metadata.requested_duration_seconds
    page_count = [long]$pageCount
    resident_mib = [math]::Round($residentBytes / 1MB, 4)
    memory_lists = $memoryLists
    page_categories = $pageCategories
    descriptions = $descriptions
}
$summaryPath = Join-Path $output 'oxide-resident-memory-summary.json'
$summary | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $summaryPath -Encoding utf8 -ErrorAction Stop

Write-Host "Resident Set-анализ Oxide: $output"
Write-Host (
    'Resident Set: {0} МиБ; страниц: {1}.' -f
        $summary.resident_mib,
        $summary.page_count
)
Write-Host 'Крупнейшие категории страниц:'
$pageCategories | Select-Object -First 12 | Format-Table -AutoSize
Write-Host 'Крупнейшие описания:'
$descriptions | Select-Object -First 12 | Format-Table -AutoSize
Write-Host "Сводка: $summaryPath"
