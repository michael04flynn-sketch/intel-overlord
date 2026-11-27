# Finds which mesh shaders hang the GPU by running the game with only some of them drawn.
#   scripts\intel-mesh-bisect.ps1 -Exe <path of your own copy's executable>
# VICTORY_MESH_TEST=keep:<digits>:<digits> (draw.rs `mesh_kept`) issues only the mesh draws whose mesh
# shader's hash ends in one of the first digits (and whose digit before it is one of the second, if
# given). The script halves the 16 last digits, runs the game on one half (up to -Seconds), keeps the
# half that hangs, then does the same for the digit before it, and last runs the single bucket it
# found to confirm. A run that is still alive at the limit counts as "survived" (when the game got
# into the world; a run that stayed at the front end is run again) and is closed. The mesh shaders of
# the bucket, from the last log, are listed at the end. Each run may reset the GPU.
# On the Arc B390 it found a bucket (hashes ending in 40) that hung alone, and skipping that bucket
# changed nothing: any subset of the title's mesh shaders stops the card (docs\intel-arc.md). Skip
# the bucket (-SkipPipelines) to see whether a bucket is the cause before reading anything into one.
param(
    [Parameter(Mandatory)][string]$Exe,
    [int]$Seconds = 150,
    [int]$PauseSeconds = 20,
    [double]$MinFreeGB,
    [ValidateSet('diag', 'release')][string]$Build = 'diag',
    [string]$Launcher,
    # More launcher parameters for every run, as a hashtable: @{ SkipFsr4 = $true }
    [hashtable]$Extra = @{}
)
if (-not $Launcher) { $Launcher = Join-Path $PSScriptRoot 'play-intel-lowmem.ps1' }
$repo = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$logs = Join-Path $repo 'logs'
New-Item -ItemType Directory -Force -Path $logs | Out-Null
$hex = [string[]]('0123456789abcdef'.ToCharArray())
$trail = @()
$hasMin = $PSBoundParameters.ContainsKey('MinFreeGB')

# One run with this keep filter: 'hung', 'survived' or something else, and the log it wrote.
# Only the game of this repository (target\...\victory.exe) is waited for and closed; another victory.exe
# (a test binary run from a copy elsewhere) is left alone.
function Get-OurGame {
    $target = Join-Path (Resolve-Path (Join-Path $PSScriptRoot '..')).Path 'target'
    Get-Process -Name victory -ErrorAction SilentlyContinue |
        Where-Object { $_.Path -and $_.Path.StartsWith($target, [StringComparison]::OrdinalIgnoreCase) }
}

function Invoke-Keep([string]$filter) {
    if (Get-OurGame) { throw 'A victory.exe of this repository is still running; close it first.' }
    $splat = @{ Exe = $Exe; Build = $Build; MeshTest = "keep:$filter" }
    foreach ($k in $Extra.Keys) { $splat[$k] = $Extra[$k] }
    if ($script:hasMin) { $splat['MinFreeGB'] = $script:MinFreeGB }
    $started = Get-Date
    $job = Start-Job -ScriptBlock { param($launcher, $splat) & $launcher @splat } -ArgumentList $Launcher, $splat
    $finished = Wait-Job $job -Timeout $Seconds
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
    if (-not $log) { return [pscustomobject]@{ Result = 'not started'; Log = $null } }
    $text = [IO.File]::ReadAllText($log.FullName)
    # A run that never left the front end (30 fps, 33 ms a frame) proves nothing: the world's frames
    # take 300 ms and more. (Three of the first nine runs of this script did not get there.)
    $slowest = 0.0
    foreach ($m in [regex]::Matches($text, '([0-9.]+) fps, ([0-9.]+) ms/frame')) { $slowest = [math]::Max($slowest, [double]::Parse($m.Groups[2].Value, [Globalization.CultureInfo]::InvariantCulture)) }
    $world = $slowest -ge 100
    $result = if ($closed -and -not $world) { 'no world' }
        elseif ($closed) { 'survived' }
        elseif ($text -match '0x887A0006') { 'hung' }
        elseif ($text -match '0x8007000E') { 'out of memory' }
        elseif ($text -match '0x887A0005') { 'device removed' }
        else { 'ended' }
    [pscustomobject]@{ Result = $result; Log = $log.FullName }
}

function Step([string]$filter) {
    Write-Host ("  keep:{0}  ..." -f $filter) -NoNewline
    $r = Invoke-Keep $filter
    # (The game does not always get into the world: 3 runs of 9 stayed at the front end.)
    for ($again = 1; $r.Result -eq 'no world' -and $again -le 3; $again++) {
        $script:trail += [pscustomobject]@{ Filter = "keep:$filter"; Result = 'no world (again)'; Log = $(if ($r.Log) { Split-Path $r.Log -Leaf } else { '' }) }
        Write-Host ' no world, again ...' -NoNewline
        Start-Sleep -Seconds $PauseSeconds
        $r = Invoke-Keep $filter
    }
    Write-Host (" {0}" -f $r.Result)
    $script:trail += [pscustomobject]@{ Filter = "keep:$filter"; Result = $r.Result; Log = $(if ($r.Log) { Split-Path $r.Log -Leaf } else { '' }) }
    if ($r.Result -in 'not started', 'out of memory', 'no world') { throw "Stopped: $($r.Result) (free RAM? see the launcher's message; or the game did not get into the world in 4 runs)." }
    Start-Sleep -Seconds $PauseSeconds
    $r
}

# Halve a list of digits until one is left: the half that hangs is kept (the other half is assumed
# to be the one when the first survives).
function Find-Digit([string[]]$candidates, [scriptblock]$filterFor) {
    while ($candidates.Count -gt 1) {
        $half = [math]::Floor($candidates.Count / 2)
        $a = $candidates[0..($half - 1)]
        $b = $candidates[$half..($candidates.Count - 1)]
        $r = Step (& $filterFor $a)
        $candidates = if ($r.Result -in 'hung', 'device removed') { $a } else { $b }
    }
    $candidates[0]
}

try {
    Write-Host 'Last digit of the mesh shader hash:'
    $last = Find-Digit $hex { param($d) ($d -join '') }
    Write-Host "  -> $last"
    Write-Host 'The digit before it:'
    $before = Find-Digit $hex { param($d) "${last}:$($d -join '')" }
    Write-Host "  -> $before$last"
    Write-Host 'Confirming the bucket alone:'
    $confirm = Step "${last}:${before}"
    $list = @()
    if ($confirm.Log) {
        $text = [IO.File]::ReadAllText($confirm.Log)
        $list = [regex]::Matches($text, 'shader translated[^\r\n]*stage=.?"?mesh"?[^\r\n]*hash=([0-9a-f]{14}' + $before + $last + ')') |
            ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
        if (-not $list) {
            $list = [regex]::Matches($text, 'mesh ([0-9a-f]{14}' + $before + $last + ')') | ForEach-Object { $_.Groups[1].Value } | Sort-Object -Unique
        }
    }
    Write-Host ''
    $trail | Format-Table -AutoSize | Out-String | Write-Host
    if ($confirm.Result -in 'hung', 'device removed') {
        Write-Host "The bucket $before$last hangs on its own. Its mesh shaders in the last log: $($list -join ', ')"
        Write-Host "Skip them with: -SkipPipelines $($list -join ',')"
    } else {
        Write-Host "The bucket $before$last did not hang alone ($($confirm.Result)): more than one bucket is needed, or the hang depends on timing. The table says which filters hung."
    }
} catch {
    Write-Host $_.Exception.Message
    $trail | Format-Table -AutoSize | Out-String | Write-Host
}
$saved = Join-Path $logs ("mesh-bisect-{0:yyyyMMdd-HHmmss}.txt" -f (Get-Date))
$trail | Format-Table -AutoSize | Out-String | Out-File -FilePath $saved -Encoding utf8
Write-Host "Saved: $saved"
