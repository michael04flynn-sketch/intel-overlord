# Preload for the game: run this before it (once after a build, a driver update or a new game version;
# the launcher's -Preload does it for you). It does ahead of time what the game would otherwise do in its
# first minutes, while you look at a frozen or half-drawn world:
#
#   1. SHADERS    every shader program of the game's shaders.rpf translated into the host's cache
#                 (`victory shaders`; a few minutes, once per translator build).
#   2. PIPELINES  the host pipelines of every pipeline earlier runs drew with, compiled by the driver and
#                 kept in its on-disk cache (`victory warm --used`), in child processes so that a compiler
#                 that hangs costs one pipeline. On an Intel GPU this includes the compute twin of each
#                 mesh program and the pipeline that draws it (the meshes of the world), and the lighting
#                 passes (the screen probes and the rest), which are among the pipelines the game draws with.
#   3. FILES      the game's small archives and executable read once, so that a cold start (after a
#                 reboot) does not read them from the disk while the game waits (the PC keeps them as cache
#                 that it gives back when the game wants the memory).
#
# What it cannot do: textures. They are made on the card from the game's own memory while it runs; there
# is nothing of them to build before the game exists. What speeds them up is room (free RAM: close big
# programs) and the shaders and pipelines above being ready, so that no draw waits for a compile.
#
#   .\scripts\preload-intel.ps1 -Exe 'C:\path\to\game_scarlett_master.exe'
#   .\scripts\preload-intel.ps1 -Exe ... -Play      # then start the game with the Intel launcher
#   -Force           do the steps again even if the stamp says they are up to date
#   -All             warm every pipeline the title ever created (84,000: hours), not just the used ones
#   -Jobs <n>        compiler processes at once (default 6; each may take 1 to 2 GB while it works)
#   -SkipShaders, -SkipPipelines, -SkipFiles
# The game must not be running. Nothing here is changed by it but the host's cache folders
# (%LOCALAPPDATA%\victory\cache) and the stamp beside them.
param(
    [Parameter(Mandatory)][string]$Exe,
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [int]$Jobs = 6,
    [switch]$Force,
    [switch]$All,
    [switch]$SkipShaders,
    [switch]$SkipPipelines,
    [switch]$SkipFiles,
    # Run the Intel launcher when done (it is given -Exe and nothing else).
    [switch]$Play,
    # Free RAM the pipeline compilers need to start (they are about 6 x 1.5 GB).
    [double]$MinFreeGB = 12,
    [string]$LogDir
)

$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
if (-not (Test-Path $Exe)) { Write-Host "No $Exe."; exit 1 }
$Exe = (Resolve-Path $Exe).Path
$gameDir = Split-Path $Exe -Parent

$victory = Join-Path $repo "target\$Build\victory.exe"
if (-not (Test-Path $victory)) {
    $other = if ($Build -eq 'release') { 'diag' } else { 'release' }
    $alt = Join-Path $repo "target\$other\victory.exe"
    if (Test-Path $alt) { Write-Host "No $victory; using the $other build."; $victory = $alt }
    else { Write-Host "No $victory. Build it first: cargo build --profile diag -p victory --features diagnostics"; exit 1 }
}

# Only this repository's game matters, and it must not run (it uses the cache these steps write).
$running = Get-Process -Name victory -ErrorAction SilentlyContinue |
    Where-Object { $_.Path -and $_.Path.StartsWith((Join-Path $repo 'target'), [StringComparison]::OrdinalIgnoreCase) }
if ($running) { Write-Host 'A victory.exe of this repository is running; close it first.'; exit 1 }

$os = Get-CimInstance Win32_OperatingSystem
$free = [math]::Round($os.FreePhysicalMemory / 1MB, 1)
if (-not $SkipPipelines -and $free -lt $MinFreeGB) {
    Write-Host "Only $free GB of RAM free, under the $MinFreeGB GB the pipeline compilers need (-MinFreeGB). Close big programs, or pass -Jobs 2 -MinFreeGB 6."
    exit 2
}

# The settings the launcher runs the game with leave the translations alone, so these steps run with none of
# yours either: the cache is keyed by them (shader_cache.rs).
$paths = 'VICTORY_CONFIG', 'VICTORY_GAME_DIR', 'VICTORY_CACHE_DIR', 'VICTORY_DATA_DIR', 'VICTORY_FIDELITYFX_DIR'
foreach ($entry in @(Get-ChildItem Env:VICTORY_* | Where-Object { $_.Name -notin $paths })) { Remove-Item "Env:$($entry.Name)" }

if (-not $LogDir) { $LogDir = Join-Path $repo 'logs' }
New-Item -ItemType Directory -Force -Path $LogDir | Out-Null
$logPath = Join-Path $LogDir ("preload-{0:yyyyMMdd-HHmmss}.log" -f (Get-Date))
$writer = New-Object System.IO.StreamWriter($logPath, $false, (New-Object System.Text.UTF8Encoding($false)))
$writer.AutoFlush = $true
function Say([string]$text) { Write-Host $text; $writer.WriteLine($text) }
function Run-Step([string]$title, [string[]]$arguments) {
    Say ''
    Say "== $title"
    Say "   victory $($arguments -join ' ')"
    $clock = [Diagnostics.Stopwatch]::StartNew()
    Push-Location $repo   # victory.toml is looked for here
    try {
        & $victory @arguments 2>&1 | ForEach-Object {
            $line = if ($_ -is [System.Management.Automation.ErrorRecord]) { $_.Exception.Message } else { "$_" }
            $writer.WriteLine($line)
            # (the progress and the summary lines only; the whole output is in the log)
            if ($line -match '^(programs|translated|structured|DXIL|containers|warm|made|done|left|\d+ of|pipelines|skipped|failed|bad|slow)|%|ETA|elapsed|translated|made ') { Write-Host "   $line" }
        }
    } finally { Pop-Location }
    $code = $LASTEXITCODE
    $clock.Stop()
    Say ("   exit code {0}, {1:N0} s" -f $code, $clock.Elapsed.TotalSeconds)
    return $code
}

# --- the stamp: what these steps were last done for -------------------------------------------------
$cache = Join-Path $env:LOCALAPPDATA 'victory\cache'
if ($env:VICTORY_CACHE_DIR) { $cache = $env:VICTORY_CACHE_DIR }
New-Item -ItemType Directory -Force -Path $cache | Out-Null
$stampPath = Join-Path $cache 'preload-stamp.json'
$driver = (Get-CimInstance Win32_VideoController | Where-Object { $_.Name -match 'Intel' } | Select-Object -First 1).DriverVersion
$vi = Get-Item $victory
$gi = Get-Item $Exe
$wanted = [ordered]@{
    victory = "$($vi.Length)-$($vi.LastWriteTimeUtc.Ticks)"
    game    = "$($gi.Length)-$($gi.LastWriteTimeUtc.Ticks)"
    driver  = "$driver"
    all     = [bool]$All
}
$stamp = $null
if ((Test-Path $stampPath) -and -not $Force) { try { $stamp = Get-Content $stampPath -Raw | ConvertFrom-Json } catch { $stamp = $null } }
function Current([string]$step) {
    if (-not $stamp -or -not $stamp.$step) { return $false }
    foreach ($k in $wanted.Keys) { if ("$($stamp.$step.$k)" -ne "$($wanted[$k])") { return $false } }
    return $true
}
$done = [ordered]@{}
if ($stamp) { foreach ($p in $stamp.PSObject.Properties) { $done[$p.Name] = $p.Value } }
function Mark([string]$step) {
    $entry = [ordered]@{}
    foreach ($k in $wanted.Keys) { $entry[$k] = $wanted[$k] }
    $entry['at'] = (Get-Date -Format o)
    $script:done[$step] = $entry
    ($script:done | ConvertTo-Json -Depth 4) | Set-Content -Path $stampPath -Encoding UTF8
}

Say "Preload for $Exe"
Say "build $victory, driver $driver, $free GB RAM free, log $logPath"
$failed = @()

# --- 1. shaders -------------------------------------------------------------------------------------
if ($SkipShaders) { Say "`n== shaders: skipped (-SkipShaders)" }
elseif (Current 'shaders') { Say "`n== shaders: up to date for this build, driver and game (-Force does them again)" }
else {
    $list = Join-Path $LogDir 'preload-untranslated.txt'
    $code = Run-Step 'shaders: every program of the game translated into the cache' @('shaders', $gameDir, '--list', $list)
    if ($code -eq 0) { Mark 'shaders' } else { $failed += 'shaders' }
}

# --- 2. pipelines -----------------------------------------------------------------------------------
if ($SkipPipelines) { Say "`n== pipelines: skipped (-SkipPipelines)" }
else {
    $arguments = @('warm', '--jobs', "$Jobs")
    if (-not $All) { $arguments += '--used' }
    # (`warm` keeps its own list of what it has made for this translator and driver and goes on from there,
    # so it is run every time: with nothing left it ends at once.)
    $code = Run-Step 'pipelines: the driver compiles what the game draws with (and the twins of the meshes)' $arguments
    if ($code -eq 0) { Mark 'pipelines' } else { $failed += 'pipelines' }
}

# --- 3. files ---------------------------------------------------------------------------------------
if ($SkipFiles) { Say "`n== files: skipped (-SkipFiles)" }
else {
    Say ''
    Say '== files: the small archives and the executable read into the PC''s file cache'
    $names = 'game_scarlett_master.exe', 'textures.rpf', 'shaders.rpf', 'common.rpf', 'data.rpf', 'appdata0.rpf', 'script.rpf', 'Catalog000.bin'
    $buffer = New-Object byte[] (4MB)
    $total = 0.0
    $clock = [Diagnostics.Stopwatch]::StartNew()
    foreach ($name in $names) {
        $file = Join-Path $gameDir $name
        if (-not (Test-Path $file)) { continue }
        # never take the RAM the game needs: stop if less than 14 GB would be left free
        $now = [math]::Round((Get-CimInstance Win32_OperatingSystem).FreePhysicalMemory / 1MB, 1)
        $size = (Get-Item $file).Length
        if ($now - $size / 1GB -lt 14) { Say ("   {0}: left out ({1:N1} GB free now, it is {2:N1} GB)" -f $name, $now, ($size / 1GB)); continue }
        $fs = [IO.File]::Open($file, 'Open', 'Read', 'ReadWrite', 'SequentialScan')
        try { while ($fs.Read($buffer, 0, $buffer.Length) -gt 0) { } } finally { $fs.Dispose() }
        $total += $size
        Say ("   {0}: {1:N0} MB" -f $name, ($size / 1MB))
    }
    $clock.Stop()
    Say ("   {0:N2} GB read in {1:N1} s" -f ($total / 1GB), $clock.Elapsed.TotalSeconds)
}

# --- the end ----------------------------------------------------------------------------------------
$os = Get-CimInstance Win32_OperatingSystem
Say ''
Say ("RAM free now: {0:N1} GB." -f ($os.FreePhysicalMemory / 1MB))
$writer.Dispose()
if ($failed) {
    Write-Host ("Not finished: {0}. The log has the details: {1}" -f ($failed -join ', '), $logPath)
    exit 3
}
Write-Host "Preload done. Log: $logPath"
if ($Play) {
    & (Join-Path $PSScriptRoot 'play-intel-lowmem.ps1') -Exe $Exe -Build $Build
}
