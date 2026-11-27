# Runs the mesh_fuzz load test (crates\victory-gpu\tests\mesh_fuzz) once per setting and says which settings
# stop the card. No game is started. A run that stops the card makes Windows reset the GPU: the screen can
# flicker for a second.
#   scripts\intel-fuzz.ps1 -Dxil <the translated shader, from victory.exe translate --list --validate with VICTORY_EMIT_DUMP>
#   -Only a,b       run just those     -List   show the settings and stop     -Seconds n   how long each runs
#   -DxilDir <folder>   the folder of translated mesh programs (VICTORY_EMIT_DUMP of translate --list, the *_Ms.dxil
#                   files) for the settings that run many of the title's programs ('shaders')
# The translated shader (the cascade shadow program 45ace4127ac61cfd):
#   $env:VICTORY_EMIT_DUMP = '<a folder outside the repo>'; $env:VICTORY_TRANSLATE_ONLY = '45ace4127ac61cfd'
#   target\diag\victory.exe translate --validate --list <game folder>\shaders.rpf
# Each run's output is kept in logs\ (fuzz-<time>-<name>.txt) and the table in logs\fuzz-<time>.txt.
param(
    [string]$Dxil,
    [string]$DxilDir,
    [int]$Seconds = 120,
    [int]$PauseSeconds = 3,
    # How long to wait for a game run that is on to end before starting (0: stop at once).
    [int]$WaitMinutes = 15,
    # How long the placeholder process (named victory.exe, so that no game run starts between two settings) may live.
    [int]$HoldMinutes = 40,
    [string[]]$Only,
    [switch]$List
)

# name -> the settings (environment variables of the test: see the top of load.rs). In the order they add up:
# each of the first ones adds one piece to the one before.
$groups = '16'
$targets = @{ MESH_DEPTH = 'd32s8'; MESH_RTS = '4'; MESH_PS = '1'; MESH_CLEAR = '1'; MESH_COPYBACK = '2' }
function Merge-Settings([hashtable[]]$parts) { $m = @{}; foreach ($p in $parts) { foreach ($k in $p.Keys) { $m[$k] = $p[$k] } }; $m }
$experiments = [ordered]@{
    # the harness itself, all switches at a low rate, 20 s: does it run?
    'smoke'     = @{ Seconds = 20; Env = @{ MESH_OWN = '1'; MESH_USER = 'fuzz'; MESH_GROUPS = $groups; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,stream,compute,churn'; MESH_INDIRECT = '5'; MESH_STATES = '4'; MESH_SHADERS = '2'; MESH_RTS = '4'; MESH_PS = '1'; MESH_DEPTH = 'd32s8'; MESH_CLEAR = '1'; MESH_COPYBACK = '2' } }
    # the translated shader as the first test ran it (the parent test's settings, in this harness)
    'plain'     = @{ Env = @{ MESH_GROUPS = '1' } }
    # root constants that vary (the shader branches on words 9 and 12)
    'user'      = @{ Env = @{ MESH_GROUPS = $groups; MESH_USER = 'fuzz' } }
    # D3D12's auto breadcrumbs on (the launcher turns them on to light by default): a marker write after every command of
    # every list. With the draws and copies of 'mixed', and with the plainest setting
    'dred-plain' = @{ Env = @{ MESH_GROUPS = '16'; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_DRED = '1' } }
    'dred-mixed' = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STENCIL = '1'; MESH_VERTEX = '3'; MESH_INDIRECT = '9'; MESH_BATCH = '8'; MESH_DRED = '1' }, $targets, @{ MESH_COPYBACK = '3'; MESH_SLICES = '2' })) }
    # the card's own count of what it ran, with the real triangles (proof that the dispatches do work)
    'stats'     = @{ Seconds = 40; Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STATS = '1' }, $targets)) }
    # a program of our own that draws real triangles (and the translated one, fuzzed)
    'own'       = @{ Env = @{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1' } }
    # the title's depth format, colour targets with a pixel shader, clears, the copy-back, lists in flight
    'targets'   = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3' }, $targets)) }
    # a copy queue (uploads, copy-back) and a compute queue running at the same time
    'queues'    = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,compute' }, $targets)) }
    # the colour targets and the depth copied to textures of their own after the draws (the unfinished command of a
    # stopped run of the title was a CopyTextureRegion after a barrier, in a list that had run mesh and pixel shaders)
    'copies'    = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STENCIL = '1' }, $targets, @{ MESH_COPYBACK = '3' })) }
    # the stencil twin copy of a depth-stencil array of two slices: two copies between a pair of barriers
    'copies-2'  = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STENCIL = '1' }, $targets, @{ MESH_COPYBACK = '3'; MESH_SLICES = '2' })) }
    # vertex draws between the mesh draws and the copies of the depth-stencil twins after them
    'mixed'     = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STENCIL = '1'; MESH_VERTEX = '3'; MESH_INDIRECT = '9'; MESH_BATCH = '8' }, $targets, @{ MESH_COPYBACK = '3'; MESH_SLICES = '2' })) }
    # the same with a 24-bit depth and 8-bit stencil format
    'copies-d24' = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STENCIL = '1' }, $targets, @{ MESH_COPYBACK = '3'; MESH_DEPTH = 'd24s8' })) }
    # the same with a 32-bit depth buffer without stencil (its twin is copied as a whole resource)
    'copies-d32' = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3' }, $targets, @{ MESH_COPYBACK = '3'; MESH_DEPTH = 'd32' })) }
    # textures made, copied into and released, descriptors rewritten by the CPU, all million views written
    'heap'      = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,compute,stream,churn'; MESH_FILL = '1' }, $targets)) }
    # also the descriptors the draws read, rewritten while they are read
    'race'      = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,compute,stream,race'; MESH_FILL = '1' }, $targets)) }
    # many pipelines: 60 states of 12 programs of our own, and the translated one in 60 states
    'pipes'     = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_STATES = '60'; MESH_SHADERS = '12' }, $targets)) }
    # every fifth dispatch an ExecuteIndirect of mesh dispatches after a clamp pass
    'indirect'  = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_INDIRECT = '5' }, $targets)) }
    # 60 executions of one record each in a row after one clamp pass (the host's batched clamp), twice a list
    'batch'     = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_INDIRECT = '20'; MESH_BATCH = '60'; MESH_STATES = '6'; MESH_DRAWS = '40' }, $targets)) }
    # the machine's memory nearly used (12 GB held by the test)
    'pressure'  = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_PRESSURE_GB = '12'; MESH_QUEUES = 'copy,compute,stream' }, $targets)) }
    # every buffer view claims 256 MB more than its buffer holds, and the programs read at offsets that reach it:
    # what the card does with a read past the end of the memory behind a view
    'oversize'  = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_OVERSIZE_MB = '256' }, $targets)) }
    # dispatches of up to 216 groups (the largest the title's direct dispatches ask for)
    'big'       = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = '216'; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_DRAWS = '20' }, $targets)) }
    # the CPU streams memory (copies and compares of big blocks) next to the draws: a busy memory bus
    'memload'   = @{ Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,compute,stream'; MESH_MEMLOAD = '6' }, $targets)) }
    # 300 of the title's translated mesh programs (a folder of them: -DxilDir), each in its own pipeline
    'shaders'   = @{ Dir = $true; Env = @{ MESH_GROUPS = $groups; MESH_USER = 'sane'; MESH_DXIL_MAX = '300'; MESH_INFLIGHT = '3'; MESH_DEPTH = 'd32s8'; MESH_CLEAR = '1' } }
    # everything above together
    'all'       = @{ Seconds = 240; Env = (Merge-Settings @(@{ MESH_GROUPS = $groups; MESH_USER = 'fuzz'; MESH_OWN = '1'; MESH_INFLIGHT = '3'; MESH_QUEUES = 'copy,compute,stream,churn'; MESH_FILL = '1'; MESH_STATES = '60'; MESH_SHADERS = '12'; MESH_INDIRECT = '5'; MESH_PRESSURE_GB = '12' }, $targets)) }
}
if ($List) {
    foreach ($name in $experiments.Keys) {
        $e = $experiments[$name].Env
        "{0,-9} {1}" -f $name, (($e.Keys | Sort-Object | ForEach-Object { "$_=$($e[$_])" }) -join ' ')
    }
    return
}
$names = @($experiments.Keys)
if ($Only) {
    $Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })
    $unknown = @($Only | Where-Object { $_ -notin $names })
    if ($unknown) { Write-Host "Unknown: $($unknown -join ', '). Known: $($names -join ', ')"; exit 1 }
    $names = @($names | Where-Object { $_ -in $Only })
}
if (-not $Dxil -or -not (Test-Path $Dxil)) { Write-Host 'Pass -Dxil <the translated mesh shader file> (see the top of this script).'; exit 1 }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$logs = Join-Path $repo 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
$stamp = Get-Date -Format 'yyyyMMdd-HHmmss'

# The card is ours only when no game run is on: wait for one that is, then hold the name `victory` with a
# placeholder (a copy of ping.exe) for the whole list, so that no launcher starts a game run between two settings.
$waited = 0
while (Get-Process -Name victory -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*victory-fuzz*hold*' }) {
    if ($waited -ge $WaitMinutes * 60) { Write-Host 'A victory.exe is still running: stopping.'; exit 1 }
    if ($waited -eq 0) { Write-Host 'A victory.exe is running; waiting for it to end.' }
    Start-Sleep -Seconds 2
    $waited += 2
}
$tools = Join-Path $env:TEMP 'victory-fuzz'
New-Item -ItemType Directory -Force -Path $tools | Out-Null
$holdDir = Join-Path $tools 'hold'
New-Item -ItemType Directory -Force -Path $holdDir | Out-Null
Copy-Item -Force (Join-Path $env:SystemRoot 'System32\ping.exe') (Join-Path $holdDir 'victory.exe')
$holder = Start-Process -FilePath (Join-Path $holdDir 'victory.exe') -ArgumentList @('-n', "$($HoldMinutes * 60)", '127.0.0.1') -WindowStyle Hidden -PassThru
Write-Host "Holding the GPU (placeholder process $($holder.Id), at most $HoldMinutes minutes)."

# (After the wait and the placeholder: a compile next to another program's game run spoils that run.)
# The test binary is built once, and each run starts a copy of it named victory.exe: scripts\intel-sweep.ps1 refuses
# to start a game run while a process of that name runs (the launcher itself does not look), so a sweep does not
# start one on this GPU while the stress test has it (two programs on the card at once spoil both).
Push-Location $repo
try {
    $messages = & cargo test -p victory-gpu --test mesh_fuzz --no-run --locked --message-format=json 2>$null
} finally { Pop-Location }
$built = $messages | Where-Object { $_ -like '*"executable"*' } | ForEach-Object { $_ | ConvertFrom-Json } |
    Where-Object { $_.executable -and $_.target.name -eq 'mesh_fuzz' } | Select-Object -Last 1
if (-not $built) { $holder.Kill(); Write-Host 'The test binary was not built (cargo test -p victory-gpu --test mesh_fuzz --no-run).'; exit 1 }
$runner = Join-Path $tools 'victory.exe'

$rows = @()
$n = 0
try {
foreach ($name in $names) {
    $n++
    # A game run another program started meanwhile (its launcher does not look for the placeholder): wait for it.
    $waited = 0
    while (Get-Process -Name victory -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*victory-fuzz*hold*' } | Where-Object { $_.Id -ne $holder.Id }) {
        if ($waited -ge $WaitMinutes * 60) { break }
        if ($waited -eq 0) { Write-Host 'Another victory.exe is running; waiting for it to end.' }
        Start-Sleep -Seconds 2
        $waited += 2
    }
    if (Get-Process -Name victory -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*victory-fuzz*hold*' } | Where-Object { $_.Id -ne $holder.Id }) { Write-Host 'Another victory.exe is still running; stopping.'; break }
    $e = $experiments[$name]
    if ($e.Dir -and -not $DxilDir) { Write-Host ("[{0}/{1}] {2}: needs -DxilDir, skipped" -f $n, $names.Count, $name); continue }
    $secs = if ($e.Seconds) { [int]$e.Seconds } else { $Seconds }
    Write-Host ("[{0}/{1}] {2}: {3} s" -f $n, $names.Count, $name, $secs)
    # The settings of this run only.
    Get-ChildItem Env:MESH_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
    foreach ($k in $e.Env.Keys) { Set-Item "Env:$k" $e.Env[$k] }
    $env:MESH_DXIL = (Resolve-Path $Dxil).Path
    if ($e.Dir) { $env:MESH_DXIL_DIR = (Resolve-Path $DxilDir).Path }
    $env:MESH_SECONDS = "$secs"
    $log = Join-Path $logs "fuzz-$stamp-$name.txt"
    $err = "$log.err"
    $started = Get-Date
    Copy-Item -Force $built.executable $runner
    $env:RUST_TEST_THREADS = '1'
    $p = Start-Process -FilePath $runner -WorkingDirectory $repo -NoNewWindow -PassThru `
        -ArgumentList @('the_titles_load', '--ignored', '--nocapture') `
        -RedirectStandardOutput $log -RedirectStandardError $err
    $limit = ($secs + 240) * 1000   # setup (pipelines, views), the card's reset, the wait for the workers
    $killed = $false
    if (-not $p.WaitForExit($limit)) {
        $killed = $true
        try { $p.Kill() } catch { }
    }
    $text = (Get-Content $log -Raw -ErrorAction SilentlyContinue) + (Get-Content $err -Raw -ErrorAction SilentlyContinue)
    $how = if ($text -match 'no hang in') { 'no hang' }
           elseif ($text -match 'THE CARD DID NOT ANSWER') { 'THE CARD STOPPED' }
           elseif ($text -match 'THE DEVICE WAS REMOVED') { 'DEVICE REMOVED' }
           elseif ($text -match 'DID NOT ANSWER in 30 s') { 'a worker queue stopped' }
           elseif ($text -match 'panicked') { 'panic' }
           elseif ($killed) { 'killed (no end after the limit)' }
           else { 'no result' }
    $last = ([regex]::Matches($text, '(?m)^\s+\d+ s: .*$') | Select-Object -Last 1).Value
    Write-Host ("  {0} ({1:N0} s){2}" -f $how, ((Get-Date) - $started).TotalSeconds, $(if ($last) { ": $($last.Trim())" } else { '' }))
    $rows += [pscustomobject]@{ Setting = $name; Result = $how; Seconds = [int]((Get-Date) - $started).TotalSeconds; Log = (Split-Path $log -Leaf) }
    if ($how -ne 'no hang') { Write-Host "  See $log"; }
    if ($n -lt $names.Count) { Start-Sleep -Seconds $PauseSeconds }
}
} finally {
    if ($holder -and -not $holder.HasExited) { $holder.Kill() }
}
Get-ChildItem Env:MESH_* | ForEach-Object { Remove-Item "Env:$($_.Name)" }
Write-Host ''
$table = $rows | Format-Table -AutoSize | Out-String
Write-Host $table
$saved = Join-Path $logs "fuzz-$stamp.txt"
$table | Out-File -FilePath $saved -Encoding utf8
Write-Host "Saved: $saved"
