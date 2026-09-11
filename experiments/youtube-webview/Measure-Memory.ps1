param(
    [Parameter(Mandatory = $true)][int]$RootProcessId,
    [ValidateRange(1, 3600)][int]$Seconds = 30,
    [Parameter(Mandatory = $true)][string]$OutputPath
)

$ErrorActionPreference = 'Stop'
$known = @{}
$rows = [System.Collections.Generic.List[object]]::new()
$clock = [System.Diagnostics.Stopwatch]::StartNew()
$initial = Get-CimInstance Win32_Process -Filter "ProcessId = $RootProcessId"
if (-not $initial) { throw 'The player process is not running' }
$known[$RootProcessId] = $initial.CreationDate

while ($clock.Elapsed.TotalSeconds -lt $Seconds) {
    $processes = @(Get-CimInstance Win32_Process)
    $live = @{}
    foreach ($process in $processes) {
        $id = [int]$process.ProcessId
        if ($known.ContainsKey($id) -and $known[$id] -eq $process.CreationDate) {
            $live[$id] = $process
        }
    }
    do {
        $added = $false
        foreach ($process in $processes) {
            $id = [int]$process.ProcessId
            $parentId = [int]$process.ParentProcessId
            if (-not $live.ContainsKey($id) -and $live.ContainsKey($parentId) -and
                $process.CreationDate -ge $live[$parentId].CreationDate) {
                $live[$id] = $process
                $known[$id] = $process.CreationDate
                $added = $true
            }
        }
    } while ($added)
    $counters = @(Get-CimInstance Win32_PerfFormattedData_PerfProc_Process |
        Where-Object { $live.ContainsKey([int]$_.IDProcess) })
    $rows.Add([pscustomobject]@{
        Seconds = [math]::Round($clock.Elapsed.TotalSeconds, 2)
        Processes = $live.Count
        CountedProcesses = $counters.Count
        PrivateWorkingSetMiB = [math]::Round(($counters | Measure-Object WorkingSetPrivate -Sum).Sum / 1MB, 2)
        PrivateCommitMiB = [math]::Round(($counters | Measure-Object PrivateBytes -Sum).Sum / 1MB, 2)
        SummedWorkingSetMiB = [math]::Round(($counters | Measure-Object WorkingSet -Sum).Sum / 1MB, 2)
    })
    Start-Sleep -Milliseconds 1000
}
$rows | Export-Csv -LiteralPath $OutputPath -NoTypeInformation
$rows | Format-Table -AutoSize
