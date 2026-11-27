# Runs the Intel launcher once per setting, unattended, and says which settings the game survives.
#   scripts\intel-sweep.ps1 -Exe <path of your own copy's executable>
# Each run gets -Seconds (default 180: the world starts about 85 s in, and every device removal so
# far came 10 to 35 s after that); a run still alive then is closed by the sweep and counts as
# "survived" when the game had got into the world (frames of 100 ms and more), else "no world":
# that run says nothing, run it again. A run that ends sooner is reported with how it ended. Every run writes its own log
# and memory timeline in logs\ (play-intel-lowmem.ps1 does that); the table is saved there too.
# Table columns from the run's last statistics: MedianMs (world frames), Waiting (texture slots waiting for room),
# Unmapped (passed over: the title had unmapped their memory), LateS (mean seconds a texture waited for memory),
# MinFreeGB / PeakGB (the PC's lowest free RAM and the game's peak working set, from the .mem.csv).
#   -Only base,skip-fsr4   run just those
#   -Repeat n              run each of them n times (three runs of one setting in a row)
#   -Hold                  wait for a game run that is on to end, then keep a placeholder process named victory.exe
#                          alive for the whole sweep, so that no other launcher starts a game run on the card between
#                          two of these runs
#   -List                  show the settings and stop
# Close big programs first: the launcher refuses to start under its free-RAM floor, and the sweep
# then stops. Each run may reset the GPU (a device removal): the screen can go black for a second.
param(
    [Parameter(Mandatory)][string]$Exe,
    [int]$Seconds = 180,
    # With a value: each run ends that many seconds after the world's first frame (a frame of 100 ms
    # and more). The title's wait on the front end varies a lot between runs (85 s to over 200 s), so
    # with a fixed -Seconds one run had 140 s of world and the next 20 s; this gives every run the same
    # stretch. -Seconds is then only the longest a run may take (use 400 and more).
    [int]$WorldSeconds = 0,
    [int]$PauseSeconds = 20,
    # Passed to the launcher as -MinFreeGB when given (its default is 23).
    [double]$MinFreeGB,
    [string[]]$Only,
    [int]$Repeat = 1,
    [switch]$Hold,
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [string]$Launcher,
    # Each run saves PNG frames of the world (and the frames' op lists) to logs\frames-<time>\<setting>\
    # (VICTORY_CAPTURE, VICTORY_CAPTURE_WORLD; the launcher's -Capture does the same for one run).
    [switch]$Capture,
    [switch]$List
)

if (-not $Launcher) { $Launcher = Join-Path $PSScriptRoot 'play-intel-lowmem.ps1' }   # (not in the param block: $PSScriptRoot is empty there in Windows PowerShell 5.1)

# name -> what is passed to the launcher on top of -Exe and -Build. In the order of how much each
# tells: what stays when a setting is taken away says what it was.
$experiments = [ordered]@{
    # the launcher as it is: on an Intel GPU the host skips the title's mesh draws (the others below draw them)
    'default'      = @{ MeshTest = '' }
    'base'         = @{}
    # the FidelityFX FSR4 compute passes (created at world entry before every hang so far)
    'skip-fsr4'    = @{ SkipFsr4 = $true }
    # every submission waited for: no copy-back thread, nothing sent ahead
    'no-relay'     = @{ More = @{ VICTORY_RELAY = '0' } }
    # mesh draws run their shaders but rasterise one pixel / are not issued at all
    'mesh-scissor' = @{ MeshTest = 'scissor' }
    'mesh-skip'    = @{ MeshTest = 'skip' }
    # the six mesh shaders whose hashes end in 40 (scripts\intel-mesh-bisect.ps1: that bucket hung
    # alone, run of 2026-11-27 11:08), skipped: if this survives they are the cause
    'skip-40'      = @{ SkipPipelines = '0384efa1b7bcd540,0d9d1c59733d6340,15ac5676f1960a40,4332a100adad3d40,70c4f59493d78540,a56a584996afb040' }
    # only the four depth-only cascade shadow mesh shaders (MS_VS_CascadeShadows_draw, a straight-line
    # program of 628 lines that stopped the card with one group), and everything but them
    'mesh-shadows-only' = @{ MeshTest = 'only:05393503e72397a3,45ace4127ac61cfd,d42a640660651285,f507ff49d4aadd4b' }
    'mesh-no-shadows'   = @{ SkipPipelines = '05393503e72397a3,45ace4127ac61cfd,d42a640660651285,f507ff49d4aadd4b' }
    # the shadow shaders alone, with a barrier on all work after each dispatch (a hang that goes is a race
    # between draws), and with no groups in the dispatch (a hang that stays is in the pipeline's state)
    'shadows-fence'     = @{ MeshTest = 'only:05393503e72397a3,45ace4127ac61cfd,d42a640660651285,f507ff49d4aadd4b'; More = @{ VICTORY_MESH_FENCE = '1' } }
    'shadows-zero'      = @{ MeshTest = 'only:05393503e72397a3,45ace4127ac61cfd,d42a640660651285,f507ff49d4aadd4b'; More = @{ VICTORY_MESH_ZERO = '1' } }
    # the mesh programs as compute twins (VICTORY_MESH_TWIN, docs/intel-arc.md, "A compute twin"): stage A runs
    # each mesh draw's program as compute, storing its vertices and indices in an arena, and draws nothing (a run
    # that still hangs says the hang is in what the programs do and read, not in the mesh pipeline); stage B draws
    # the arena with a vertex shader and the title's pixel shader; the third is stage B for the four shadow
    # cascade programs alone
    'twin'         = @{ MeshTest = 'skip'; More = @{ VICTORY_MESH_TWIN = 'all' } }
    'twin-draw'    = @{ MeshTest = 'skip'; More = @{ VICTORY_MESH_TWIN = 'all'; VICTORY_MESH_TWIN_DRAW = '1' } }
    'twin-shadows' = @{ MeshTest = 'skip'; More = @{ VICTORY_MESH_TWIN = '05393503e72397a3,45ace4127ac61cfd,d42a640660651285,f507ff49d4aadd4b'; VICTORY_MESH_TWIN_DRAW = '1' } }
    # nothing is ever let go (host copies of title buffers and textures stay): if the hang goes, a view in
    # the heap that a shader picks by an index reads memory the host had given back
    'no-sweep'     = @{ More = @{ VICTORY_SWEEP = '0'; VICTORY_TEXTURE_SWEEP = '0' } }
    # the debug layer with GPU-based validation (descriptor kinds and ranges of what shaders pick
    # from the heap by an index; very slow, so give it -Seconds 420 or more)
    'gbv'          = @{ D3dDebug = $true; Profile = 'Lean'; More = @{ VICTORY_D3D12_GBV = '1' } }
    # every title buffer's copy in the PC's memory (the card's own copies are copied into with
    # COPY_DEST states that the debug layer found read as shader resources without a barrier)
    'card-off'     = @{ More = @{ VICTORY_CARD_BUFFERS = '0' } }
    # a host heap of one slot bound for the sampler / view heap the title did not set (the debug
    # layer: SetComputeRootSignature before a sampler heap, 114 times in a run)
    'fallback-heaps' = @{ More = @{ VICTORY_FALLBACK_HEAPS = '1' } }
    # descriptor reads as data by plain loads instead of getDimensions on a picked handle
    'no-getdims'   = @{ More = @{ VICTORY_NO_GETDIMS = '1' } }
    # mesh draws on and the device-removed data (D3D12's auto breadcrumbs, which the launcher turns on to light by default and
    # which put a marker write after every command of every list) off
    'dred-off'     = @{ Dred = 'off' }
    # the same with the full device-removed data (a text for every command: the report names the copy or draw that
    # does not finish; slower)
    'base-dred'    = @{ Dred = 'full' }
    # the title's own counts of vertices and primitives in the mesh programs instead of the declared maxima
    # (VICTORY_MESH_COUNTS=exact: docs\intel-arc.md), mesh draws on
    'exact'        = @{ More = @{ VICTORY_MESH_COUNTS = 'exact' } }
    'exact-dred'   = @{ Dred = 'full'; More = @{ VICTORY_MESH_COUNTS = 'exact' } }
    # the one mesh program (MS_VS_Transform_4dbafe93, with its pixel shader 3711fdad456538a1) whose ExecuteIndirect was
    # the first command not finished in two runs (command 107 of a list of 160, 14 s after its pipeline's first bind):
    # its draws alone, and every draw but its own
    'only-ea2e'    = @{ MeshTest = 'only:ea2eadde55090b27' }
    'skip-ea2e'    = @{ SkipPipelines = 'ea2eadde55090b27' }
    # indirect mesh executions (most of the title's mesh draws), each issued with a count of 0 (the clamp and all state
    # as before: nothing runs), and with a barrier on all work after each / every 8th one
    'ind-zero'     = @{ More = @{ VICTORY_MESH_INDIRECT = 'zero' } }
    'ind-fence'    = @{ More = @{ VICTORY_MESH_INDIRECT = 'fence:1' } }
    'ind-fence8'   = @{ More = @{ VICTORY_MESH_INDIRECT = 'fence:8' } }
    # every mesh draw, direct and indirect, issued empty (zero groups, a count of 0): all the state of a draw and no work
    'all-zero'     = @{ More = @{ VICTORY_MESH_ZERO = '1'; VICTORY_MESH_INDIRECT = 'zero' } }
    # the three above together (if this one hangs too, none of them is the cause alone)
    'all-three'    = @{ More = @{ VICTORY_CARD_BUFFERS = '0'; VICTORY_FALLBACK_HEAPS = '1'; VICTORY_NO_GETDIMS = '1' } }
    # memory against CPU time, with the host's default (no mesh draws: the settings that stay stable); compare
    # the frame times in the logs (scripts do not average them: `ms/frame` lines of the world)
    # the settings VIctory-NVIDIA's launcher plays with (scripts/play.ps1 there: relay 2, copy queue,
    # card steps, indirect batch, thin lists, joined copies...), with the host default for meshes
    'nv-all'       = @{ MeshTest = ''; More = @{ VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'; VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'; VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_CARD_AWAKE = '1'; VICTORY_SHOW_AFTER = '1'; VICTORY_THIN = '1'; VICTORY_JOIN_PARTS = '8'; VICTORY_UPLOAD_AHEAD = '1' } }
    # the same without keeping the card's clocks up (a compute queue spinning between submissions: on an
    # iGPU that shares the CPU's power budget it may cost more than it gives)
    'nv-noawake'   = @{ MeshTest = ''; More = @{ VICTORY_TEXTURE_NO_WAIT = '1'; VICTORY_RELAY = '2'; VICTORY_WIDE_BACK = '1'; VICTORY_CARD_REHASH_RUNS = '1'; VICTORY_JOIN_COPIES = '1'; VICTORY_INDIRECT_WINDOW = '1'; VICTORY_RELAY_COPY = '1'; VICTORY_CARD_STEPS = '2'; VICTORY_INDIRECT_BATCH = '2'; VICTORY_MAP_THREADS = '1'; VICTORY_TITLE_PRIORITIES = '0'; VICTORY_SHOW_AFTER = '1'; VICTORY_THIN = '1'; VICTORY_JOIN_PARTS = '8'; VICTORY_UPLOAD_AHEAD = '1' } }
    # Textures (the host caps a streamed texture's side at 1024, 512 from 92 % of the card's budget, 256
    # from 97 %: on this GPU the budget is what is in use plus the free RAM less the reserve, so in the
    # world it stands at 100 % and every texture made then is 256 pixels at most) and what limits how many
    # are made (the card's room is the PC's free RAM)
    'tex-512'      = @{ MeshTest = ''; More = @{ VICTORY_TEXTURE_MAX = '512' } }
    'tex-1024'     = @{ MeshTest = ''; More = @{ VICTORY_TEXTURE_MAX = '1024' } }
    'inplace'      = @{ MeshTest = ''; InPlace = $true }
    'reserve-2g'   = @{ MeshTest = ''; More = @{ VICTORY_UMA_RESERVE_MB = '2048' } }
    # the indirect light stand-in the NVIDIA launcher turns on (80 %)
    'ambient'      = @{ MeshTest = ''; More = @{ VICTORY_AMBIENT = '0.8' } }
    'lean'         = @{ Profile = 'Lean'; MeshTest = '' }
    'fast'         = @{ Profile = 'Fast'; MeshTest = '' }
    # the card's budget as the system gives it
    'uma-off'      = @{ More = @{ VICTORY_UMA_HEADROOM = '0' } }
}
function Format-Setting($value) {
    if ($value -is [bool]) { return '' }
    if ($value -is [hashtable]) { return '@{' + (($value.Keys | ForEach-Object { "$_='$($value[$_])'" }) -join '; ') + '}' }
    "$value"
}
if ($List) {
    foreach ($name in $experiments.Keys) {
        $shown = ($experiments[$name].Keys | ForEach-Object { ("-$_ " + (Format-Setting $experiments[$name][$_])).Trim() }) -join ' '
        "{0,-13} {1}" -f $name, $(if ($shown) { $shown } else { '(as the launcher is)' })
    }
    return
}
$names = @($experiments.Keys)
if ($Only) {
    $Only = @($Only | ForEach-Object { $_ -split ',' } | Where-Object { $_ })   # (-File passes a comma list as one string)
    $unknown = @($Only | Where-Object { $_ -notin $names })
    if ($unknown) { Write-Host "Unknown: $($unknown -join ', '). Known: $($names -join ', ')"; exit 1 }
    $names = @($Only)   # in the order given, repeats too (-Only base-dred,exact,exact)
}
# Each setting $Repeat times, in the order given.
$seen = @{}
$plan = @(foreach ($name in $names) {
    foreach ($k in 1..$Repeat) {
        $seen[$name] = 1 + [int]$seen[$name]
        [pscustomobject]@{ Name = $name; Label = $(if ($seen[$name] -gt 1 -or $Repeat -gt 1) { "$name#$($seen[$name])" } else { $name }) }
    }
})
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$logs = Join-Path $repo 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null

# What a log says about how the run ended.
function Get-Outcome([string]$path) {
    $text = [IO.File]::ReadAllText($path)
    $how = 'ended'
    if ($text -match '0x887A0006') { $how = 'hung (0x887A0006)' }
    elseif ($text -match '0x8007000E') { $how = 'out of memory (0x8007000E)' }
    elseif ($text -match '0x887A0005') { $how = 'device removed (0x887A0005)' }
    elseif ($text -match 'panicked at') { $how = 'panic' }
    $frames = [regex]::Matches($text, 'frame=(\d+)') | ForEach-Object { [int]$_.Groups[1].Value }
    $last = if ($frames) { ($frames | Measure-Object -Maximum).Maximum } else { 0 }
    $secs = $null
    if ($text -match '# Exit code (-?\d+)[^\r\n]*? after (\d+) s') { $secs = [int]$Matches[2] }
    # Whether the game got into the world: its frames take 300 ms and more, the front end's 33 ms.
    # (A run that stays at the front end for the whole time proves nothing about the hang; 3 of 9
    # runs of one session did.)
    $slowest = 0.0
    foreach ($m in [regex]::Matches($text, '([0-9.]+) fps, ([0-9.]+) ms/frame')) {
        $slowest = [math]::Max($slowest, [double]::Parse($m.Groups[2].Value, [Globalization.CultureInfo]::InvariantCulture))
    }
    [pscustomobject]@{ How = $how; LastFrame = $last; Seconds = $secs; World = ($slowest -ge 100) }
}

# The game of this repository (target\<build>\victory.exe): only that is waited for and closed. Another
# victory.exe, e.g. a test binary somebody else is running from a copy elsewhere, is left alone (it can
# still change the timings: run when it is gone if the numbers matter).
function Get-OurGame {
    Get-Process -Name victory -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith((Join-Path $repo 'target'), [StringComparison]::OrdinalIgnoreCase) }
}

# What a run's log says about textures and memory, from its last statistics (the sweep's table columns).
function Get-RunStats([string]$path) {
    $inv = [Globalization.CultureInfo]::InvariantCulture
    $text = [IO.File]::ReadAllText($path)
    $last = { param($rx) $m = [regex]::Matches($text, $rx); if ($m.Count) { $m[$m.Count - 1] } else { $null } }
    $r = [ordered]@{ Waiting = ''; Unmapped = ''; LateS = ''; MedianMs = ''; MinFreeGB = ''; PeakGB = '' }
    $m = & $last 'textures picked by index: (\d+) uploaded so far \((\d+) MB\), (\d+) waiting, (\d+) passed over unmapped'
    if ($m) { $r.Waiting = [int]$m.Groups[3].Value; $r.Unmapped = [int]$m.Groups[4].Value }
    $m = & $last 'the same after waiting for memory: (\d+), mean (\d+) ms'
    if ($m) { $r.LateS = [math]::Round([double]$m.Groups[2].Value / 1000, 0) }
    # world frames only (the front end's are 33 ms): the median of the logged ms a frame
    $ms = @([regex]::Matches($text, '([0-9.]+) fps, ([0-9.]+) ms/frame') |
        ForEach-Object { [double]::Parse($_.Groups[2].Value, $inv) } | Where-Object { $_ -ge 100 } | Sort-Object)
    if ($ms.Count) { $r.MedianMs = [math]::Round($ms[[int][math]::Floor(($ms.Count - 1) / 2)], 0) }
    $csv = [IO.Path]::ChangeExtension($path, '.mem.csv')
    if (Test-Path $csv) {
        $free = @(); $ws = @()
        foreach ($row in Import-Csv $csv) {
            $w = 0.0
            if ([double]::TryParse($row.victory_private_mb, [ref]$w) -and $w -ge 1000) {
                $free += [double]$row.free_ram_mb; $ws += [double]$row.victory_ws_mb
            }
        }
        if ($free.Count) {
            $r.MinFreeGB = [math]::Round(($free | Measure-Object -Minimum).Minimum / 1024, 1)
            $r.PeakGB = [math]::Round(($ws | Measure-Object -Maximum).Maximum / 1024, 1)
        }
    }
    [pscustomobject]$r
}

# Whether the log written so far has a frame of 100 ms or more (the world: the front end's are 33 ms).
function Test-InWorld([string]$path) {
    try {
        $fs = [IO.File]::Open($path, 'Open', 'Read', 'ReadWrite')
        try {
            $len = $fs.Length
            $take = [int][math]::Min($len, 400000)
            [void]$fs.Seek($len - $take, 'Begin')
            $buf = New-Object byte[] $take
            $got = $fs.Read($buf, 0, $take)
            $text = [Text.Encoding]::UTF8.GetString($buf, 0, $got)
        } finally { $fs.Dispose() }
    } catch { return $false }
    foreach ($m in [regex]::Matches($text, '([0-9.]+) fps, ([0-9.]+) ms/frame')) {
        if ([double]::Parse($m.Groups[2].Value, [Globalization.CultureInfo]::InvariantCulture) -ge 100) { return $true }
    }
    return $false
}
$holder = $null
if ($Hold) {
    $waited = 0
    while (Get-Process -Name victory -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*victory-fuzz*hold*' }) {
        if ($waited -ge 900) { Write-Host 'A victory.exe is still running after 15 minutes: stopping.'; exit 1 }
        if ($waited -eq 0) { Write-Host 'A victory.exe is running; waiting for it to end.' }
        Start-Sleep -Seconds 2
        $waited += 2
    }
    $holdDir = Join-Path $env:TEMP 'victory-fuzz\hold'
    New-Item -ItemType Directory -Force -Path $holdDir | Out-Null
    Copy-Item -Force (Join-Path $env:SystemRoot 'System32\ping.exe') (Join-Path $holdDir 'victory.exe')
    $holder = Start-Process -FilePath (Join-Path $holdDir 'victory.exe') -ArgumentList @('-n', '3600', '127.0.0.1') -WindowStyle Hidden -PassThru
    Write-Host "Holding the card (placeholder process $($holder.Id), at most an hour)."
}
$foreign = { Get-Process -Name victory -ErrorAction SilentlyContinue | Where-Object { $_.Path -notlike '*victory-fuzz*hold*' } | Where-Object { -not $holder -or $_.Id -ne $holder.Id } }

$rows = @()
$n = 0
try {
foreach ($item in $plan) {
    $n++
    $name = $item.Name
    if (Get-OurGame) {
        Write-Host 'A victory.exe of this repository is still running; close it first.'
        break
    }
    $others = @(& $foreign)
    if ($others) { Write-Host ("  (note: {0} other victory.exe running, not ours: PID {1}; it may change the timings)" -f $others.Count, (($others | ForEach-Object { $_.Id }) -join ', ')) }
    Write-Host ("[{0}/{1}] {2}: up to {3} s" -f $n, $plan.Count, $item.Label, $Seconds)
    # Mesh draws on, unless the setting says otherwise: the host skips them on an Intel GPU by default.
    $splat = @{ Exe = $Exe; Build = $Build; MeshTest = 'off' }
    if ($PSBoundParameters.ContainsKey('MinFreeGB')) { $splat['MinFreeGB'] = $MinFreeGB }
    foreach ($k in $experiments[$name].Keys) { $splat[$k] = $experiments[$name][$k] }
    if ($Capture) {
        if (-not $frames) { $frames = Join-Path $logs ('frames-' + (Get-Date -Format 'yyyyMMdd-HHmmss')) }
        $more = @{}
        if ($splat['More']) { foreach ($k in $splat['More'].Keys) { $more[$k] = $splat['More'][$k] } }
        $more['VICTORY_CAPTURE'] = Join-Path $frames $name
        $more['VICTORY_CAPTURE_WORLD'] = '1'
        $more['VICTORY_CAPTURE_WORLD_ALSO'] = '60,120,180,240,300,400,500'
        New-Item -ItemType Directory -Force -Path $more['VICTORY_CAPTURE'] | Out-Null
        $splat['More'] = $more
    }
    $started = Get-Date
    $job = Start-Job -ScriptBlock { param($launcher, $splat) & $launcher @splat } -ArgumentList $Launcher, $splat
    $cap = $started.AddSeconds($Seconds)
    $limit = $cap
    $worldAt = $null
    while ($true) {
        $finished = Wait-Job $job -Timeout 5
        if ($finished -or (Get-Date) -ge $limit) { break }
        if ($WorldSeconds -gt 0 -and -not $worldAt) {
            $cur = Get-ChildItem $logs -Filter 'victory-*.log' -ErrorAction SilentlyContinue |
                Where-Object { $_.LastWriteTime -ge $started.AddSeconds(-2) } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
            if ($cur -and (Test-InWorld $cur.FullName)) {
                $worldAt = Get-Date
                $due = $worldAt.AddSeconds($WorldSeconds)
                if ($due -lt $cap) { $limit = $due }
                Write-Host ("  the world began {0:N0} s in; ends the run {1} s later" -f ($worldAt - $started).TotalSeconds, $WorldSeconds)
            }
        }
    }
    $closed = $false
    if (-not $finished) {
        $closed = $true
        Get-OurGame | Stop-Process -Force
        [void](Wait-Job $job -Timeout 90)
    }
    $null = Receive-Job $job -ErrorAction SilentlyContinue
    Remove-Job $job -Force
    $log = Get-ChildItem $logs -Filter 'victory-*.log' -ErrorAction SilentlyContinue |
        Where-Object { $_.LastWriteTime -ge $started.AddSeconds(-2) } | Sort-Object LastWriteTime -Descending | Select-Object -First 1
    if (-not $log) {
        Write-Host '  No log: the launcher did not start the game (probably the free-RAM floor). Stopping.'
        $rows += [pscustomobject]@{ Setting = $item.Label; Result = 'not started'; Seconds = ''; LastFrame = ''; Log = '' }
        break
    }
    $o = Get-Outcome $log.FullName
    $result = if ($closed -and -not $o.World) { 'no world' } elseif ($closed) { 'survived' } else { $o.How }
    $secs = if ($closed) { [int]((Get-Date) - $started).TotalSeconds } else { $o.Seconds }
    Write-Host ("  {0}, {1} s, last frame {2}" -f $result, $secs, $o.LastFrame)
    $st = Get-RunStats $log.FullName
    $rows += [pscustomobject]@{ Setting = $item.Label; Result = $result; Seconds = $secs; LastFrame = $o.LastFrame; MedianMs = $st.MedianMs; Waiting = $st.Waiting; Unmapped = $st.Unmapped; LateS = $st.LateS; MinFreeGB = $st.MinFreeGB; PeakGB = $st.PeakGB; Log = $log.Name }
    if ($n -lt $plan.Count) { Start-Sleep -Seconds $PauseSeconds }
}
} finally {
    if ($holder -and -not $holder.HasExited) { $holder.Kill() }
}

Write-Host ''
$table = $rows | Format-Table -AutoSize | Out-String
Write-Host $table
$survivors = @($rows | Where-Object { $_.Result -eq 'survived' })
$base = $rows | Where-Object { $_.Setting -eq 'base' -or $_.Setting -like 'base#*' }
if ($survivors.Count -eq 0) {
    Write-Host 'No setting survived. The hang does not depend on any of them; the logs name the command it stops at.'
} elseif ($base -and $base.Result -eq 'survived') {
    Write-Host 'The base setting survived: nothing to bisect (try a longer -Seconds).'
} else {
    Write-Host ("Survived: {0}. What those settings take away is where to look." -f (($survivors | ForEach-Object { $_.Setting }) -join ', '))
}
$saved = Join-Path $logs ("sweep-{0:yyyyMMdd-HHmmss}.txt" -f (Get-Date))
$table | Out-File -FilePath $saved -Encoding utf8
Write-Host "Saved: $saved"
