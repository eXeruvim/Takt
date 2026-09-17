param(
    [string]$TracePath,
    [switch]$Symbols
)

$ErrorActionPreference = 'Stop'

function Set-AttributeValue {
    param(
        [System.Xml.XmlElement]$Element,
        [string]$Name,
        [string]$Value
    )

    $Element.SetAttribute($Name, $Value)
}

function Convert-WpaNumber {
    param([string]$Value)

    return [double]::Parse(
        $Value,
        [System.Globalization.NumberStyles]::Float,
        [System.Globalization.CultureInfo]::CurrentCulture
    )
}

function Measure-SampledTable {
    param(
        [string]$Path,
        [string]$Key
    )

    return @(Import-Csv -LiteralPath $Path -Delimiter ';' | Group-Object $Key |
        ForEach-Object {
            [pscustomobject][ordered]@{
                name = $_.Name
                samples = [int](($_.Group | Measure-Object -Property Count -Sum).Sum)
                cpu_ms = [math]::Round(($_.Group | ForEach-Object {
                    Convert-WpaNumber $_.'Weight (in view) (ms)'
                } | Measure-Object -Sum).Sum, 4)
            }
        } | Sort-Object cpu_ms -Descending)
}

$repository = [System.IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..'))
$traceRoot = Join-Path $repository 'benchmark-results\wpr'

if ([string]::IsNullOrWhiteSpace($TracePath)) {
    $passport = Get-ChildItem -LiteralPath $traceRoot -Filter '*.json' -File -ErrorAction Stop |
        Sort-Object LastWriteTime -Descending |
        Select-Object -First 1
    if ($null -eq $passport) {
        throw 'Паспорт WPR-трассы не найден. Сначала выполните capture-oxide-wpr.ps1.'
    }
    $metadata = Get-Content -LiteralPath $passport.FullName -Raw -ErrorAction Stop |
        ConvertFrom-Json -ErrorAction Stop
    if ($metadata.kind -ne 'oxide-wpr-trace' -or $metadata.schema -ne 1) {
        throw "Неизвестный формат паспорта: $($passport.FullName)"
    }
    $trace = Join-Path $passport.DirectoryName ([string]$metadata.trace_file)
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
    $metadata = Get-Content -LiteralPath $passport.FullName -Raw -ErrorAction Stop |
        ConvertFrom-Json -ErrorAction Stop
}

$trace = [System.IO.Path]::GetFullPath($trace)
if (-not $trace.StartsWith($traceRoot + [System.IO.Path]::DirectorySeparatorChar, [System.StringComparison]::OrdinalIgnoreCase)) {
    throw "Трасса должна находиться внутри $traceRoot"
}
if (-not (Test-Path -LiteralPath $trace -PathType Leaf)) {
    throw "ETL-трасса не найдена: $trace"
}
if ([int]$metadata.process_id -le 0) {
    throw 'В паспорте отсутствует корректный PID Oxide.'
}

$wpaExporter = Get-Command wpaexporter.exe -ErrorAction SilentlyContinue
if ($null -eq $wpaExporter) {
    throw 'WPA Exporter не найден. Установите Windows Performance Analyzer.'
}
$wpaPackage = Get-AppxPackage -Name Microsoft.WindowsPerformanceAnalyzer -ErrorAction SilentlyContinue |
    Sort-Object Version -Descending |
    Select-Object -First 1
if ($null -eq $wpaPackage -or [string]::IsNullOrWhiteSpace($wpaPackage.InstallLocation)) {
    throw 'Не удалось определить каталог установленного Windows Performance Analyzer.'
}
$sourceProfile = Join-Path $wpaPackage.InstallLocation `
    'CustomDataSources\Event_Tracing_for_Windows\Catalog\AppLaunch.wpaProfile'
if (-not (Test-Path -LiteralPath $sourceProfile -PathType Leaf)) {
    throw "Стандартный CPU-профиль WPA не найден: $sourceProfile"
}

$traceBaseName = [System.IO.Path]::GetFileNameWithoutExtension($trace)
$analysisRoot = Join-Path $traceRoot 'analysis'
$output = Join-Path $analysisRoot "$traceBaseName-$([DateTimeOffset]::Now.ToString('yyyyMMdd-HHmmss'))"
New-Item -ItemType Directory -Path $output -ErrorAction Stop | Out-Null
$derivedProfile = Join-Path $output 'oxide-cpu.wpaProfile'

[xml]$profile = Get-Content -LiteralPath $sourceProfile -Raw -ErrorAction Stop
$namespace = $profile.DocumentElement.NamespaceURI
$manager = [System.Xml.XmlNamespaceManager]::new($profile.NameTable)
$manager.AddNamespace('w', $namespace)
$cpuGraph = $profile.SelectSingleNode(
    '//w:Views/w:View/w:Graphs/w:Graph[@Guid="b855361e-7be0-4bc8-a754-3e8507715ca5"]',
    $manager
)
if ($null -eq $cpuGraph) {
    throw 'Стандартный профиль WPA не содержит CPU Usage (Sampled).'
}
$preciseGraph = $profile.SelectSingleNode(
    '//w:Views/w:View/w:Graphs/w:Graph[@Guid="c58f5fea-0319-4046-932d-e695ebe20b47"]',
    $manager
)
if ($null -eq $preciseGraph) {
    throw 'Стандартный профиль WPA не содержит CPU Usage (Precise).'
}
$graphs = $cpuGraph.ParentNode
$cpuView = $graphs.ParentNode

$templates = @(
    @{ Name = 'Oxide by Module'; Column = 'Module' },
    @{ Name = 'Oxide by Function'; Column = 'Function' },
    @{ Name = 'Oxide by Thread'; Column = 'Thread ID' }
)
$generatedGraphs = [System.Collections.Generic.List[System.Xml.XmlNode]]::new()
foreach ($template in $templates) {
    $graph = $cpuGraph.CloneNode($true)
    $preset = $graph.SelectSingleNode('w:Preset', $manager)
    Set-AttributeValue $preset 'Name' $template.Name
    Set-AttributeValue $preset 'KeyColumnCount' '1'
    Set-AttributeValue $preset 'InitialFilterQuery' `
        "[Process]:=`"oxide.exe ($([int]$metadata.process_id))`""
    Set-AttributeValue $preset 'InitialFilterShouldKeep' 'true'
    Set-AttributeValue $preset 'InitialSelectionQuery' `
        "([Series Name]:=`"$($template.Column)`")"
    $preset.RemoveAttribute('InitialExpansionQuery')

    $columns = @($preset.SelectNodes('w:Columns/w:Column', $manager))
    $key = $columns | Where-Object { $_.GetAttribute('Name') -eq $template.Column } |
        Select-Object -First 1
    if ($null -eq $key) {
        throw "В CPU-профиле WPA отсутствует столбец $($template.Column)."
    }
    $columnContainer = $preset.SelectSingleNode('w:Columns', $manager)
    [void]$columnContainer.RemoveChild($key)
    [void]$columnContainer.PrependChild($key)
    $ordered = @($preset.SelectNodes('w:Columns/w:Column', $manager))
    for ($index = 0; $index -lt $ordered.Count; $index++) {
        $priority = if ($ordered[$index].GetAttribute('Name') -eq '% Weight') {
            0
        }
        else {
            $index + 1
        }
        Set-AttributeValue $ordered[$index] 'SortPriority' ([string]$priority)
        $isMetric = $ordered[$index].GetAttribute('Name') -in @('Count', 'Weight (in view)', '% Weight')
        Set-AttributeValue $ordered[$index] 'IsVisible' `
            $(if ($index -eq 0 -or $isMetric) { 'true' } else { 'false' })
    }
    $generatedGraphs.Add($graph)
}

$contextGraph = $preciseGraph.CloneNode($true)
$contextPreset = $contextGraph.SelectSingleNode('w:Preset', $manager)
Set-AttributeValue $contextPreset 'Name' 'Oxide Context Switches by Thread'
Set-AttributeValue $contextPreset 'KeyColumnCount' '1'
Set-AttributeValue $contextPreset 'InitialFilterQuery' `
    "[New Process]:=`"oxide.exe ($([int]$metadata.process_id))`""
Set-AttributeValue $contextPreset 'InitialFilterShouldKeep' 'true'
Set-AttributeValue $contextPreset 'InitialSelectionQuery' `
    '([Series Name]:="New Thread Id")'
$contextPreset.RemoveAttribute('InitialExpansionQuery')
$contextColumns = @($contextPreset.SelectNodes('w:Columns/w:Column', $manager))
$threadKey = $contextColumns | Where-Object { $_.GetAttribute('Name') -eq 'New Thread Id' } |
    Select-Object -First 1
if ($null -eq $threadKey) {
    throw 'В CPU Usage (Precise) отсутствует столбец New Thread Id.'
}
$contextColumnContainer = $contextPreset.SelectSingleNode('w:Columns', $manager)
[void]$contextColumnContainer.RemoveChild($threadKey)
[void]$contextColumnContainer.PrependChild($threadKey)
$orderedContextColumns = @($contextPreset.SelectNodes('w:Columns/w:Column', $manager))
for ($index = 0; $index -lt $orderedContextColumns.Count; $index++) {
    $columnName = $orderedContextColumns[$index].GetAttribute('Name')
    $priority = if ($columnName -eq '% CPU Usage') { 0 } else { $index + 1 }
    Set-AttributeValue $orderedContextColumns[$index] 'SortPriority' ([string]$priority)
    $isMetric = $columnName -in @(
        'New Thread Start Module',
        'New Thread Start Function',
        'Readying Process',
        'Readying Thread Id',
        'Ready Thread Stack',
        'Count',
        'Ready',
        'Waits',
        'Count:Waits',
        'CPU Usage (in view)',
        '% CPU Usage'
    )
    Set-AttributeValue $orderedContextColumns[$index] 'IsVisible' `
        $(if ($index -eq 0 -or $isMetric) { 'true' } else { 'false' })
}
$generatedGraphs.Add($contextGraph)

$graphs.RemoveAll()
foreach ($graph in $generatedGraphs) {
    [void]$graphs.AppendChild($graph)
}
$views = $profile.SelectSingleNode('//w:Views', $manager)
$views.RemoveAll()
[void]$views.AppendChild($cpuView)
$modifiedGraphs = $profile.SelectSingleNode('//w:ModifiedGraphs', $manager)
if ($null -ne $modifiedGraphs) {
    $modifiedGraphs.RemoveAll()
}
$profile.SelectNodes('//w:FileReferences', $manager) | ForEach-Object { $_.RemoveAll() }

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
if ($Symbols) {
    $arguments += '-symbols'
}
$originalSymbolPath = $env:_NT_SYMBOL_PATH
try {
    if ($Symbols) {
        # Resolve only this exact profiling build. Deliberately omit public
        # symbol servers so analysis never starts an implicit network download.
        $env:_NT_SYMBOL_PATH = Join-Path $repository 'target\profiling'
    }
    else {
        $arguments += '-symcacheonly'
    }
    $exportOutput = (& $wpaExporter.Source @arguments 2>&1 | Out-String).Trim()
}
finally {
    $env:_NT_SYMBOL_PATH = $originalSymbolPath
}
if ($LASTEXITCODE -ne 0 -or $exportOutput -match '(?i)filter query.+invalid|error exporting profile') {
    throw "WPA Exporter не смог построить CPU-срезы: $exportOutput"
}

$tables = Get-ChildItem -LiteralPath $output -Filter '*.csv' -File -ErrorAction Stop
$expectedTableCount = $templates.Count + 1
if ($tables.Count -ne $expectedTableCount) {
    throw "Ожидалось $expectedTableCount CPU-таблицы, получено $($tables.Count)."
}

$moduleTable = $tables | Where-Object Name -Like '*_Oxide_by_Module.csv'
$functionTable = $tables | Where-Object Name -Like '*_Oxide_by_Function.csv'
$threadTable = $tables | Where-Object Name -Like '*_Oxide_by_Thread.csv'
$preciseTable = $tables | Where-Object Name -Like '*_Oxide_Context_Switches_by_Thread.csv'
$sampledByModule = Measure-SampledTable $moduleTable.FullName 'Module'
$sampledByFunction = Measure-SampledTable $functionTable.FullName 'Function'
$sampledByThread = Measure-SampledTable $threadTable.FullName 'Thread ID'

$preciseHeaders = @(
    'ThreadId', 'ReadyStack', 'ReadyingProcess', 'ReadyingThreadId', 'Count',
    'ReadySumUs', 'ReadyMaxUs', 'WaitsSumUs', 'WaitsMaxUs', 'CountWaits',
    'StartModule', 'StartFunction', 'CpuMs', 'Percent'
)
$preciseRows = @(Get-Content -LiteralPath $preciseTable.FullName |
    Select-Object -Skip 1 |
    ConvertFrom-Csv -Delimiter ';' -Header $preciseHeaders)
$contextByThread = @($preciseRows | Group-Object ThreadId | ForEach-Object {
    $first = $_.Group | Select-Object -First 1
    [pscustomobject][ordered]@{
        thread_id = [int]$first.ThreadId
        start_module = $first.StartModule
        start_function = $first.StartFunction
        switch_ins = [int](($_.Group | Measure-Object -Property Count -Sum).Sum)
        cpu_ms = [math]::Round(($_.Group | ForEach-Object {
            Convert-WpaNumber $_.CpuMs
        } | Measure-Object -Sum).Sum, 4)
        ready_total_us = [math]::Round(($_.Group | ForEach-Object {
            Convert-WpaNumber $_.ReadySumUs
        } | Measure-Object -Sum).Sum, 2)
        ready_max_us = [math]::Round(($_.Group | ForEach-Object {
            Convert-WpaNumber $_.ReadyMaxUs
        } | Measure-Object -Maximum).Maximum, 2)
    }
} | Sort-Object switch_ins -Descending)

$busiestThreadId = $contextByThread[0].thread_id
$topWakers = @($preciseRows | Where-Object { [int]$_.ThreadId -eq $busiestThreadId } |
    Group-Object ReadyingProcess | ForEach-Object {
        [pscustomobject][ordered]@{
            process = $_.Name
            wakeups = $_.Count
            cpu_ms_after_wake = [math]::Round(($_.Group | ForEach-Object {
                Convert-WpaNumber $_.CpuMs
            } | Measure-Object -Sum).Sum, 4)
        }
    } | Sort-Object wakeups -Descending | Select-Object -First 20)

$summary = [ordered]@{
    schema = 1
    kind = 'oxide-wpa-cpu-summary'
    trace_file = [System.IO.Path]::GetFileName($trace)
    git_commit = [string]$metadata.git_commit
    package_version = [string]$metadata.package_version
    process_id = [int]$metadata.process_id
    requested_duration_seconds = [int]$metadata.requested_duration_seconds
    sampled_cpu_ms = [math]::Round(($sampledByModule |
        Measure-Object -Property cpu_ms -Sum).Sum, 4)
    sampled_by_module = $sampledByModule
    sampled_by_function = $sampledByFunction
    sampled_by_thread = $sampledByThread
    context_switches = $preciseRows.Count
    context_switches_by_thread = $contextByThread
    busiest_thread_top_wakers = $topWakers
}
$summaryPath = Join-Path $output 'oxide-cpu-summary.json'
$summary | ConvertTo-Json -Depth 8 |
    Set-Content -LiteralPath $summaryPath -Encoding utf8 -ErrorAction Stop

Write-Host "CPU-анализ Oxide: $output"
Write-Host "Sampled CPU: $($summary.sampled_cpu_ms) мс; context switches: $($summary.context_switches)."
$tables | Sort-Object Name | Select-Object Name, Length, LastWriteTime
