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
  and fell as low as 0.2 GB (the process peaked at 21.5 GB in memory). **That run overlapped a stress
  test another session was running on the same PC (some 6.7 GB and the same GPU), so the memory and
  waiting figures here are worse than a run alone would show; the picture and the draw census are not
  affected. Repeat it alone for clean figures.** About 8 GB of the PC was the desktop's (Edge 2 GB, the
  Claude app 2 GB, Defender, Windows; measured in another moment). Closing programs before the run is
  the one thing that gives textures more room.
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
## The stress test with the title's load added (2026-11-27, evening)

`crates\victory-gpu\tests\mesh_fuzz.rs` has a second test, `the_titles_load_around_a_mesh_shader`
(`mesh_fuzz\load.rs`, `mesh_fuzz\hlsl.rs`; the switches are tabled at the top of `load.rs`), run one setting
after another by `scripts\intel-fuzz.ps1` (`-List` shows them; every run starts a copy of the test named
`victory.exe`, and the script holds that name with a placeholder for the whole list: `intel-sweep.ps1` refuses to
start a game run next to a process of that name, so no game run starts on the card meanwhile). Reading the
shadow shader's DXIL and the first test's setup showed what that test had not been doing:

- **The root constants.** The shader reads only words 9 (the table's place in the heap) and 12 (bit 28 picks a
  typed buffer through the heap by the word's low 10 bits). The test left word 12 at 0, so that path never ran.
- **The slot it read.** The shader reads the meshlet view at the table's slot + 1; the test wrote its first view at the
  table's slot, so that slot was never written.
- **Real triangles.** Over garbage the translated shader rasterises next to nothing. The new test has a mesh
  shader of our own (`hlsl.rs`: 128 vertices, 128 triangles a group, real depth, a position, a texture
  coordinate and a normal) drawn with the title's targets.
- **Everything else on the list**: depth and stencil formats (`D32_S8`, `D24_S8`, `D32`, `D16`), four colour
  targets with a pixel shader, clears, the copy-back of the targets into write-back memory (and with
  `MESH_COPYBACK=3` the host's depth-to-colour-twin copies, a stencil plane through a buffer, both ways), lists
  in flight, a copy queue, a compute queue, textures made, copied into and released, a CPU rewriting the heap
  (also the descriptors the draws read), many pipelines (states and programs of our own, and with
  `MESH_DXIL_DIR` hundreds of the title's translated programs), `ExecuteIndirect` of mesh dispatches after the
  host's kind of clamp pass, the million views written, memory held by the test, views that claim more than their
  buffer holds, the CPU streaming memory.

What it found: **no setting that ran to its end stopped the card.** 120 s each unless noted; the totals are the last
line of each log (the test counts dispatches, 40 a list):

| Setting | Adds | Result |
|---|---|---|
| `plain` | the first test's work in this harness (translated shadow shader over garbage) | no hang, 9.9 M dispatches |
| `user` | root constants that vary by draw | no hang, 9.0 M |
| `own` | our own program with real triangles | no hang, 7.4 M |
| `targets` | `D32_S8`, 4 colour targets, a pixel shader, clears, copy-back, 3 lists in flight | no hang, 2.2 M |
| `queues` | a copy queue and a compute queue at the same time | no hang, 5.1 M |
| `heap` | textures made and copied, 141 million descriptor rewrites, all million views written | no hang, 5.0 M |
| `race` | also the descriptors the draws read, rewritten while read | no hang, 5.1 M |
| `pipes` | 780 pipelines (60 states of 13 programs) | no hang, 3.8 M |
| `copies` | `MESH_COPYBACK=3` and stencil writes: each colour target copied to a texture of its own, and the depth copied to its colour twin and back after the draws (`D32_S8`: the stencil plane through a buffer, both ways) | no hang, 3.6 M |
| `stats` (40 s) | `targets` with the card's own count of what it ran (pipeline statistics of each list) | no hang, 1.7 M dispatches; at 30 s the card had run 1.37 billion mesh invocations, as many primitives and 36 billion pixels (45 M mesh invocations a second): the dispatches do work |

The progress lines of every other setting print the same "the card ran" column; it reads 0 there because the query is off
(`MESH_STATS=1` turns it on), not because nothing ran.

Cut short or never run, so **no result**: `copies-2` (a depth-stencil array of two slices, so that two twin copies sit
between a pair of barriers as in the stuck lists) and `dred-plain` (the plainest setting with the test's own auto
breadcrumbs) were stopped from outside after 30 of 120 s and 60 of 100 s; neither log has an error or a hang report, the
log just ends. `mixed` (vertex draws between the mesh draws, indirect executions in batches, the two slices), `copies-d24`,
`copies-d32`, `batch` (60 single-record `ExecuteIndirect`s in a row), `indirect`, `shaders` (up to 300 of the title's
translated mesh programs in their own pipelines; the folder made for it had 294, from `victory.exe translate --validate
--list` with `VICTORY_EMIT_DUMP` and `VICTORY_TRANSLATE_ONLY=<hashes>`), `oversize` (views claiming 256 MB more than their
buffer holds), `pressure` (12 GB held), `memload` (the CPU streaming memory), `big` (dispatches of up to 216 groups),
`dred-mixed` and `all` have no log: they compile, pass clippy and were not run.

## The title's own runs, bisected (game, 2026-11-27 15:40 to 16:11, driver 8724, mesh draws on)

`scripts\intel-sweep.ps1 -Hold` (new: `-Hold` keeps the card against other launchers' sweeps, `-Repeat n`, `-Only` in
the order given) with the settings `base-dred`, `exact`, `only-ea2e`, `skip-ea2e`, `ind-zero`, `ind-fence`, `all-zero`
(`-List`). One run each, 220 s:

**A caveat on four of these runs.** Another session working on the same machine reported afterwards that it compiled
(`cargo check`, `test` and one 37 s `build`) at times between 15:57 and 16:08, and ran a one-group mesh unit test on the
hardware device at 16:12. The runs inside that window may have been perturbed by it (the rule on this machine is: no compile
while a run is going): `ind-fence` (15:55:58 to 15:58:09, its last minute), `ind-zero` (15:58:32), `all-zero` (16:00:20) and
`dred-off` (16:04:48), and the stress runs `stats`, `copies-2` and `dred-plain` above. The runs before it (`base-dred`,
`exact` x3, `only-ea2e`, `skip-ea2e`: 15:40 to 15:54) and the one with the twin patch (16:10) were outside it, by that
session's account. A hang in a perturbed run
does not show that what the setting left in place is the cause, so the conclusions that rest on those rows are marked below
and need the runs repeated on a quiet machine.

| Setting | What it does | Result | First unfinished command (device-removed data) |
|---|---|---|---|
| `base-dred` | mesh draws on, `-Dred full` | hung 101 s | `CopyTextureRegion` 34 of list 209 (63 commands), after a barrier |
| `exact` (3 runs) | `VICTORY_MESH_COUNTS=exact` | hung 89, 87, 72 s | `CopyTextureRegion` 31 of 46; then twice `ExecuteIndirect` 107 of 160 (`[mesh ea2eadde55090b27, pixel 3711fdad456538a1]`, 14 s after that pipeline's first bind) |
| `only-ea2e` | only that mesh program's draws | **survived 220 s** | |
| `skip-ea2e` | every mesh draw but that program's | hung 68 s | `ExecuteIndirect` 48, `[mesh 85eaffa59a3a6ecf, pixel b352dd911b665364]` |
| `ind-fence` | a barrier on all work after every indirect mesh execution | hung 131 s (its last minute in the window) | `CopyTextureRegion` 44 of 68, after a barrier |
| `ind-zero` | every indirect mesh execution issued with a count of 0 | hung 73 s (in the window) | `CopyTextureRegion` 31 of list 244 |
| `dred-off` | mesh draws on, `-Dred off` (no auto breadcrumbs at all) | hung 90 s (in the window) | none kept ("no breadcrumbs"); the device was removed after 11 submissions sent ahead |
| `base`, the depth twins made to wait for their mapping | mesh draws on; `resource.rs` waits with `host::mapped` after `resident` for the two twin textures of a depth texture (see below); not kept | hung 81 s | `CopyTextureRegion` 43 of list 290 |
| `all-zero` | every mesh draw empty: direct dispatches of 0 groups, indirect counts of 0, all state and bindings as before | **hung 68 s** (in the window) | `CopyBufferRegion` 58 of 60: the 16-byte copy of the card budget's notes into mapped memory, the last commands of a list (`card_budget_out`), between two barriers |

What it says:

- **The stop probably does not need a mesh shader to run (one run, to repeat).** With every mesh draw empty the card still
  stopped (`all-zero`, in the window above; `ind-zero` agrees, also in it), while the standalone test, which runs 45 million
  mesh invocations a second, never stopped it. If it holds, what the host does for the draws is left: pipelines, root
  signatures, descriptors, the refresh and copies of what they read.
- **The first unfinished command is, in 6 of the 9 reports of that evening (the 8 runs of the unpatched build above and
  the 14:54 run of another; the run with the twin patch also stopped on a `CopyTextureRegion`), a copy that the host records, right after a `ResourceBarrier` that did finish** (so every draw
  before it had finished): `CopyTextureRegion` (5; two between a pair of barriers each time), `CopyBufferRegion`
  (1). In the other 3 it is a mesh `ExecuteIndirect`. The copies are the host's own (a depth texture's twin
  through a buffer, a texture copied back, the card budget's notes); none is the title's (those carry a marker
  text in `-Dred full`; the report says "no marker" for these).
- **One mesh program alone does not do it; any larger set does** (`only-ea2e` lived, `skip-ea2e` died on another
  program): the program stuck in two runs is where the 59th of 109 `ExecuteIndirect`s in a row (no barrier between,
  how the host's batched clamp leaves them) fell, not a program that is wrong.
- Not the output counts (`exact`: 3 of 3 hung, before the window). Probably not the overlap of indirect draws (`ind-fence`)
  and not the device-removed data itself (`dred-off` hung with the auto breadcrumbs off), but each of those is one run, in
  the window.
- **A lead in the host's code that was tried and was not it.** `host::mapped` says new card memory is mapped into the
  card's address space after `MakeResident` returns, a GPU access before that is a page fault (two losses of 2026-11-25),
  and the host waits with a copy on a queue of its own. The 45 depth textures' twins (`resource.rs`) are made resident
  but not waited for until mapped, and the stuck copies are the host's first uses of such twins, so the twins were made
  to wait (a patch, run once, not kept): the run hung all the same. The depth twins are why mesh draws matter to the
  copies: the title's depth-only shadow draws are mesh draws, and each depth texture they write is copied into its twin
  and back at the list's end, empty draws included (`twin_use`, `twins_to_depth`); with the draws skipped no depth texture
  is written by them and, by the code, none of those copies is recorded (not measured).

## What a stopped run of the title says (the report of a run of 2026-11-27 14:54, mesh draws on, 74 s)

The log of a `-MeshTest off` run (device removed data on, `-Dred light`) names the command that did not finish:
list 169 of 63 commands, 40 done, the last of them a `ResourceBarrier` (op 15), the first not done a
**`CopyTextureRegion` (op 8)**; the list binds seven mesh pipelines with pixel shaders (the title's skinned
character and vehicle programs: `MSNC_VS_PedTransformSkinAlpha`, `MSR_VS_TransformSkin`, `MS_VS_Transform`,
`MSNC_VS_WindshieldWiperMask`), a compute pipeline and two vertex pipelines. The other lists never started. Free RAM
was 1.0 GB, but runs with the mesh draws skipped went down to 0.1 to 1.0 GB free and lived: not memory.

The host copies depth textures into colour twins and back (`record_twin_copy`, `twin_use`: the title writes
depth textures with compute, so each has a colour twin; a depth buffer with a stencil plane goes through a
buffer, `CopyTextureRegion` both ways). The same run's log made 45 such depth textures between 20:55:20 and
20:55:28, 19 s before the device went: 43 of format 53 (`R16_TYPELESS`: cube and spot shadow maps of 128 to
1536 texels), one 1280x720 of the console's format 118 and one 1920x1080 `R32_TYPELESS`. A `CopyTextureRegion` of
a depth or stencil plane after mesh and pixel shaders wrote the same buffer is the pattern `MESH_COPYBACK=3`
adds to the test (the console's 16-bit depth with stencil has no PC format: `d32s8`, `d24s8`).

## Exact output counts (`VICTORY_MESH_COUNTS=exact`)

The console's mesh programs ask for their outputs with one lane's `s_sendmsg` GS_ALLOC_REQ (m0 = vertices |
primitives << 12, made by a few scalar instructions from the meshlet's header; `(w & 127) + 1` and
`((w >> 7) & 127) + 1` in the shadow shader, 1 and 1 for an invalid meshlet). DXIL wants one
`SetMeshOutputCounts` in code every thread runs, so the translator declared the maxima (128, 128) at the start
and culled the primitives past the real count (`mesh_prologue`). With `VICTORY_MESH_COUNTS=exact` the
translator finds the one request in a block only lane 0 runs (`hoistable_alloc`), takes the instructions that make
m0 (scalar moves, logic, shifts and adds over values the block before it holds) and lowers them at the end of that
block (`hoist_counts`) for `SetMeshOutputCounts(vertices, primitives)` (never more than the maxima); the
primitives up to the count start culled as before. A program of another shape keeps the maxima.
Off by default; part of the shader cache's key. Offline, on the title's files: of 294 mesh programs (every 30th by
name, 300 hashes; `victory.exe translate --validate --list`) all 294 change, and the same 291 validate as without it
(the three that do not are the 32 KiB outputs `-MeshStrict` refuses); the shadow shader's DXIL has the counts
before the lane-0 branch. **In the game it changed nothing**: three runs with it and mesh draws on hung at 89, 87 and
72 s (table above). It stays off; it is kept as an experiment, and the offline check above shows the hoist is well
formed.

## Not known yet

- **What the card stops on.** Not the output counts (3 of 3 runs, unperturbed) and not one program. Probably not the mesh
  shaders' work (empty draws stop it), not DRED and not the overlap of indirect draws, but each of those three is one run made
  while another session compiled (`all-zero`, `dred-off`, `ind-fence`). What the reports show is a copy the host records at
  the end of a list, right after a barrier, that does not finish, once mesh draws have been prepared in the list (or a mesh
  `ExecuteIndirect` in a long run of them).
  Next steps: first repeat `all-zero`, `dred-off` and `ind-fence` three times each on a quiet machine
  (`scripts\intel-sweep.ps1 -Only all-zero,dred-off,ind-fence -Repeat 3 -Seconds 220 -MinFreeGB 16.5 -Hold`, about half an
  hour of the card); then `-Dred full` markers on every host copy (twin, copy-back, card budget; `host.rs` has them for the first two now)
  to name the stuck one and its resources; a run with the depth twins switched off (`VICTORY_DEPTH_TWINS=nodepth` removes
  only the depth plane's); an Intel driver newer than 8724 (Intel's download page lists newer 32.0.101 builds, 8991 WHQL and 8992 non-WHQL;
  none is installed here); a report to Intel naming the copy once it is known.
- What the title's run has that the standalone test has not. Run to the end, one setting at a time and 120 s each
  (`scripts\intel-fuzz.ps1 -List`): a copy queue and a compute queue, the title's targets with a pixel shader, 780
  pipelines, a heap rewritten while read, the twins' copies of a `D32_S8` depth; none stops the card, so the hang does
  not need those. Written but not run (or run only 20 s, `smoke`): indirect executions, memory held near 17 GB, views
  past the end of their buffers, vertex draws between the mesh draws, the title's own translated programs, a
  two-slice depth-stencil twin, and everything together (`all`). Not in the test at all: the host's copies of title memory
  into card buffers while the draws read them, and real meshlet data (the stuck lists can be captured with
  `docs/replay.md` only below 17 GB).
- Whether a newer Intel graphics driver (8724 is from 2026-04) changes it; the device-removed data names no
  page-fault address on this vendor.
- `MS` programs wrote the declared maxima of vertices and primitives every time; with
  `VICTORY_MESH_COUNTS=exact` they pass the console's own counts, and the card stopped all the same (see above).
- Whether the process itself or other programs took the RAM (the `.mem.csv` of the next run).
- Which title memory kinds block `-InPlace` (`-Probe` logs a census 30 s and 150 s in).
- Whether any change to the buffer-copy pass is needed or safe.

## A compute twin for the mesh draws: the study (2026-11-27)

Question: can the title's mesh draws be drawn on this card without the driver's mesh pipeline, which stops
the card (26 of 27 runs that issued them), so that the nine draws in ten of a world frame that are missing
come back? The idea: the same translated mesh program, built as a **compute twin** that writes its vertices
and primitive indices into a buffer, then a vertex shader that reads that buffer (`SV_VertexID`) with the
title's own pixel shader, one non-indexed draw per mesh draw. Everything below is offline: the title's files,
the translator, and the logs of runs that did issue mesh draws; no game run was made for it.

### What the world frame needs

- Frame 2796 of the capture of 15:35 (`frames-20261127-153511\frame-2796-ops.txt` in `intel-overlord`, 4,629
  op lines): 898 `DROPPED indirect kind 10` (`VICTORY_MESH_TEST=skip`). The title's indirect executions are
  all **one record, no count buffer** (`indirect kind N max 1`: 173 kind 2 and 9 kind 1 in that frame, 87 and
  77 in frame 3056): a mesh draw is one `DispatchMesh(x, y, z)` whose `x` a GPU pass wrote, and nothing the
  title declares bounds `x`. The op list does not say which mesh shader a dropped draw had, nor its groups.
- Groups, from the "indirect mesh dispatches" statistic of the runs that did issue them (13 one-second lines
  in two runs, world frames: `victory-20261127-154020.log` and `intel-overlord`'s `145435`): **1,967 to
  10,461 groups a frame** (median 6,154), the largest single record **449 groups** (median of the per-second
  largest 228). That is about 7 groups a draw over 898 draws. The host's clamp runs a record of more than
  65,535 groups empty, and a list's budget is 2^27 threads, 2^20 mesh groups.
- Per group, from the 489 mesh programs the logs show the title creating pipelines for (564 pipelines,
  470 with a pixel shader, 94 depth-only; translated offline with `VICTORY_TRANSLATE_ONLY=<those hashes>`
  and `VICTORY_EMIT_DUMP`, read with `scripts\mesh-census.py`): **all 489 are 128 threads, 128 vertices, 128
  primitives**, no per-primitive outputs. A lane writes its own vertex and its own primitive and the
  translator always declares the maxima (`mesh_prologue`), so a group's output is a fixed block. Vertex
  scalars per vertex: 4 to 68, median 25; 84 programs (17 %) write a position alone; 27 write a clip distance.

| What one thread group writes (128 vertices as 4-byte scalars + 128 triangles as 3 x u32) | Bytes |
|---|---|
| smallest (position only) | 3,584 |
| median | 14,336 |
| mean | 13,972 |
| 90th percentile | 27,648 |
| largest (68 scalars) | 36,352 |

### The buffer

| | smallest shader | mean (13,972 B) | largest shader for every group (36,352 B) |
|---|---|---|---|
| a frame of 1,967 groups | 7 MB | 27 MB | 72 MB |
| the median frame, 6,154 groups | 22 MB | 86 MB | 224 MB |
| the largest frame seen, 10,461 groups | 38 MB | 146 MB | 380 MB |
| the largest record seen, 449 groups | 1.6 MB | 6.3 MB | 16.3 MB |
| a record at the clamp's limit, 65,535 groups | 0.23 GB | 0.92 GB | 2.4 GB |
| a list's budget, 2^20 groups | 3.8 GB | 14.7 GB | 38 GB |

- **A worst case sized from what the title declares does not exist**: no count buffer, no maximum; the
  groups are whatever a GPU pass wrote, up to the clamp. A static buffer for the clamp's limit is 0.9 to
  2.4 GB a draw, for the list's budget 15 to 38 GB. No-go as a design: the buffer has to be sized by us.
- **The memory is the PC's**: the process is 17.5 to 19.5 GB private in the world, and the free RAM of the 3
  world runs with a timeline went down to 0.2, 0.3 and 1.0 GB of 31.4 GB. A buffer for a whole frame (146 MB
  for the largest frame seen, 380 MB at the ceiling) would be a sixth to all of that floor, and the card's
  texture budget comes from the same free RAM. A buffer **reused by each draw** (the twin's dispatch, a UAV
  barrier, the draw that reads it, a UAV barrier, the next draw) needs only the largest single record: 16.3 MB
  at the largest shader, so a **32 MB arena** (923 groups at the largest shader, 2,401 at the mean; a draw
  that does not fit is left out and counted) is 3 to 16 % of that floor. The vertex shader can read the arena
  through a root UAV, so it stays in the unordered-access state and only UAV barriers are needed.
- **Allocation is on the GPU**: the group count is in GPU memory, so the host's clamp pass (which already
  reads that record) would also write the twin's two argument records (`Dispatch(x, 1, 1)` and
  `DrawInstanced(x * 384, 1, 0, 0)`) into the scratch buffer the executions read, with no extra pass or state.
  No compaction is needed: a draw owns the arena from offset 0, and a primitive a lane culls is three zero
  indices (degenerate, no pixels).
- **Traffic** at the median frame: 86 MB written by the twin and 236 MB read by the vertex shader (6,154 x 384
  corners, about 100 bytes each, no post-transform reuse), against the 275 to 337 ms the world frame takes
  without mesh draws: a few ms of memory bandwidth. The cost is the commands: about 5 more per mesh draw
  (clamp, dispatch, draw, two barriers), about 4,500 a frame.

### Attributes against the title's pixel shaders

- Every one of the **470 pipelines with a pixel shader** the logs show links: each pixel input that is not a
  rasteriser value is an output of the same mesh program with the same semantic, index, register and mask
  (`scripts\mesh-census.py`: 0 not covered). A vertex shader that declares the mesh program's own output
  signature and passes the values through links the same way; the pixel shader is the title's, unchanged.
- The pixel shaders' system-value inputs are all the rasteriser's, as in a mesh pipeline: position (442 of 442
  programs), barycentrics (224), front face (105), clip distance (30). None reads `SV_PrimitiveID`.
- Winding and provoking vertex are the draw's: the indices are the mesh program's own triples, and the
  rasteriser state is the pipeline's.

### What the translator and host have to do

- Translator (`victory-shader`, `emit/lower.rs`): lower the same program as compute (`numthreads(128,1,1)`,
  the lane and group ids it already has) with the export writing into a raw UAV block of the arena instead of
  `storeVertexOutput`/`emitIndices` (vertex `v` of group `g` at `g*128+v`, 4 bytes a scalar in `OSG1` element
  order; the indices as arena vertex numbers; a culled primitive as zeros), and a vertex shader built from
  the same element list (`stub_vs` in `emit/host.rs` and `rect.rs` already build shaders from nothing). About
  35 places read `s.mesh.is_some()` and mean "lanes are the console's, exports are per lane".
- Host (`victory-gpu`): a compute pipeline and a vertex+pixel pipeline made beside the mesh pipeline from the
  title's graphics state (`pipeline/host.rs` makes both kinds already), the arena, the clamp writing the two
  records, and the bind of the mesh stage's resources as compute: `bind(list, p, compute)` takes the title's
  state from `self.compute` or `self.graphics` by the same flag that picks the `SetCompute*` call, so the twin
  can run with the graphics roots copied over the compute roots for its dispatch.

### Verdict

**Go, as a staged prototype**: the memory (with an arena a draw reuses), the matching and the command count
do not stop it; the translator and host changes are about a thousand lines in two crates. One thing the
figures cannot say, and it decides whether the drawing is worth finishing:

1. **Does the twin avoid the hang?** Stage A: the compute twin of a few shaders runs for every mesh draw of
   those shaders and writes the arena, nothing is drawn. If the card still stops, it is the shaders' code or
   what they read, not the driver's mesh pipeline, and stages B and C are not worth building (that is a
   finding for the session hunting the cause too). If it lives for 200 s in the world, the mesh pipeline is
   the suspect.
2. Stage B: the vertex shader and the draw for the same shaders; the first captured frames are compared with
   the current ones (`intel-sweep.ps1 -Capture`).
3. Stage C: all shaders, and direct dispatches (`x` known on the CPU).

Not known: whether stage A lives (the whole point); how the groups divide between shaders in a world frame
(the op list does not name the shader of a dropped draw: it takes a run that issues them); whether some mesh
programs use group-shared memory or barriers in a way a compute lowering breaks (all 489 are 128 lanes, and
`mesh_prologue` already refuses cross-lane operations); the cost of the barriers per draw.
