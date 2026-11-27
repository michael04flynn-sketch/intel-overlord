# Running on an Intel Arc iGPU (shared memory)

Measured on one machine only: Intel Arc B390 (driver 32.0.101.8724), 31 GB RAM, Windows 11,
title dump 1.0.2.1, `--console series-s`, `diag` build. One run so far; treat every number as a
single data point.

## What the host reports

| Query | Value |
|---|---|
| Wave lanes (`WaveLaneCountMin` / `Max`) | 16 / 32 |
| `DedicatedVideoMemory` | 128 MB (a carve-out, not the card's memory) |
| Memory architecture | UMA true, cache-coherent UMA false, isolated MMU true |
| Local memory budget (DXGI) | about 9.2 GB at start; it follows free RAM and fell to 7.6 GB |
| Shared (non-local) budget | 0 |
| View heap | refused above 1,008,571 views; settles at 1,000,000 (`raw_twins=false`) |

The card's memory is the PC's memory. Free RAM is the real limit, not the VRAM figure.

## How to run

```powershell
cargo build --locked --profile diag -p victory --features diagnostics
target\diag\victory.exe shaders <the shader archives>
scripts\play-intel-lowmem.ps1 -Exe <path of your copy's executable> -Build diag -More @{VICTORY_PANEL='1'}
scripts\intel-sweep.ps1 -Exe <path of your copy's executable>     # the settings below, one run each, unattended
```

`-InPlace` and `-Probe` are no longer in the example: in place covered 350 MB of 6.8 GB of the
title's memory (the rest is page-pool view memory the host cannot open), and the probe only
reported that.

Close browsers, remote-desktop tools and anything large first. The launcher refuses to start
under 20 GB free RAM or 20 GB free commit (`-MinFreeGB`, `-MinCommitGB`, `-Force` overrides).

Each run writes `logs\victory-<time>.log` and `logs\victory-<time>.mem.csv` (free RAM, free
commit, `victory.exe` working set and private bytes, once a second). The newest 20 are kept.

## Known result (2026-11-27)

The run reached the first world frame (frame 1217, 54 s in) and ended there. Inside that
frame free RAM fell from 17.9 GB to 2.9 GB: about 3.2 GB of shared buffer copies (4,183
buffers), a 2 GB title-filled texture, and GPU memory use rising from 5.1 to 8.1 GB of an 8.2 GB
budget. A host command list then failed to close with E_OUTOFMEMORY (0x8007000E) and the
device was removed (exit code 0x887A0005).

- `-InPlace` placed 352 MB and turned down 237 windows (page-pool view memory, which the host
  cannot open as a heap). It did not reduce the burst.
- The FSR4 passes fall back (`DeserializeComputePipelineStateX FAILED`) and the host policy
  refuses `amd_fidelityfx_upscaler_xbox.dll`. Expected on this hardware; not a cause.
- 198 shaders do not translate: 195 containers with no vertex, pixel, compute or mesh program,
  and 3 with a lane mask exchanged where not every lane runs (cause on Intel not established).

## Second run (2026-11-27, with `-Probe` and the memory timeline)

Same end, 45 s in, frame 1034: command list failed to close with E_OUTOFMEMORY, device removed.

- `victory.exe` private bytes grew from 7.3 to 14.1 GB (working set 6 to 17.9 GB) in the last
  10 s. Free RAM at the end was still 4.6 GB and free commit about 30 GB: not a RAM or pagefile
  limit. The limit is the GPU's local memory budget, which was 9,208 MB and moved between 8,092
  and 8,599 MB as free RAM fell.
- Card use went from 5.4 GB to 8.1 GB in about 2 s: shared buffer copies from 0.35 to 3.1 GB
  (the pass that makes buffer views ready for shaders that pick them by index: 77,216 views,
  4,010 copies, 1,930 MB), on top of 2.9 GB of texture blocks and 2.1 GB of title-filled textures.
- Census of title memory (`-Probe`): 42,632 views of physical pages (6,826 MB), 42,626 of them one
  64 KiB page; four reserved sections (220, 297, 185 and 3,977 MB) that the host refuses as heaps.
  Only 2 of 12 MB of private committed memory could be opened. So `-InPlace` cannot cover the
  title's memory here (77 windows, 368 MB placed, 488 turned down).

Change made for it: on a UMA GPU the always-allowed texture host copies start at 1 GiB instead of
2 GiB (`VICTORY_TEXTURE_FLOOR_MB` sets it). That is about 1 GB less before the burst; not
measured yet.

## Third run (2026-11-27, with the 1 GiB texture floor)

Ended at 36 s, frame 734, before the first world frame, so the floor change is still untested.
Cause: a panic in the performance panel (`VICTORY_PANEL=1`), `clamp(0, -156250)` in the per-core
CPU meter: a core's counters went backwards between two looks (a parked core on a hybrid CPU).
A panic there cannot unwind and aborted the process (exit 0xC0000409). Fixed: such a look counts
as an idle core, and a panel failure is caught and logged instead of ending the run.

## Fourth run (2026-11-27, panel fixed, 1 GiB texture floor)

Reached the first world frame (frame 1001, 53 s) and ended with the device removed (0x887A0005,
after 0x887A0006: a 3.4 s stall preparing one submission, with free RAM at 1.2 GB).

- This time the GPU's local budget was 14,978 MB, not 9,208 MB: on a UMA GPU it follows free
  RAM, so it is no limit worth going by. The card's own use peaked at 9.0 GB, well under it.
- The limit was physical RAM. `victory.exe` working set went from 6.4 GB to 19.8 GB (private
  bytes 7.5 to 15.6 GB) in about 15 s; free RAM from 13.6 GB to 1.2 GB, with Memory Compression
  active. Before the run there were 20.8 GB free.
- Of that growth the card's accounting explains about 3.4 GB; the rest is title memory touched
  as the world loads, which the host does not control.
- The 1 GiB floor did not hold the title-filled texture copies down (2,451 MB were made): the
  layer's rule beyond the floor goes by the budget, which was large.

Changes for it, in the launcher: `-TextureCapMB` (default 1024, sets `VICTORY_TEXTURE_BUDGET_MB`)
and `-MinFreeGB` 23. Not measured. With 31 GB of RAM this machine is at the edge: the title needs
about 20 GB of working set by the first world frame.

## Fifth run (2026-11-27, texture cap 1024 MB, 25.9 GB free before)

Reached frame ~1000 (53 s) and ended again, but this time not for memory: working set 18.3 GB,
free RAM never under 6.9 GB, no E_OUTOFMEMORY. The device was removed as HUNG (0x887A0006, then
0x887A0005) after the submitting thread spent 3.3 s preparing one submission, with 13 submissions
sent ahead. Windows' default timeout (`TdrDelay` 2 s, default here) resets a GPU that works that
long on one piece of work; whether the world's first frame is that slow on this iGPU is not
established. The log said "no breadcrumbs" because the Intel launcher did not set
`VICTORY_D3D12_DRED` (scripts\play.ps1 does).

Changes: the launcher sets `VICTORY_D3D12_DRED=light` by default (`-Dred off|light|faults`), has
`-D3dDebug`, and after a run reads its log: on 0x887A0006 it prints the `TdrDelay` registry
command (to run elevated by hand; the launcher changes no system setting) and on 0x8007000E it
points at the memory timeline. Pipeline-library notes: 40 of 61,293 pages of the 251 MB library
read as zeros and some pipelines were remade ("The parameter is incorrect", 0x80070057); not
shown to matter.

## Sixth run (2026-11-27, device-removed data on)

Ended at 64 s, frame 1252, hung device (0x887A0006) again; RAM was fine (free never under 6.2 GB
until the process died, working set 18.6 GB). `TdrDelay` is not set on this machine (2 s).

- DRED names the list: list 448 of 689 had finished 53 of its 86 commands. The commands around
  the stop are indirect mesh executions (kind 10, max 1), for example
  `[mesh 3c067b199b10fea2, pixel 3ec62ecabb36f1d2]`, `[mesh 85eaffa59a3a6ecf, pixel b352dd911b665364]`,
  `[mesh 53a2a19cf91c6088, pixel 8b901a7fb38c169b]`. The device-removed report gives the page of
  each command's view slots; those slots are placed buffers in title memory.
- Frames at that point took 0.35 to 3.1 s (frame 1244: 3.1 s); the removal came 3.6 s after frame
  1252 began, with the submitting thread 3.3 s into one list. So a single list running past the
  2 s timeout is the likeliest reading, but a bad command is not excluded: the data does not say
  which. The test that tells them apart is a larger `TdrDelay` (30): if the run then goes on, it
  was slowness.

The launcher now notes a low `TdrDelay` before the run (read-only) and prints the first command
the device-removed data names after a hang.

## Seventh run (2026-11-27, `TdrDelay` 30 read by the host)

Ended at 58 s, frame 1142, hung device (0x887A0006) again. Both runs with device-removed data
(the sixth and this one) name the same draw as the first command that did not finish:

| Run | List | Done / recorded | First unfinished command |
|---|---|---|---|
| sixth | 448 of 689 | 53 / 86 | 53: `[mesh 85eaffa59a3a6ecf, pixel b352dd911b665364]` |
| seventh | 485 of 735 | 48 / 90 | 48: `[mesh 85eaffa59a3a6ecf, pixel b352dd911b665364]` |

It is an indirect mesh execution (kind 10, max 1) that reads a 43 MB placed buffer (895,654
elements of 48 bytes) and a 998,400-byte one; the commands around it (`3c067b199b10fea2`,
`53a2a19cf91c6088`, `8321022826281144`) use the same views.

- The same draw in two runs points at a fault in that draw, not at load. The hang was reported
  3.2 to 3.7 s into the list in every run, including this one with `TdrDelay` 30 in the registry;
  whether that value was in force in Windows (it is read at start-up) is not known, so slowness is
  not ruled out by this alone.
- `lower.rs` marks a known gap (`mesh_prologue`): a mesh shader whose outputs pass the host's 32 KiB
  even at its lane count is emitted anyway ("drivers take them; refuse them here if one hangs").
  Whether `85eaffa59a3a6ecf` is such a shader is not established.

Experiments, both in the launcher: `-MeshStrict` is on by default (`-NoMeshStrict` turns it off;
`VICTORY_MESH_OUTPUT_STRICT=1`): such shaders are refused and their draws dropped. `-SkipPipelines
85eaffa59a3a6ecf` skips that shader's draws (diag build) to test whether it is the one. After a hang
the launcher prints the first unfinished command and its pipeline from the log.

## CPU and GPU load (from the seventh run's frame statistics)

Last statistics block before the hang (frames of roughly 0.3 s, so about 3 fps):

- The card was about 25 % busy: 241.7 ms a frame with nothing of ours queued, 163.1 ms of it while
  the submitting thread was still preparing (the card idle behind it).
- The submitting thread spent 113 ms in the lists' steps and 90.7 ms waiting for flights; texture
  refreshes took 174.5 ms across threads, 148.9 ms of it waiting for the runtime to stop remapping
  title memory.
- 2,601 MB a frame of title buffer copies were compared on the CPU (2,579 MB snapshotted), 2,567
  MB of them in copies of 1 MiB or more. The log says `VICTORY_CARD_BUFFERS_MIN_MB=1` would have the
  card look there; the real limiter is the cap the launcher set, 128 MB (`VICTORY_CARD_BUFFERS_MB`).
- The process kept its threads on 4 fast cores of 16 (`our threads' processors`); the code has an
  opt-in for exactly this part (`VICTORY_FAST_WIDE=1`: four performance cores, eight efficiency
  cores on one cache, "not measured on a title yet").

So the frame is CPU-bound and the card idles. Changes, in the launcher and not measured: 
`VICTORY_FAST_WIDE=1` by default (`-NoFastWide`) and `-CardBuffersMB` 512 instead of 128. More
buffers on the card means more system memory used (written buffers count twice), so watch
`.mem.csv`. After a run the launcher prints the last card-busy figure and the MB compared on
the CPU; those are the two numbers to compare between settings. None of this can matter until a
run gets past the hang at the mesh draw above.

## Memory against CPU time: launcher profiles

What the seventh run (128 MB card buffers, 1024 MB texture cap, no wide helpers) reached before the
hang, from its log and `.mem.csv`: lowest free RAM 6.2 GB, peak `victory.exe` working set 18.0 GB,
card use 8.3 GB at frame 1132 (budget 14,978 MB, which follows free RAM on this GPU). Of that
8.3 GB: 3.2 GB shared buffer copies (4,622 buffers), 1.8 GB texture blocks, 2.1 GB host copies
(1.0 GB of them title-filled textures, now capped), 0.7 GB upload buffers, 0.3 GB staging.
The 1024 MB cap did what it says: title-filled texture copies 2,451 MB (uncapped, fourth run)
against 1,039 MB.

CPU time and memory pull the other way. A card copy of a title buffer keeps a CPU-side copy and a
card-side copy (a third, "before", when shaders write it), all system memory on this GPU, in return
for the card comparing it instead of the CPU (2.6 GB a frame were compared on the CPU). So the
launcher has `-Profile Lean` (128 / 768 MB: least memory), `Balanced` (512 / 1024, default) and
`Fast` (1024 / 1536), setting `-CardBuffersMB` and `-TextureCapMB` unless they are passed. After a run
it prints the lowest free RAM and peak working set next to the card-busy and CPU-compare figures, so
two profiles can be compared on the same four numbers. Which profile is better on this machine is
not known: no run has lived long enough to measure frame time.

## Eighth run (2026-11-27, Balanced profile, mesh-output strict on, `TdrDelay` 10)

Ended at 65 s, hung device again, at the same draw: list 572, 48 of 88 commands done, first
unfinished command 48 = `[mesh 85eaffa59a3a6ecf, pixel b352dd911b665364]` (third run in a row).

- The wait before the reset scaled with `TdrDelay`: the submitting thread was 11.9 s into its list
  with the value at 10, against 3.3 s at the default 2 (the seventh run's 3.2 s with 30 in the
  registry was too short for the value to have been in force). So the draw does not finish at all:
  a real hang, not a slow frame.
- `VICTORY_MESH_OUTPUT_STRICT=1` changed nothing: no mesh shader was refused and the same draw ran.
  The shader is not an output-size overshoot.
- Same list, in flight around it: `3c067b199b10fea2`, `53a2a19cf91c6088`, `8321022826281144`.

What the profile did (this run against the seventh, last statistics block of each):

| | Seventh (128 MB card buffers, no wide helpers) | Eighth (512 MB, `VICTORY_FAST_WIDE=1`) |
|---|---|---|
| Card busy | 22 to 25 % | 48 to 67 % |
| Title buffers compared on the CPU | about 2,500 to 2,600 MB a frame | about 140 to 170 MB a frame |
| Threads | 4 fast cores | 4 fast, 12 wide, of 16 |
| Lowest free RAM / peak working set | 6.2 / 18.0 GB | 5.7 / 18.9 GB |

So the CPU side moved as intended and the cost was about 0.9 GB more working set. Frame time was
not measured (the log does not carry it; the panel does).

Launcher: `-MeshTest` passes the project's own search for what stops the card
(`VICTORY_MESH_TEST`, `draw.rs`): `scissor` runs the mesh shaders but rasterises one pixel (a hang
that stays is the mesh shader itself, one that goes is what its triangles cover), `skip` issues
no mesh draw, `keep:<digit>` keeps only shaders whose hash ends in that digit. After a hang the
launcher prints the three next steps for the named command.

## Ninth run (2026-11-27 01:08, Balanced, `TdrDelay` 10): into the world, textures missing

The first run that lived into the game: 125 s, 1,543 frames; about 35 s of loading screens at 30 fps,
then the world at 6.6 s and 3.7 s a frame (0.2 to 0.3 fps). Ended with the device removed again,
this time at a different draw: list 339 (the 25-list submission had been prepared for 16.4 s, the
reset came with `TdrDelay` 10), 44 of 73 commands done, the first unfinished one among eight mesh
pipelines (`[mesh a26422c01dcf5612, pixel 6a3a8d3fe52b4eab]`, `[mesh 3b408d7e675fddc5, ...]`, ... and
a vertex one). So mesh draws hang on this GPU at more than one place, not at one shader only.

Why the textures were missing, from the log:

- `VICTORY_TEXTURE_BUDGET_MB=1024` (the launcher's cap) is a hard stop. Title-filled copies reached
  1,088 MB, then "passes that made none for lack of room: 599" and "2,930 title textures have no host
  copy yet (5,632 MB of title memory)". Nothing was let go to make room, because the sweep goes by
  the card's budget (14,978 MB) and the card was at 8.5 GB. Fixed: no hard cap by default.
- Waiting on the runtime: "texture refreshes per frame, all threads: 13,519 ms, of them 13,486 ms
  waiting for title memory to hold still (the runtime remapping it)". `texture.rs` describes this
  exact case and its remedy, `VICTORY_TEXTURE_NO_WAIT=1`, which `scripts\play.ps1` sets and this
  launcher did not. Now on (`-TextureWait` restores the wait). Textures may arrive a frame later.
- Dropped draws (the frame line says 4,180 at worst, 1,466 later): 1,457 a frame are the ray-tracing
  builder, which is off. The rest are compute passes: `477b52e73ddeb6f2`, `17af85f170e66f30`,
  `8b437ab61d8008fe` do not translate ("a lane mask exchanged where not every lane runs is not
  supported", the three of the first shader run), and `02384a60fda6f2d0`, `2da4beb386509b77`,
  `369cbe906ea72764` translate but the host refuses them (`0x80070057`). Six compute passes missing
  from every frame can explain some of the see-through geometry. `-D3dDebug` (needs the Graphics Tools
  feature) names why the host refuses the three; the three lane-mask ones need translator work.

The memory limit on this GPU is the free RAM, not the card's budget (the system's figure follows
free RAM: 9,208 MB in one run, 14,978 MB in another). The host now works the budget out as the
system's or, if less, what the process uses plus the free RAM less a reserve
(`VICTORY_UMA_RESERVE_MB`, set by the profile: 4,096 / 3,072 / 2,560 MB). The texture queue and the
sweep that lets idle copies go both use it, so copies stop and idle ones go before the PC runs out.
The profiles are Lean (cap 2,048 MB, 70 %), Balanced (no cap, 85 %, the default) and Fast (92 %).
None of this has been run in game.

## Tenth run (2026-11-27 01:33): textures better, frames 4 to 6 times faster, still the same end

The ninth run's two texture causes are gone. Ended at 117 s, frame about 2,256, hung device again.

- Waiting on the runtime: "texture refreshes ... waiting for title memory to hold still" is 0.0 ms
  (13,486 ms before), with `VICTORY_TEXTURE_NO_WAIT=1`.
- Copies: 21,930 title-filled host copies alive, 2,202 MB (10,760 and 1,088 MB with the hard cap),
  "passes that made none for lack of room" 8 to 19 (599). The host's budget line shows it working:
  "the card's budget is taken as 12,182 MB, not the system's 14,978 MB (in use 9,549 MB, free RAM
  5,705 MB, reserve 3,072 MB)".
- World frames: 1.0 to 1.2 s (3.7 to 6.6 s before), the card about 23 % busy. Lowest free RAM 4.5
  GB, peak working set 20.8 GB.
- Still 5,192 to 5,610 title textures (5.9 GB of title memory) with no host copy: the queue is at
  its room again (used 9.6 GB against 85 % of 12.1 GB). The limit on this machine is RAM, and the
  rest of the 9.5 GB in use (3.0 GB shared buffer copies, 3.2 GB texture blocks, 1.3 GB upload
  buffers) is not the textures'.
- The end: list 100 stopped at command 35, direct mesh dispatches (`a mesh dispatch of 1 groups`) of
  `[mesh 45ace4127ac61cfd]` and its siblings, which are `MS_VS_CascadeShadows_draw_65459485_Wrapped`
  (mesh-only, depth-only shadow cascade draws). The earlier hangs were `MS_VS_TransformD_c5fd36bb`
  and `MS_VS_Transform_4dbafe93` (indirect, with a pixel shader). So the hang is in mesh draws
  generally, at changing places.
- A pattern in all four runs with device-removed data (7th to 10th): the title's FidelityFX FSR4
  passes were created at world entry ("FFX FSR4 ACO: DeserializeComputePipelineStateX FAILED ...
  falling back to NewBE"), 9.7, 20, 33 and 27 s before the device was removed, never after. The 17
  compute shaders they run are not in the title's files (the loader builds them at run time) and the
  same 17 hashes appear in every run; three more of them do not translate. Whether they have
  anything to do with it is not known: a stray write of one of them would corrupt memory the later
  draws read, and no page fault is reported (DRED has none for any run).

### Offline, from the title's files (shaders.rpf, `victory.exe translate` on this GPU)

None of this needs the game running. `VICTORY_TRANSLATE_ONLY=<hashes>` translates just those
shaders, `--pipelines` makes their host pipelines on this GPU, `--validate` runs dxil.dll over
them, `VICTORY_EMIT_DUMP=<dir outside the repo>` keeps the DXIL, and `dxc -dumpbin` (Windows SDK)
reads it. `victory.exe shader-info -v` lists every shader by the hash a run's log uses.

- 26,503 containers (614 duplicates), 26,084 programs: 12,056 pixel, 2,191 vertex, 8,765 mesh and
  2,682 compute shaders. All translate.
- The mesh shaders that hang are well formed: no barriers, `SetMeshOutputCounts` first and
  uniform, outputs within the limit (one position, 128 vertices and primitives), and the host's
  pipelines for them are created by this driver (checked for `45ace4127ac61cfd`, `d42a640660651285`,
  `85eaffa59a3a6ecf`, `3c067b199b10fea2`). Nothing in the DXIL points at the hang.
- `VICTORY_MESH_OUTPUT_STRICT=1` refuses 71 mesh shaders: the character families
  `MS_VS_PedTransform*` and `MSNC_VS_PedTransformSkin*` with Fire, Decals and Transmission variants.
  Without it they are emitted and the driver creates their pipelines (the validator says their
  outputs pass 32 KiB). Three runs with it on hung all the same, so it is off again in the launcher
  (`-MeshStrict` turns it on).
- 195 geometry-stage programs (`GS_VoxelizeGeneric`) do not translate; the title does not use
  them on this path.
- Of the 2,682 compute programs 1,261 are translated for waves of 32 and 28 stay pinned to a wave of
  64, which this GPU (waves 16 to 32) refuses at pipeline creation with "The parameter is
  incorrect": they are the passes dropped every frame. Families: GpuParticleEmit
  (`CS_Emit_Dynamic_Model_*`, `CS_EmitCounts_Dynamic_Edge`), GpuParticleUpdate `CS_Visibility`,
  vfx_triangleClip (`CS_CollectTriangles*`, `CS_DefragHeap_ComputeRelocations`), proj_decal_trail
  (build, pack, update, defrag), water_shading `CS_TransmissionVolumetrics*`, alphaResolve
  `CS_AlphTransmissionPrefilterMip`, hair shadow masks (`..._SampleHairScreenSpace`) and
  `CS_SpawnLiquidTrail_SampleDecalRaycastTriangleTest`. Missing: GPU particles, decal trails,
  volumetric transmission (water), hair shadows. The translator keeps 64 for them because they
  share LDS inside a console wave without barriers, read lanes (`v_readlane`, `v_mbcnt`) or use lane
  masks as data; making them run in waves of 32 is translator work, not a setting.
- The log's "host pipeline refused" line now says "wave size 64" for such a program.

### Settings to tell the causes apart

`scripts\intel-sweep.ps1` runs settings one after another (each up to `-Seconds`, default 180) and
prints which ones survive (`-List` shows them: `default`, `base`, `skip-fsr4`, `no-relay`,
`mesh-scissor`, `mesh-skip`, `card-off`, `fallback-heaps`, `no-getdims`, `all-three`, `no-sweep`,
`mesh-shadows-only`, `mesh-no-shadows`, `shadows-fence`, `shadows-zero`, `skip-40`, `gbv`, `lean`,
`uma-off`). It closes a run still alive at the limit and counts it as survived when the game had got
into the world (frames of 100 ms and more; a run that stayed at the front end at 30 fps says
`no world` and proves nothing: 4 runs of one session did), and leaves a `sweep-<time>.txt` and
one log with a memory timeline per run in `logs\`. `scripts\intel-mesh-bisect.ps1` halves the mesh
shaders by the last digits of their hashes (`VICTORY_MESH_TEST=keep:<digits>[:<digits>]`).
`VICTORY_MESH_TEST=only:<hash>,<hash>` draws just those mesh shaders.

## The session of 2026-11-27, runs 11 to 40: what stops the card, and what does not

26 of the 27 runs that issued the title's mesh draws ended in a hung device (0x887A0006; "device
removed" 0x887A0005 with the debug layer) 56 to 147 s after the process started, soon after the
world's first frames (the 27th ran a filter that kept only the shaders whose hashes end in 00, 10, 20
or 30). The three runs with `-MeshTest skip` ran on to the limit in the world (187 s, 200 s and 200 s,
frames 2835, 2577 and 1720, 17 GB private). So the launcher, and the host on an Intel adapter, skip
mesh draws unless told otherwise (`-MeshTest off`, `VICTORY_MESH_TEST=off`): the world's meshes are
missing, the run is stable.

| Setting | What it tests | Result |
|---|---|---|
| `-SkipFsr4` | the FSR4 compute passes | hung, 73 s |
| `VICTORY_RELAY=0` | submissions sent ahead, the copy-back thread | hung, 63 s |
| `-MeshTest scissor` | what the mesh triangles cover (one pixel rasterised) | hung, 68 s |
| `-MeshTest skip` (3 runs) | all mesh draws | **survived** 187 s, 200 s, 200 s |
| debug layer, full device-removed data | validation errors, the stopped command | hung, 86 s; see below |
| `VICTORY_CARD_BUFFERS=0`, `VICTORY_FALLBACK_HEAPS=1`, `VICTORY_NO_GETDIMS=1` together | the three findings of the debug layer, and `getDimensions` of a picked descriptor | hung, 123 s |
| `VICTORY_SWEEP=0`, `VICTORY_TEXTURE_SWEEP=0` | host copies given back while a view still shows them | hung, 56 s |
| `-MeshTest keep:0:4` (hash ends in 40: 6 mesh shaders) | the bisect's bucket, alone | hung, 2 runs |
| `-SkipPipelines` of those six | a single bucket as the cause | hung, 65 s |
| `-MeshTest only:` the four `MS_VS_CascadeShadows_draw` shaders | the simplest family: depth only, one position, 628 lines of DXIL | hung, 78 s |
| `-SkipPipelines` of those four | | hung, 68 s |
| shadow shaders only, a barrier on all work and memory after each dispatch (`VICTORY_MESH_FENCE=1`) | draws overlapping each other or other work | hung, 86 s |
| shadow shaders only, dispatches with zero groups (`VICTORY_MESH_ZERO=1`) | the pipeline's state and bindings alone | **survived** 200 s |

What follows from it:

- **It is the mesh shaders running, not any one shader.** Any subset of them stops the card: the four
  shadow shaders alone, everything but them, the bucket `40` alone, everything but it. The first
  unfinished command of the device-removed data is a `DispatchMesh` (1 group of `45ace4127ac61cfd` in
  two runs, with the dispatches before it done), an `ExecuteIndirect` of an indirect mesh draw
  (kind 10), or the `CopyTextureRegion` that follows the barrier after such draws (the barrier before
  it had finished, so the copy itself, or what it waits for, is what stopped); the list position is
  the same in every run that draws the same shaders (command 27 of the list with
  `MS_VS_BreakableGlassNoCrack` in five of the bisect's runs).
- **Not the load**: 1,500 to 8,400 mesh groups a frame (the largest dispatch 216 groups), no
  refusals by the indirect budget. Not rasterisation (one pixel), not the amount of overlap (a
  barrier after every dispatch), not the order of events (no relay), not the FSR4 passes, not the
  sampler heap, the card copies, the heap's `getDimensions`, nor anything let go early.
- **Not the pipeline's state alone**: with zero groups everything is bound and set, and the run lives.
  (The driver may skip an empty dispatch altogether, so this does not rule out the state changes that
  go with a draw that runs.)
- **The debug layer** (191 messages in the run) found three things, none the cause (the run with all
  three put right still hung): `SetComputeRootSignature` before any sampler heap was bound, 114
  times (id 1419; `VICTORY_FALLBACK_HEAPS=1` binds a heap of one slot); 59 root SRVs of "title buffer
  ... on the card" read while still in `COPY_DEST` after the copy that fills them (id 538;
  `VICTORY_CARD_BUFFERS=0` keeps the copies in the PC's memory); and one vertex draw whose viewport
  (1920x1080) is larger than its target (1280x720; id 1378).
  (Four compute pipelines of wave size 64 are refused by the driver, as expected.)
- **GPU-based validation does not run the title**: with `VICTORY_D3D12_GBV=1` the title crashed 80 s in
  (`victory-image-....dll+0x23cf683`, a write to address 0), before any validation message about a
  shader.
- **The standalone test** `crates\victory-gpu\tests\mesh_fuzz.rs` (`cargo test -p victory-gpu --test
  mesh_fuzz -- --ignored --nocapture`, parameters in its header) dispatches the translator's own DXIL
  of such a shader (`MESH_DXIL=<file>`, from `VICTORY_EMIT_DUMP` of `victory.exe translate
  --validate`) on this GPU with the bindings the host gives it. It did not stop the card in any of: 40,000
  lists of 40 one-group dispatches over each of garbage, small values, zeros and ones (6.4 million
  dispatches); 20,000 lists with each of nine kinds of view in the tables (the right ones, never
  written, a null texture, a null buffer, a texture, typed, structured, unordered access, kinds
  crossed); the buffers in write-back PC memory with 4 and 12 CPU threads rewriting them while the
  card reads; vertex draws between the mesh dispatches (root signature and pipeline switches, the
  input assembler root flag); lists 250 ms apart for 100 s, and 120 ms apart for 65 s with 12 more
  CPU threads busy. So the shader, its bindings, the data in them, the memory kind and the CPU's
  writes are not enough. What the title's run has on top is not known (see below).

Frame cost with the mesh draws skipped (world, 3 fps): 275 to 337 ms a frame, of which 43 to 74 ms
preparing, 85 to 122 ms on the GPU and 73 to 145 ms copying back 41 to 44 MB of what the GPU wrote;
17 GB private. The copy-back and the GPU time are where a frame goes, not the CPU's threads.

The three launcher profiles in that stable mode (one run each, 220 s, 5-second statistics of the
world's frames, 2026-11-27 14:01 to 14:13; the scene is the same, the numbers vary between runs by a
third either way):

| Profile | Card buffers | ms a frame, median (mean) | RAM free at the end |
|---|---|---|---|
| Lean | 128 MB | 545 (801); copying back 303, preparing 288 | 1.8 GB |
| Balanced (default) | 512 MB | 136 (294) | 2.1 GB |
| Fast | 1024 MB | 125 (174) | 1.0 GB |

Lean is the slow one: with fewer title buffers on the card the CPU compares and copies them instead.
Fast is a little quicker than Balanced and leaves the PC 1 GB, which a browser can take: Balanced stays
the default.

## What was taken from VIctory-NVIDIA (2026-11-27)

[VIctory-NVIDIA](https://github.com/trustno1-coder/VIctory-NVIDIA) is another fork of the same
host with a few months of its own fixes (GPL-3.0, like this one; the commits keep their original
authors). Its history was read commit by commit; what was taken, each with the tests that came with it:

| Commit here | What it does | What its author found (on their GPUs) |
|---|---|---|
| `cb109a3` (from fxceless/victory-fork#1324) | a dynamic descriptor slot goes back only to the host heap that handed it out (every heap has an id) | when a title frees a heap before the lists using it are reset and a new heap lands at the same address, resetting the old list freed the new heap's slots; two live lists then overwrote each other's descriptors (seen on NVIDIA) |
| `85efdc9` | the buffer comparisons are submitted as their own list after the state decay | title buffers that were promoted implicitly can decay to COMMON first: fixes D3D12 error 538 |
| `c2a8e69` (from fxceless/victory-fork#1266) | raw (byte-address) views stay inside their buffer: a range past the end is shortened, sub-dword and unaligned ones are fixed (`VICTORY_RAW_VIEW_CLAMP=0` restores the old behaviour) | the run's log now counts the cuts (`raw view clamp ...`): **0 in the world on this machine so far**, so on this GPU it is a guard, not a cure |
| `0ead72a` | a pipeline with static samplers is not loaded from the pipeline library (`VICTORY_LIBRARY_STATIC_SAMPLERS=1` restores) | a stored pipeline keeps the sampler-table places of the run that stored it; loaded in a run that made its samplers in another order it samples through other ones. On their 4070 SUPER that gave posterized light (the composite pass reading its colour-grading tables unfiltered). **Lighting, so worth having here too; not yet looked for on this GPU's screen** |

None of these cured the mesh hang (a run with mesh draws on hung at 74 s, as before). They are
in because they are correct and cost nothing measurable; they are not claimed as the fix.

Not taken, deliberately:

- **Downloading runtime DLLs** and the DLSS / frame-generation paths: NVIDIA-only, and a binary
  fetched at run time is not something to add to a repository that ships no binaries.
- **The wave-32 lowering** (`4f49709`, `89cfae2`, `eeb17cc`, `854f57f`, `3bcc96f` there): it is
  what would let the 28 wave-64 compute programs run on this GPU, but it conflicts deeply with the
  translator here, and their widened water programs left dark tiles on their own screenshots.
  Porting it blind would trade missing effects for wrong ones. Still to do, with a check against
  the offline survey (`victory.exe translate --validate`) after each step.
- Their launcher's settings are not copied wholesale: `scripts\intel-sweep.ps1` has them as
  experiments (`nv-all`, `nv-noawake`, `ambient`) so they are measured here before any is a default.

## What the screen shows, from the captured frames (2026-11-27 15:35, Balanced, mesh draws skipped)

`play-intel-lowmem.ps1 -Capture` (or `intel-sweep.ps1 -Capture`) saves the frames the host presents as
PNG files in `logs\frames-<time>\` (half size: a frame at every step the log reports, and the world's 2nd,
6th, 30th, 80th and later ones; diag build). One run, 230 s, 3,190 frames, about 630 of them in the world
(it began at frame 2,556, the first of 500 draws or more):

- **Textures that are drawn are there and sharp.** The loading-screen art (the flamingo cards, the
  key art with the house and the car) comes out in full detail at 960x540 (half of 1920x1080), the title
  cards' lettering is clean, nothing is pink, black or stretched.
- **The 3D scene is the part that is missing, not its textures.** The world starts with the title's
  night-time opening scene: the distant lights and the hair strands are drawn, but no head or body can
  be seen around the hair and the ground is dark noise (the frames are dark, so read this loosely).
  The op list of one world frame (`frame-2796-ops.txt`, 4,629 lines) has
  **898 mesh draws dropped (`VICTORY_MESH_TEST=skip`) against 34 ordinary vertex draws**, and 210 stages
  of the ray-tracing structure builder dropped (RT is off). So about nine draws in ten of a world frame
  are mesh draws, and with them off there are few surfaces left to put a texture on. Until the mesh
  hang is solved, "all textures show" cannot be checked on the scene itself.
- **Textures wait for memory in the world.** In that run: the card at 100 % of the budget the host
  allows itself (10,136 MB of 10,136 MB), 32,268 texture slots uploaded (1,376 MB), 18,822 waiting for
  room, 16,006 passed over because the title had unmapped their memory, and the 3,108 that had to wait
  for memory were shown 42 s late on average. The room is the PC's RAM: free RAM was 20 GB before the run
  and fell as low as 0.2 GB (the process peaked at 21.5 GB in memory). About 8 GB of the PC was the
  desktop's (Edge 2 GB, the Claude app 2 GB, Defender, Windows; measured in another moment). Closing
  programs before the run is the one thing that gives textures more room.
- **A limit worth knowing about.** With `VICTORY_TEXTURE_MAX` unset the host caps a streamed texture's
  longer side at 1024, at 512 from 92 % of the card's budget and at 256 from 97 %. Here the budget is what
  the process uses plus the free RAM less the 3 GB reserve, so once free RAM is under about 3.4 GB it is at
  100 % and every texture *made then* is 256 pixels at most (the ones made earlier keep 1024). The sweep
  has `tex-512` and `tex-1024` to set it outright; the default is unchanged, because with no meshes on
  screen there is no scene to judge the sharpness on, and more pixels is more RAM that is not there.
- **One run in eight never leaves the front end**: the title sits on "ACCOUNT PICKER (Y) / PLAYER (A)" and
  waits for a button. The keyboard is the stand-in pad: **Enter is A**, Esc is Menu, Tab is View, X and Y
  are X and Y, the arrows are the D-pad (`gameinput.rs`). Press Enter on that screen; the sweep reports
  such a run as `no world`.

Newer Intel drivers: the release notes of 32.0.101.8991 (2026-08-25) list no mesh-shader, DX12 hang or
device-removal fix for Core Ultra Series 3 (their known issues name three games only); those of
32.0.101.8974 could not be fetched (HTTP 403). So a driver newer than 8724 is not known to cure the hang,
and no run here used one.

## Not known yet

- What the title's run has that the standalone test has not, and the hang needs: several queues at
  once (the copy queues for textures and uploads), the real depth and colour targets (`D32_S8`,
  `R16G16B16A16F` at 1280x720) and their pipelines' state, the pixel shaders, thousands of different
  pipelines, real meshlet data, a view heap with a million views written, memory at 17 GB. The next
  step is to add them to `mesh_fuzz.rs` one at a time, or to capture the list that stops
  (`docs/replay.md`: too large in the world so far) and replay it standalone.
- Whether a newer Intel graphics driver (8724 is from 2026-04) takes mesh shaders that this one does
  not. The device-removed data names no page-fault address on this vendor.
- `MS` programs write the declared maxima of vertices and primitives every time
  (`SetMeshOutputCounts(128, 128)`, then culled triangles for the lanes past the real count): the
  console's own counts could be passed instead, which also saves the GPU the empty primitives; whether
  it matters here is untested.
- Whether the process itself or other programs took the RAM (the `.mem.csv` of the next run).
- Which title memory kinds block `-InPlace` (`-Probe` logs a census 30 s and 150 s in).
- Whether any change to the buffer-copy pass is needed or safe.
