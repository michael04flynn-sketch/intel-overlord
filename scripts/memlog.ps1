# Memory timeline for one run: every -IntervalMs it appends a line to -Out with the PC's free
# RAM and free commit and the working set / private bytes of victory.exe. Started hidden by
# play-intel-lowmem.ps1; stops when victory.exe has exited (or after -MaxSeconds).
# Why: on an iGPU the card's memory is the PC's memory, and the first world frame takes several
# GB in a few seconds; the run log alone does not show where the PC's RAM went.
param(
    [Parameter(Mandatory)][string]$Out,
    [int]$IntervalMs = 1000,
    [int]$MaxSeconds = 1800,
    # Only a victory.exe under this folder counts (the repository's target\, so another victory.exe on
    # the PC, e.g. somebody's test binary, is not sampled instead of the game). Empty: any.
    [string]$Under = ''
)
$ErrorActionPreference = 'SilentlyContinue'
$started = Get-Date
"time,free_ram_mb,free_commit_mb,victory_ws_mb,victory_private_mb,top_other" | Out-File $Out -Encoding ascii
$seen = $false
while (((Get-Date) - $started).TotalSeconds -lt $MaxSeconds) {
    $p = Get-Process -Name victory |
        Where-Object { -not $Under -or ($_.Path -and $_.Path.StartsWith($Under, [StringComparison]::OrdinalIgnoreCase)) } |
        Sort-Object StartTime -Descending | Select-Object -First 1
    if ($p) { $seen = $true } elseif ($seen) { break }
    $os = Get-CimInstance Win32_OperatingSystem
    $top = Get-Process | Where-Object { $_.Name -ne 'victory' } | Sort-Object WorkingSet64 -Descending |
        Select-Object -First 1
    $line = '{0:HH:mm:ss.f},{1},{2},{3},{4},{5}' -f (Get-Date),
        [math]::Round($os.FreePhysicalMemory / 1KB), [math]::Round($os.FreeVirtualMemory / 1KB),
        $(if ($p) { [math]::Round($p.WorkingSet64 / 1MB) } else { '' }),
        $(if ($p) { [math]::Round($p.PrivateMemorySize64 / 1MB) } else { '' }),
        $(if ($top) { "$($top.Name) $([math]::Round($top.WorkingSet64 / 1MB))" } else { '' })
    Add-Content -Path $Out -Value $line -Encoding ascii
    Start-Sleep -Milliseconds $IntervalMs
}
