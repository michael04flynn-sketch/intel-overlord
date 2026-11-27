# Low-memory profile for Intel GPUs at 1080p, aiming at 30 fps.
# Starts the title by running victory.exe directly with the Series S console profile
# (1920x1080 output, 1280x720 rendering), with smaller caches for card memory (VRAM)
# and system memory (RAM). play.ps1 has no -Console parameter, so it is not used here.
#   scripts\play-intel-lowmem.ps1 -Exe <path of your own copy's executable>
# -Build        release (the default, faster) or diag
# -InPlace      use title memory in place instead of copying it (VICTORY_INPLACE=1). Run of 2026-11-27: it
#               placed 352 MB and turned down 237 windows (page-pool view memory, which the host
#               cannot open as a heap), so it did not reduce the first-frame memory burst.
# -Probe        also log which title memory blocks -InPlace (a census 30 s and 150 s into the run)
# -MinFreeGB    free RAM below which the run is not started (default 20); -Force starts anyway
# -LogDir / -NoLog   the run is logged to logs\victory-<time>.log (newest 20 kept) with a header
#               (command, git commit, GPU and driver, env values) and an exit-code summary
# -Profile      Lean, Balanced (default) or Fast: memory against CPU time (see the parameters below)
# -MeshTest / -SkipPipelines / -SkipFsr4 / -MeshStrict   experiments for the device removals; the
#               launcher prints the next ones to try after a hang. scripts\intel-sweep.ps1 runs the
#               list unattended and prints which settings survive.
# Extra settings may be passed the same way as play.ps1: -More @{VICTORY_PANEL='1'}
# Measurements of this profile on Intel hardware: none yet. Check the frame time with
# VICTORY_PANEL=1 and VRAM in Task Manager before relying on it.
param(
    [Parameter(Mandatory)][string]$Exe,
    [ValidateSet('diag', 'release')][string]$Build = 'release',
    [hashtable]$More = @{},
    # Free RAM (GB) below which the run is not started (-Force: start anyway).
    # (23: on 2026-11-27 four runs on 31 GB all ended in the first world frame. The one with
    # 20.8 GB free before the run peaked at a working set of 19.8 GB and ended with 1.2 GB free;
    # the title's own memory (about 8 GB of private growth) is not something the host can cut.)
    [double]$MinFreeGB = 23,
    [switch]$Force,
    # Free commit (RAM + pagefile, GB) below which the run is not started.
    [double]$MinCommitGB = 20,
    # Use title memory in place instead of copying it (VICTORY_INPLACE=1, see below).
    [switch]$InPlace,
    # Also log what kind of title memory blocks -InPlace (VICTORY_INPLACE_PROBE=1: a census 30 s and 150 s in).
    [switch]$Probe,
    # Hard cap (MB) on the host copies of textures the title fills (VICTORY_TEXTURE_BUDGET_MB;
    # 0 = none: the layer goes by the card's budget, which on this GPU the host now cuts to what
    # the free RAM can hold, VICTORY_UMA_HEADROOM). A hard cap is a hard stop: the run of
    # 2026-11-27 01:08 with 1024 made 1,088 MB of copies, then 599 passes found no room and 2,930
    # textures (5.6 GB) never got a copy: missing textures, nothing let go to make room.
    [int]$TextureCapMB = 0,
    # Share of the card's budget up to which texture copies are made (VICTORY_TEXTURE_ROOM_PERCENT,
    # 50 to 97, layer default 90). Set by -Profile unless passed.
    [int]$RoomPercent = 85,
    # Free RAM (MB) the host keeps back when it works out the card's budget on this GPU
    # (VICTORY_UMA_RESERVE_MB). Set by -Profile unless passed.
    [int]$ReserveMB = 3072,
    # Wait for the runtime to finish remapping title memory before reading a texture (the layer's
    # default; slow while the world streams in: 13.5 s a frame of waiting summed over threads in
    # the run above). Without this switch textures are read through the kernel instead
    # (VICTORY_TEXTURE_NO_WAIT=1, which scripts\play.ps1 sets); a texture that arrives while the
    # runtime remaps may show a frame later.
    [switch]$TextureWait,
    # Device-removed data (VICTORY_D3D12_DRED, as scripts\play.ps1 sets it): light keeps what a
    # lost device was doing, so the log names the command and pipeline instead of "no
    # breadcrumbs". off leaves it out.
    # (full: a text for every command, so the report says what the stopped copy or draw is; slower.)
    [ValidateSet('off', 'light', 'full', 'faults')][string]$Dred = 'light',
    # The D3D12 debug layer (VICTORY_D3D12_DEBUG=1): a refused command is named. Slower; needs
    # Windows' "Graphics Tools" feature, and is ignored without it.
    [switch]$D3dDebug,
    # Mesh shaders whose outputs pass the host's 32 KiB even at their lane count are refused
    # instead of being emitted anyway (VICTORY_MESH_OUTPUT_STRICT=1; lower.rs says "refuse them
    # here if one hangs"). Off by default: three runs with it on hung all the same (so it was
    # not those shaders), and in the dump it refuses 71 character shaders (MS_VS_PedTransform*
    # with Fire, Decals and Transmission variants), whose draws are then dropped. The host's
    # driver creates their pipelines (checked offline on the B390).
    [switch]$MeshStrict,
    # Skips the draws of the 17 compute shaders the title's FidelityFX FSR4 passes run (and the
    # three of them that do not translate). All four runs with device-removed data created these
    # passes at world entry, 10 to 33 s before the device was removed, and none of them is in the
    # title's files (the loader builds them at run time). A test for whether they are what
    # corrupts the memory the later mesh draws read; the upscaled picture is missing while on.
    [switch]$SkipFsr4,
    # CPU side (the log of 2026-11-27: the card was about 25 % busy and the frame waited on the
    # submitting thread; 2.6 GB a frame of title buffer copies were compared on the CPU).
    # VICTORY_FAST_WIDE=1: the helper threads use the 12 cores that share the performance cores'
    # cache (4 P + 8 E on a Core Ultra X9 388H) instead of the 4 fast ones; the submitting thread
    # stays on the fast cores. Not measured on a title; -NoFastWide turns it off.
    [switch]$NoFastWide,
    # MB of title buffer copies kept in the card's memory, where the card compares them instead of
    # the CPU (VICTORY_CARD_BUFFERS_MB). It was 128 (a memory saving); 512 is the layer's own value
    # for budgets up to 12 GiB. Each MB is system memory on this GPU, written buffers twice.
    [int]$CardBuffersMB = 512,
    # Which way to trade CPU time against memory (each sets -CardBuffersMB and -TextureCapMB unless
    # you pass them): Lean 128 / 768 (least memory, most CPU compare work, fewest texture copies),
    # Balanced 512 / 1024 (the default), Fast 1024 / 1536 (the most card buffers, the most memory).
    # A card copy of a title buffer costs a CPU-side copy and a card-side copy (and a third, "before",
    # for one shaders write): on this GPU all of it is system memory. Measured: the 1024 cap took
    # title-filled texture copies from 2,451 MB to 1,039 MB. The buffer-memory figures are from the
    # code's own description, not measured here.
    [ValidateSet('Lean', 'Balanced', 'Fast')][string]$Profile = 'Balanced',
    # Shader hashes (16 hex digits, comma separated; diag build) whose draws are skipped
    # (VICTORY_SKIP_PIPELINES): to test whether one pipeline is the one that hangs the device.
    [string]$SkipPipelines,
    # The project's own search for what stops the card (VICTORY_MESH_TEST, diag build): scissor runs
    # every mesh draw's shaders but rasterises one pixel (a hang that stays is the mesh shader itself,
    # one that goes is what its triangles cover); skip issues no mesh draw at all; keep:<digits> keeps
    # only the mesh draws whose shader hash ends in one of those digits (keep:<digits>:<digits>: and the
    # digit before it is one of the second set); only:<hash>,<hash> keeps the draws of those mesh
    # shaders alone; off draws them all (the host's default on an Intel GPU is skip: the card stops on
    # them, docs\intel-arc.md).
    [string]$MeshTest,
    # Where the log goes (default: logs\ in the repository). -NoLog writes none.
    [string]$LogDir,
    [switch]$NoLog,
    # Saves frames of the run as PNG files (half size) in logs\frames-<time>\: every power-of-two frame
    # and, once the world starts (a frame of 500 draws), its 2nd, 6th, 30th, 80th and some later ones
    # (VICTORY_CAPTURE, VICTORY_CAPTURE_WORLD; diag build). To see with your own eyes what is textured and
    # what is not, without sitting at the screen for the 2 minutes of the world.
    [switch]$Capture
)

$profiles = @{
    Lean     = @{ CardBuffersMB = 128;  TextureCapMB = 2048; RoomPercent = 70; ReserveMB = 4096 }
    Balanced = @{ CardBuffersMB = 512;  TextureCapMB = 0;    RoomPercent = 85; ReserveMB = 3072 }
    Fast     = @{ CardBuffersMB = 1024; TextureCapMB = 0;    RoomPercent = 92; ReserveMB = 2560 }
}
foreach ($name in 'CardBuffersMB', 'TextureCapMB', 'RoomPercent', 'ReserveMB') {
    if (-not $PSBoundParameters.ContainsKey($name)) { Set-Variable -Name $name -Value $profiles[$Profile][$name] }
}
Write-Host "Profile $Profile`: card buffers $CardBuffersMB MB, texture copies up to $RoomPercent % of the card's budget (cap: $(if ($TextureCapMB) { "$TextureCapMB MB" } else { 'none' })), $ReserveMB MB of RAM kept free."

# Memory knobs (values in MB unless the name says KB). Each one lowers how much is kept
# ready at once, at the cost of textures arriving a little later.
$lowMemory = @{
    # Texture queue stops filling the card at -RoomPercent of its budget (layer default 90; 50 is
    # the lowest it accepts). The card room kept for render targets counts in the same sum.
    # Only works while VICTORY_TEXTURE_BUDGET_MB is unset: that setting skips the budget check.
    VICTORY_TEXTURE_ROOM_PERCENT      = [string]$RoomPercent
    # Title buffers copied into card memory: at most -CardBuffersMB, and only buffers of at least 1 MB.
    # The queue also keeps less card room back for these copies.
    VICTORY_CARD_BUFFERS_MB           = [string]$CardBuffersMB
    VICTORY_CARD_BUFFERS_MIN_KB       = '1024'
    # Card memory kept free for render targets: left at the layer's default (3,072 MB). A lower
    # value lets textures fill more of the card.
    # Cap on title memory used in place (system memory). It only counts with -InPlace: without
    # VICTORY_INPLACE the title's memory is copied, and this value does nothing.
    VICTORY_INPLACE_MB                = '512'
    VICTORY_KEEP_SNAPSHOTS_MB         = '64'
    VICTORY_SNAPSHOT_POOL             = '128'
    # Texture bytes uploaded per frame: the layer's per-frame rate, not a total cap.
    # The default is 192 MB (96 MB under a 6 GiB budget); 256 raised it.
    VICTORY_TEXTURE_STAGE_MB          = '64'
    # Textures are uploaded ahead of need less often (default 12 ms a frame).
    VICTORY_EARLY_TEXTURES_MS         = '6'
    # Bytes of picked views checked per frame (default 32): less CPU time and memory traffic each frame.
    VICTORY_PICKED_MB                 = '8'
    # Host-memory budgets (residency.rs): texture prefetch and staging are cut back while free
    # RAM or commit is low, and on a PC with 32 GiB of RAM or less even when they are not.
    VICTORY_RESIDENCY                 = '1'
    # Smaller-PC budgets on any PC. Needs VICTORY_RESIDENCY=1, which is set above.
    VICTORY_RESIDENCY_LIMITED         = '1'
}

# -InPlace: title memory outside buffers is opened as a host heap and used where the title wrote
# it, with no copy. Run 9 made about 3 GB of such copies (shared buffers) in the first world frame
# and that growth filled the card's budget. The cap rises to 3072 MB so the windows fit. Not
# measured on this GPU yet: the line "title memory in place" (every 10 s) says what was placed and
# what was turned down.
if (-not $NoFastWide) { $lowMemory['VICTORY_FAST_WIDE'] = '1' }
if ($MeshStrict) { $lowMemory['VICTORY_MESH_OUTPUT_STRICT'] = '1' }
if ($SkipFsr4) {
    $fsr4 = '32aa8384586e0f12,3d147e8190574516,47d8c225abfcb246,528278df1274cb33,55c46e739fa0796a,6a6c7b7fde59134f,80a0178d3a9d8d12,85dea520c893a3d9,8cbd7f5b9ce7530c,8e614708143ef917,8fb5e4aa9c542c3f,ba578b59c8c28a57,c99e30956be0deaf,f46117391dd81461,f5ab5dcf95536794,f7fc6c984427fef3,fdce2a0a9fb1521a,17af85f170e66f30,477b52e73ddeb6f2,8b437ab61d8008fe'
    $SkipPipelines = if ($SkipPipelines) { "$SkipPipelines,$fsr4" } else { $fsr4 }
}
if ($MeshTest) {
    $lowMemory['VICTORY_MESH_TEST'] = $MeshTest
} else {
    # (The host's own default on an Intel adapter, said here because the picture shows it: the world's
    # meshes are missing. 26 of 27 runs that drew them stopped the card; 3 of 3 that did not, ran on.)
    Write-Host 'Mesh draws are skipped on this Intel GPU (they stop it within a minute or two of the world: docs\intel-arc.md). -MeshTest off draws them anyway.'
}
if ($Capture) {
    $frames = Join-Path (Join-Path $PSScriptRoot '..') ("logs\frames-{0:yyyyMMdd-HHmmss}" -f (Get-Date))
    New-Item -ItemType Directory -Force -Path $frames | Out-Null
    $lowMemory['VICTORY_CAPTURE'] = (Resolve-Path $frames).Path
    $lowMemory['VICTORY_CAPTURE_WORLD'] = '1'
    # frames counted from the world's first (3 fps there: 60 is 20 s in, 400 about 2 minutes)
    $lowMemory['VICTORY_CAPTURE_WORLD_ALSO'] = '60,120,180,240,300,400,500'
    Write-Host "Capturing frames to $($lowMemory['VICTORY_CAPTURE'])"
}
if ($SkipPipelines) { $lowMemory['VICTORY_SKIP_PIPELINES'] = $SkipPipelines }
if ($Dred -eq 'full') { $lowMemory['VICTORY_D3D12_DRED'] = '1' } elseif ($Dred -ne 'off') { $lowMemory['VICTORY_D3D12_DRED'] = $Dred }
if ($D3dDebug) { $lowMemory['VICTORY_D3D12_DEBUG'] = '1' }
if ($Probe) { $lowMemory['VICTORY_INPLACE_PROBE'] = '1' }
$lowMemory['VICTORY_UMA_RESERVE_MB'] = [string]$ReserveMB
if (-not $TextureWait) { $lowMemory['VICTORY_TEXTURE_NO_WAIT'] = '1' }
if ($TextureCapMB -gt 0) { $lowMemory['VICTORY_TEXTURE_BUDGET_MB'] = [string]$TextureCapMB }
if ($InPlace) {
    $lowMemory['VICTORY_INPLACE'] = '1'
    $lowMemory['VICTORY_INPLACE_MB'] = '3072'
}

# Caller values (-More) win over this profile.
$merged = @{}
foreach ($k in $lowMemory.Keys) { $merged[$k] = $lowMemory[$k] }
foreach ($k in $More.Keys) { $merged[$k] = $More[$k] }

$victory = Join-Path $PSScriptRoot "..\target\$Build\victory.exe"
if (-not (Test-Path $victory)) {
    # Fall back to the other build when only that one exists (only diag is built by default here).
    $other = if ($Build -eq 'release') { 'diag' } else { 'release' }
    $alt = Join-Path $PSScriptRoot "..\target\$other\victory.exe"
    if (Test-Path $alt) {
        Write-Host "No $victory; using the $other build instead."
        $Build = $other
        $victory = $alt
    } else {
        Write-Host "No $victory. Build it first (README.md)."; exit 1
    }
}
if (-not (Test-Path $Exe)) { Write-Host "No $Exe."; exit 1 }
$Exe = (Resolve-Path $Exe).Path

# Memory pre-flight. On this kind of PC the card's budget is system memory, and the title's
# first world frame grows the process by several GB at once (runs 6 to 8 ended with under
# 0.5 GB free). Shows what is free and what holds it, before and after the run.
function Show-MemoryState([string]$when) {
    $os = Get-CimInstance Win32_OperatingSystem
    $free = [math]::Round($os.FreePhysicalMemory / 1MB, 1)
    Write-Host ("Memory {0}: {1} GB free of {2} GB; commit {3} of {4} GB" -f $when, $free,
        [math]::Round($os.TotalVisibleMemorySize / 1MB, 1),
        [math]::Round(($os.TotalVirtualMemorySize - $os.FreeVirtualMemory) / 1MB, 1),
        [math]::Round($os.TotalVirtualMemorySize / 1MB, 1))
    Get-Process | Sort-Object WorkingSet64 -Descending | Select-Object -First 5 |
        ForEach-Object { Write-Host ("  {0,-24} {1,7} MB" -f $_.Name, [math]::Round($_.WorkingSet64 / 1MB)) }
    $free
}
$freeNow = Show-MemoryState 'before the run'
# Commit headroom: on an iGPU the card's memory is system memory too, and the title's first
# world frame grows the process by several GB. Free RAM alone does not show whether the
# pagefile can take it when RAM runs out.
$osNow = Get-CimInstance Win32_OperatingSystem
$commitFree = [math]::Round($osNow.FreeVirtualMemory / 1MB, 1)
if ($commitFree -lt $MinCommitGB -and -not $Force) {
    Write-Host "Only $commitFree GB of commit left (limit $MinCommitGB GB, -MinCommitGB). Grow the pagefile (System Managed or 16 GB or more), or pass -Force."
    exit 2
}
if ($freeNow -lt $MinFreeGB -and -not $Force) {
    Write-Host "Only $freeNow GB free, under the $MinFreeGB GB limit (-MinFreeGB). Close the biggest programs above (or restart explorer.exe if it is one), or pass -Force to start anyway."
    exit 2
}

# Windows resets a GPU that works on one piece for longer than TdrDelay (default 2 s). Runs on the
# Arc B390 had world frames of 0.4 to 3.1 s, and the last two were lost to a hung device (0x887A0006)
# in a list of mesh draws. Read-only check; the launcher never changes it.
$tdr = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\GraphicsDrivers' -ErrorAction SilentlyContinue).TdrDelay
if (-not $tdr -or $tdr -lt 10) {
    Write-Host "Note: TdrDelay is $(if ($tdr) { "$tdr s" } else { 'not set (2 s)' }). A slow world frame can be reset as a hang; to give it room, elevated: reg add HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers /v TdrDelay /t REG_DWORD /d 30 /f   (then restart)"
}

# Settings left over in this window from an earlier run (an older version of this script, or
# -InPlace last time) go; where your files are stays. Pass what you want with -More.
$paths = 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR', 'VICTORY_FIDELITYFX_DIR'
$inherited = @(Get-ChildItem Env:VICTORY_* | Where-Object { $_.Name -notin $paths })
foreach ($entry in $inherited) { Remove-Item "Env:$($entry.Name)" }
$discarded = $inherited | Where-Object { $_.Name -notin $merged.Keys } | Sort-Object Name
if ($discarded) {
    Write-Host "Discarded inherited values for $($discarded.Name -join ', '). Use -More to pass them explicitly."
}
foreach ($k in $merged.Keys) { Set-Item "Env:$k" $merged[$k] }
# The same launch scripts\play.ps1 makes, with the console chosen here (play.ps1 cannot choose it).
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$victoryArgs = @('run', $Exe, '--execute', '--console', 'series-s')

# Log file: everything victory.exe prints (stdout and stderr) goes to the console and to a
# file, with a header saying exactly what was run. -NoLog turns it off.
$writer = $null
$logPath = $null
if (-not $NoLog) {
    if (-not $LogDir) { $LogDir = Join-Path $repo 'logs' }
    New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
    $logPath = Join-Path $LogDir ("victory-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
    $writer = New-Object System.IO.StreamWriter($logPath, $false, (New-Object System.Text.UTF8Encoding($false)))
    $writer.AutoFlush = $true   # a crash or a kill still leaves the lines printed so far
    $commit = (& git -C $repo rev-parse --short HEAD 2>$null)
    $gpu = (Get-CimInstance Win32_VideoController | ForEach-Object { "$($_.Name) (driver $($_.DriverVersion))" }) -join '; '
    $writer.WriteLine("# started   $(Get-Date -Format o)")
    $writer.WriteLine("# command   $victory $($victoryArgs -join ' ')")
    $writer.WriteLine("# build     $Build, git $commit")
    $writer.WriteLine("# host      $gpu")
    $writer.WriteLine("# inplace   $InPlace, free RAM before: $freeNow GB")
    foreach ($k in ($merged.Keys | Sort-Object)) { $writer.WriteLine("# env       $k=$($merged[$k])") }
    $writer.WriteLine('# ----')
    Write-Host "Log: $logPath"
}

# Memory timeline next to the log (see memlog.ps1): free RAM and commit, and victory.exe's size.
$sampler = $null
if ($logPath) {
    $memPath = [IO.Path]::ChangeExtension($logPath, '.mem.csv')
    $sampler = Start-Process powershell -WindowStyle Hidden -PassThru -ArgumentList @(
        '-NoProfile', '-ExecutionPolicy', 'Bypass', '-File', "`"$(Join-Path $PSScriptRoot 'memlog.ps1')`"", '-Out', "`"$memPath`"", '-Under', "`"$([IO.Path]::GetFullPath((Join-Path $PSScriptRoot '..\target')))`"")   # quoted: a path may hold spaces; -Under: not another victory.exe on the PC
    Write-Host "Memory timeline: $memPath"
}

$clock = [Diagnostics.Stopwatch]::StartNew()
Push-Location $repo   # victory.toml is looked for here
try {
    # 2>&1 merges stderr; each item is turned into a plain string so PowerShell 5.1 does not
    # print stderr lines as error records.
    & $victory @victoryArgs 2>&1 | ForEach-Object {
        $line = if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" }
        Write-Host $line
        if ($writer) { $writer.WriteLine($line) }
    }
} finally {
    Pop-Location
    if ($sampler -and -not $sampler.HasExited) { Stop-Process -Id $sampler.Id -Force -ErrorAction SilentlyContinue }
}
$exitCode = $LASTEXITCODE
$clock.Stop()

# Exit code 0 is a clean exit. A large unsigned code is a Windows crash status.
$meaning = switch ($exitCode) {
    0 { 'clean exit' }
    -2005270523 { 'DXGI_ERROR_DEVICE_REMOVED (0x887A0005): the GPU device was lost, look for "out of memory" or "removed" just above' }
    -1073741819 { 'access violation (0xC0000005)' }
    -1073740791 { 'stack buffer overrun (0xC0000409)' }
    -1073741571 { 'stack overflow (0xC00000FD)' }
    -1073740940 { 'heap corruption (0xC0000374)' }
    default { '' }
}
$summary = "Exit code $exitCode $(if ($meaning) { "($meaning) " })after {0:N0} s." -f $clock.Elapsed.TotalSeconds
Write-Host $summary
# Where a frame's time went, from the last statistics the run printed: how busy the card was and
# how much title-buffer memory the CPU compared (the figures to set against a run with other settings).
if ($logPath -and (Test-Path $logPath)) {
    $t = Get-Content $logPath -Raw
    $busy = [regex]::Matches($t, 'nothing of ours queued ([\d.]+) ms \(the card about (\d+)% busy')
    $cmp = [regex]::Matches($t, '([\d.]+) MB in \d+ ranges of title buffers. copies')
    if ($busy.Count) { Write-Host "Last frame statistics: card about $($busy[$busy.Count - 1].Groups[2].Value) % busy, idle $($busy[$busy.Count - 1].Groups[1].Value) ms a frame." }
    if ($cmp.Count) { Write-Host "  title buffer copies compared on the CPU: $($cmp[$cmp.Count - 1].Groups[1].Value) MB a frame (more of them on the card with -CardBuffersMB)." }
}
# The memory timeline's extremes (the RAM figures to set against another profile).
if ($memPath -and (Test-Path $memPath)) {
    $rows = @(Import-Csv $memPath)
    if ($rows.Count) {
        $minFree = ($rows | ForEach-Object { [int]$_.free_ram_mb } | Measure-Object -Minimum).Minimum
        $peakWs = ($rows | Where-Object { $_.victory_ws_mb } | ForEach-Object { [int]$_.victory_ws_mb } | Measure-Object -Maximum).Maximum
        Write-Host ("Memory: lowest free RAM {0:N1} GB, peak victory.exe working set {1:N1} GB." -f ($minFree / 1024), ($peakWs / 1024))
    }
}
# What the run said about why the device went (the exit code only says that it did).
if ($logPath -and (Test-Path $logPath)) {
    $text = Get-Content $logPath -Raw
    if ($text -match '0x887A0006') {
        # The command the device-removed data says did not finish: the first list whose breadcrumbs
        # show fewer commands done than recorded ("done/count"), and its command number "done".
        $after = $text.IndexOf('device-removed data')
        if ($after -ge 0) {
            $tail = $text.Substring($after)
            $stuck = $null
            foreach ($m in [regex]::Matches($tail, '"(\d+)/(\d+) [^"]*"')) {
                if ([int]$m.Groups[1].Value -lt [int]$m.Groups[2].Value) { $stuck = [int]$m.Groups[1].Value; break }
            }
            if ($null -ne $stuck -and $tail -match "command $stuck ran (\[[^\]]+\])") {
                Write-Host "Device-removed data: the first unfinished command is $stuck, running $($Matches[1])."
                $mesh = [regex]::Match($Matches[1], 'mesh ([0-9a-f]{16})').Groups[1].Value
                Write-Host 'Same command in several runs means a fault in that draw. To narrow it, rerun with one of:'
                Write-Host '  -MeshTest scissor   (still hangs: the mesh shader itself; no hang: what its triangles cover)'
                if ($mesh) { Write-Host "  -SkipPipelines $mesh   (that shader's draws are dropped: some geometry missing)" }
                Write-Host '  -MeshTest skip      (no mesh draws at all)'
                Write-Host '  -SkipFsr4           (the FidelityFX FSR4 compute passes: created 10 to 33 s before every hang so far)'
                Write-Host '  scripts\intel-sweep.ps1 runs the whole list one after the other and prints which settings survive.'
            } elseif ($tail -match 'the first uncompleted is command index (\d+) \(op (\d+)\) of "([^"]*)"') {
                # Not a draw the report can name: say what kind of command it is (D3D12 breadcrumb ops).
                $ops = @{ 3 = 'DrawInstanced'; 4 = 'DrawIndexedInstanced'; 5 = 'ExecuteIndirect'; 6 = 'Dispatch'; 7 = 'CopyBufferRegion'
                          8 = 'CopyTextureRegion'; 9 = 'CopyResource'; 12 = 'ClearRenderTargetView'; 13 = 'ClearUnorderedAccessView'
                          14 = 'ClearDepthStencilView'; 15 = 'ResourceBarrier'; 42 = 'DispatchMesh' }
                $op = [int]$Matches[2]
                $name = if ($ops.ContainsKey($op)) { $ops[$op] } else { "op $op" }
                Write-Host "Device-removed data: the first unfinished command is $($Matches[1]) of $($Matches[3]), a $name."
                Write-Host '  (With -Dred full the report says what such a copy or draw is. Hangs have named ExecuteIndirect, DispatchMesh and CopyTextureRegion; the run with no mesh draws at all (-MeshTest skip) did not hang.)'
            } elseif ($tail -match 'command (\d+) ran (\[[^\]]+\])') {
                Write-Host "Device-removed data: near command $($Matches[1]), running $($Matches[2])."
            }
        }
        Write-Host 'The log says the GPU HUNG (0x887A0006): Windows reset it after TdrDelay seconds on one piece of work. If the time to the reset grows when TdrDelay does (11.9 s at 10 in the run of 2026-11-27, 3.3 s at 2), the draw never finishes: a real hang, not a slow frame. Read the lines after "host GPU device removed" for the command; or give slow frames room (elevated PowerShell, then restart):'
        Write-Host '  reg add HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers /v TdrDelay /t REG_DWORD /d 30 /f'
        Write-Host '  (undo: reg delete HKLM\SYSTEM\CurrentControlSet\Control\GraphicsDrivers /v TdrDelay /f)'
    } elseif ($text -match '0x8007000E') {
        Write-Host 'The log says the card ran out of memory (0x8007000E): see the .mem.csv for the free RAM at that moment.'
    }
}
$after = Show-MemoryState 'after the run'
if ($writer) {
    $writer.WriteLine('# ----')
    $writer.WriteLine("# $summary")
    $writer.WriteLine("# free RAM after: $after GB")
    $writer.Close()
    # Keep the newest 20 logs, and drop the memory timelines of the ones that go.
    Get-ChildItem $LogDir -Filter 'victory-*.log' | Sort-Object LastWriteTime -Descending |
        Select-Object -Skip 20 | ForEach-Object {
            Remove-Item $_.FullName, ([IO.Path]::ChangeExtension($_.FullName, '.mem.csv')) -ErrorAction SilentlyContinue
        }
    Write-Host "Log saved: $logPath"
}
exit $exitCode
