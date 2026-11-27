//! The title's load around a mesh shader, a piece at a time.
//!
//! `a_translated_mesh_shader_over_garbage` (the parent file) dispatches the translator's DXIL on one
//! queue with idle gaps and never stopped the card, where every run of the title that issued mesh draws did
//! (`docs/intel-arc.md`). This test keeps what that one does and adds, each by a switch, what the title's
//! run has and it lacked:
//!
//! | switch | adds |
//! |---|---|
//! | `MESH_USER=fuzz` or `sane` | root constants that vary by draw (the shaders branch on them: word 12 bit 28 of the shadow shader picks a typed buffer picked from the heap by the word's low bits) |
//! | `MESH_OWN=1` | a mesh shader of our own that draws real, valid triangles (the translated one, over garbage, rasterises next to nothing) |
//! | `MESH_DEPTH=d32s8`, `MESH_RTS=n`, `MESH_PS=1`, `MESH_CLEAR=1` | the title's depth format, `n` colour targets (the formats of its main pass) with a pixel shader, clears |
//! | `MESH_COPYBACK=1`, `2` or `3` | the colour targets (with 2: the depth and stencil planes too) copied into write-back memory after the draws, as the host's relay does; the CPU reads them a list later. With 3 each colour target is also copied to a texture of its own, and the depth goes to its colour twin and back (a depth buffer read as a colour texture: the host keeps such a twin and copies one into the other, a stencil plane through a buffer; the unfinished command of a run of the title that stopped the card was a `CopyTextureRegion` after a barrier, in a list that had run mesh and pixel shaders) |
//! | `MESH_DEPTH=d32`, `d32s8`, `d24s8`, `d16` | the depth format (the title's main depth buffer has a stencil plane: the console's D16 + S8, which no PC format is) |
//! | `MESH_SLICES=n` | the depth buffer is an array of `n` slices (a stencil plane then has `n` subresources: the host's twin copy of it is one `CopyTextureRegion` for each, two between a pair of barriers, as in the stuck lists of the title's runs) |
//! | `MESH_STENCIL=1` | stencil writes in every pipeline (a reference value each draw), as the title's passes that mark material classes do |
//! | `MESH_INFLIGHT=n` | `n` lists in flight, not one waited for at a time |
//! | `MESH_QUEUES=copy,compute,stream,churn,race` | a copy queue's traffic, a compute queue's passes, textures created, copied into and released, descriptors rewritten by the CPU (with `race`: also those the draws read) |
//! | `MESH_STATES=n`, `MESH_SHADERS=n` | `n` pipeline states and `n` programs of our own: many different pipelines |
//! | `MESH_INDIRECT=n` | every `n`-th dispatch is an `ExecuteIndirect` of mesh dispatches, clamped by a compute pass first, as the host does |
//! | `MESH_BATCH=n` | with `MESH_INDIRECT`: each indirect run is `n` executions of one record each (`ExecuteIndirect` with a count of 1), one after another with nothing between them but new root constants and now and then another pipeline, as the host's batched clamp leaves the title's: the stuck lists of its runs had 109 such executions in a row |
//! | `MESH_VERTEX=n` | every `n`-th draw is a vertex draw (a triangle by vertex id, the same targets and pixel shader) between the mesh draws, so that lists switch between the two kinds of pipeline as the title's do (the stuck lists bound mesh and vertex pipelines) |
//! | `MESH_FILL=1` | every one of the million views written |
//! | `MESH_OVERSIZE_MB=n` | every buffer view (raw, typed, structured) claims `n` MB more than its buffer holds: what the card does with reads past the end of the memory behind a view (the host makes views of host copies that may be smaller than the extent a title's descriptor names) |
//! | `MESH_STATS=1` | the card's own count of what it ran (pipeline statistics of each list: mesh shader invocations and primitives, pixel shader invocations), printed with the progress: proof that the dispatches did work |
//! | `MESH_MEMLOAD=n` | `n` CPU threads that copy and compare big blocks of memory, as the host does with the title's buffers (gigabytes a frame), so that the card and the CPU share a busy memory bus |
//! | `MESH_DXIL_DIR=<folder>`, `MESH_DXIL_MAX=n` | the first `n` translated mesh programs of a folder (`..._Ms.dxil`, from `VICTORY_EMIT_DUMP` of `victory.exe translate --list`): hundreds of real pipelines; with `MESH_USER=sane` the user data stays inside the heap |
//! | `MESH_PRESSURE_GB=n` | `n` GB of this process's memory touched and held |
//!
//! Other settings: `MESH_SECONDS` (default 120), `MESH_DRAWS` (dispatches a list, 40), `MESH_GROUPS` (at
//! most this many groups a dispatch, 1), `MESH_MODE`/`MESH_MEMORY` as in the parent, `MESH_IA=0` (the
//! host's graphics root signatures carry the input assembler flag, so does this test's), `MESH_DRED=1`
//! (breadcrumbs, printed when the card stops), `MESH_DXIL=<file>` (the translated shader; without it only
//! `MESH_OWN=1` has a program to run).
//!
//! The card resets when it stops: the screen may flicker.

use std::mem::ManuallyDrop;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

use windows::Win32::Foundation::{HANDLE, RECT};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::Interface;

use super::hlsl;
use super::{HEIGHT, Rng, TABLES, VIEWS, WIDTH, env_u64, fill, upload_buffer};

/// Moves a D3D12 object to another thread (devices, queues, fences and resources are free-threaded).
struct Share<T>(T);

// SAFETY: see above; only D3D12 objects are put in one.
unsafe impl<T> Send for Share<T> {}

impl<T> Share<T> {
    /// The object, taking the whole wrapper (a closure that named only the field would not be `Send`).
    fn into_inner(self) -> T {
        self.0
    }
}

fn env_str(name: &str, default: &str) -> String {
    std::env::var(name).unwrap_or_else(|_| default.into())
}

/// Whether the comma list holds the word.
fn has(list: &str, word: &str) -> bool {
    list.split(',').any(|w| w.trim() == word)
}

/// The settings of a run, from the environment.
struct Opts {
    seconds: u64,
    draws: u64,
    groups: u64,
    inflight: usize,
    user_fuzz: bool,
    /// Random words that stay inside the heap and the buffers (the user data of programs not known well).
    user_sane: bool,
    memory: String,
    mode: String,
    rts: u32,
    depth: DXGI_FORMAT,
    depth_name: String,
    /// 0: no pixel shader, 1: every pipeline has one, 2: half of them.
    ps: u64,
    clear: bool,
    copyback: u64,
    queues: String,
    indirect: u64,
    vertex: u64,
    batch: u64,
    fill_views: bool,
    pressure_gb: u64,
    states: u64,
    shaders: u64,
    ia: bool,
    own: bool,
    stencil: bool,
    slices: u16,
    stats: bool,
    dred: bool,
    sleep_ms: u64,
    memload: u64,
    oversize_mb: u64,
    dxil_dir: Option<String>,
    dxil_max: usize,
}

impl Opts {
    fn from_env() -> Opts {
        let depth_name = env_str("MESH_DEPTH", "d32");
        let depth = match depth_name.as_str() {
            "d32s8" => DXGI_FORMAT_D32_FLOAT_S8X24_UINT,
            "d24s8" => DXGI_FORMAT_D24_UNORM_S8_UINT,
            "d16" => DXGI_FORMAT_D16_UNORM,
            _ => DXGI_FORMAT_D32_FLOAT,
        };
        Opts {
            seconds: env_u64("MESH_SECONDS", 120),
            draws: env_u64("MESH_DRAWS", 40),
            groups: env_u64("MESH_GROUPS", 1).max(1),
            inflight: env_u64("MESH_INFLIGHT", 1).clamp(1, 8) as usize,
            user_fuzz: env_str("MESH_USER", "zero") == "fuzz",
            user_sane: env_str("MESH_USER", "zero") == "sane",
            memory: env_str("MESH_MEMORY", "upload"),
            mode: env_str("MESH_MODE", "garbage"),
            rts: env_u64("MESH_RTS", 0).min(8) as u32,
            depth,
            depth_name,
            ps: env_u64("MESH_PS", 0),
            clear: env_u64("MESH_CLEAR", 0) != 0,
            copyback: env_u64("MESH_COPYBACK", 0),
            queues: env_str("MESH_QUEUES", ""),
            indirect: env_u64("MESH_INDIRECT", 0),
            vertex: env_u64("MESH_VERTEX", 0),
            batch: env_u64("MESH_BATCH", 0).min(64),
            fill_views: env_u64("MESH_FILL", 0) != 0,
            pressure_gb: env_u64("MESH_PRESSURE_GB", 0),
            states: env_u64("MESH_STATES", 1).max(1),
            shaders: env_u64("MESH_SHADERS", 1).max(1),
            ia: env_u64("MESH_IA", 1) != 0,
            own: env_u64("MESH_OWN", 0) != 0,
            stencil: env_u64("MESH_STENCIL", 0) != 0,
            slices: env_u64("MESH_SLICES", 1).clamp(1, 16) as u16,
            stats: env_u64("MESH_STATS", 0) != 0,
            dred: env_u64("MESH_DRED", 0) != 0,
            sleep_ms: env_u64("MESH_SLEEP_MS", 0),
            memload: env_u64("MESH_MEMLOAD", 0),
            oversize_mb: env_u64("MESH_OVERSIZE_MB", 0),
            dxil_dir: std::env::var("MESH_DXIL_DIR").ok(),
            dxil_max: env_u64("MESH_DXIL_MAX", 200) as usize,
        }
    }

    fn describe(&self) -> String {
        format!(
            "{} s, {} dispatches a list of up to {} groups, {} in flight, user {}, memory {}, mode {}, depth {}, {} colour targets, ps {}, clear {}, copyback {}, queues [{}], indirect {}, fill {}, pressure {} GB, {} states x {} programs, ia {}, own {}, memload {}, oversize {} MB, dxil dir {:?} (at most {})",
            self.seconds,
            self.draws,
            self.groups,
            self.inflight,
            if self.user_fuzz {
                "fuzz"
            } else if self.user_sane {
                "sane"
            } else {
                "zero"
            },
            self.memory,
            self.mode,
            self.depth_name,
            self.rts,
            self.ps,
            self.clear,
            self.copyback,
            self.queues,
            self.indirect,
            self.fill_views,
            self.pressure_gb,
            self.states,
            self.shaders,
            self.ia,
            self.own,
            self.memload,
            self.oversize_mb,
            self.dxil_dir,
            self.dxil_max
        )
    }
}

/// Whether a depth format has a stencil plane.
fn has_stencil(format: DXGI_FORMAT) -> bool {
    format == DXGI_FORMAT_D32_FLOAT_S8X24_UINT || format == DXGI_FORMAT_D24_UNORM_S8_UINT
}

/// The formats of the colour targets, in the order of the title's main pass (a 16-bit float colour first).
const COLOURS: [DXGI_FORMAT; 8] = [
    DXGI_FORMAT_R16G16B16A16_FLOAT,
    DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R11G11B10_FLOAT,
    DXGI_FORMAT_R10G10B10A2_UNORM,
    DXGI_FORMAT_R16G16_FLOAT,
    DXGI_FORMAT_R8G8B8A8_UNORM,
    DXGI_FORMAT_R32_FLOAT,
    DXGI_FORMAT_R16G16B16A16_FLOAT,
];

/// The root signature of every pipeline here: the user data as root constants, a table of 256 raw views of
/// the vertex data (registers t0 to t255 of space 1: the bindings the host gives the translated programs), and
/// tables of 32 constant buffers (b0 to b31) and 32 unordered access views (u0 to u31) of the same space, for
/// the programs that declare them.
fn root(device: &ID3D12Device, dwords: u32, input_assembler: bool) -> ID3D12RootSignature {
    let range = D3D12_DESCRIPTOR_RANGE1 {
        RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
        NumDescriptors: 256,
        BaseShaderRegister: 0,
        RegisterSpace: 1,
        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
            | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let other = |kind: D3D12_DESCRIPTOR_RANGE_TYPE| D3D12_DESCRIPTOR_RANGE1 {
        RangeType: kind,
        NumDescriptors: 32,
        BaseShaderRegister: 0,
        RegisterSpace: 1,
        Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
            | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
        OffsetInDescriptorsFromTableStart: 0,
    };
    let (constants, unordered) = (
        other(D3D12_DESCRIPTOR_RANGE_TYPE_CBV),
        other(D3D12_DESCRIPTOR_RANGE_TYPE_UAV),
    );
    let table = |range: &D3D12_DESCRIPTOR_RANGE1| D3D12_ROOT_PARAMETER1 {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER1_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: 1,
                pDescriptorRanges: range,
            },
        },
        ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
    };
    let params = [
        D3D12_ROOT_PARAMETER1 {
            ParameterType: D3D12_ROOT_PARAMETER_TYPE_32BIT_CONSTANTS,
            Anonymous: D3D12_ROOT_PARAMETER1_0 {
                Constants: D3D12_ROOT_CONSTANTS {
                    ShaderRegister: 0,
                    RegisterSpace: 0,
                    Num32BitValues: dwords,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        table(&range),
        table(&constants),
        table(&unordered),
    ];
    let desc = D3D12_VERSIONED_ROOT_SIGNATURE_DESC {
        Version: D3D_ROOT_SIGNATURE_VERSION_1_1,
        Anonymous: D3D12_VERSIONED_ROOT_SIGNATURE_DESC_0 {
            Desc_1_1: D3D12_ROOT_SIGNATURE_DESC1 {
                NumParameters: params.len() as u32,
                pParameters: params.as_ptr(),
                NumStaticSamplers: 0,
                pStaticSamplers: std::ptr::null(),
                Flags: D3D12_ROOT_SIGNATURE_FLAG_CBV_SRV_UAV_HEAP_DIRECTLY_INDEXED
                    | D3D12_ROOT_SIGNATURE_FLAG_SAMPLER_HEAP_DIRECTLY_INDEXED
                    | if input_assembler {
                        D3D12_ROOT_SIGNATURE_FLAG_ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT
                    } else {
                        D3D12_ROOT_SIGNATURE_FLAG_NONE
                    },
            },
        },
    };
    serialize_root(device, &desc)
}

fn serialize_root(
    device: &ID3D12Device,
    desc: &D3D12_VERSIONED_ROOT_SIGNATURE_DESC,
) -> ID3D12RootSignature {
    // SAFETY: the description points at live data; the blob is read in full before it is released.
    unsafe {
        let mut blob: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;
        let mut error: Option<windows::Win32::Graphics::Direct3D::ID3DBlob> = None;
        if let Err(e) = D3D12SerializeVersionedRootSignature(desc, &mut blob, Some(&mut error)) {
            let text = error.map(|e| {
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    e.GetBufferPointer().cast::<u8>(),
                    e.GetBufferSize(),
                ))
                .into_owned()
            });
            panic!("root signature: {e} {text:?}");
        }
        let blob = blob.unwrap();
        let bytes =
            std::slice::from_raw_parts(blob.GetBufferPointer().cast::<u8>(), blob.GetBufferSize());
        device.CreateRootSignature(0, bytes).unwrap()
    }
}

/// The state of a pipeline: one of many, by number (0 is the plain one).
#[derive(Clone, Copy)]
struct State {
    cull: D3D12_CULL_MODE,
    func: D3D12_COMPARISON_FUNC,
    write: bool,
    bias: i32,
    slope: f32,
    clip: bool,
    blend: bool,
    stencil: bool,
}

fn state(k: u64, stencil: bool) -> State {
    if k == 0 {
        return State {
            stencil,
            cull: D3D12_CULL_MODE_NONE,
            func: D3D12_COMPARISON_FUNC_LESS,
            write: true,
            bias: 0,
            slope: 0.0,
            clip: true,
            blend: false,
        };
    }
    State {
        stencil,
        cull: [
            D3D12_CULL_MODE_NONE,
            D3D12_CULL_MODE_BACK,
            D3D12_CULL_MODE_FRONT,
        ][(k % 3) as usize],
        func: [
            D3D12_COMPARISON_FUNC_LESS,
            D3D12_COMPARISON_FUNC_LESS_EQUAL,
            D3D12_COMPARISON_FUNC_GREATER_EQUAL,
            D3D12_COMPARISON_FUNC_ALWAYS,
        ][((k / 3) % 4) as usize],
        write: (k / 12).is_multiple_of(2),
        bias: [0, 2000, -2000, 100_000][((k / 24) % 4) as usize],
        slope: [0.0, 1.5, 4.0][((k / 96) % 3) as usize],
        clip: (k / 288).is_multiple_of(2),
        blend: (k / 576) % 2 == 1,
    }
}

/// A vertex pipeline (a triangle by vertex id) with the same targets, root signature and state as the mesh ones.
fn vertex_pipe(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    vs: &[u8],
    ps: &[u8],
    rts: u32,
    depth: DXGI_FORMAT,
    s: State,
) -> windows::core::Result<ID3D12PipelineState> {
    let mut formats = [DXGI_FORMAT_UNKNOWN; 8];
    for (k, f) in COLOURS.iter().enumerate().take(rts.max(1) as usize) {
        formats[k] = *f;
    }
    let mut blend = D3D12_BLEND_DESC::default();
    for t in blend.RenderTarget.iter_mut() {
        t.RenderTargetWriteMask = 0xf;
    }
    let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
        // SAFETY: a borrowed pointer never released.
        pRootSignature: unsafe { std::mem::transmute_copy(root) },
        VS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: vs.as_ptr().cast(),
            BytecodeLength: vs.len(),
        },
        PS: D3D12_SHADER_BYTECODE {
            pShaderBytecode: ps.as_ptr().cast(),
            BytecodeLength: ps.len(),
        },
        BlendState: blend,
        SampleMask: u32::MAX,
        RasterizerState: D3D12_RASTERIZER_DESC {
            FillMode: D3D12_FILL_MODE_SOLID,
            CullMode: D3D12_CULL_MODE_NONE,
            DepthClipEnable: s.clip.into(),
            ..Default::default()
        },
        DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
            DepthEnable: true.into(),
            DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
            DepthFunc: D3D12_COMPARISON_FUNC_LESS,
            StencilEnable: s.stencil.into(),
            StencilReadMask: 0xff,
            StencilWriteMask: 0xff,
            FrontFace: stencil_ops(),
            BackFace: stencil_ops(),
        },
        PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        NumRenderTargets: rts.max(1),
        RTVFormats: formats,
        DSVFormat: depth,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        ..Default::default()
    };
    // SAFETY: the description points at live shaders.
    unsafe { device.CreateGraphicsPipelineState(&desc) }
}

/// Stencil operations that write the reference value wherever a pixel passes.
fn stencil_ops() -> D3D12_DEPTH_STENCILOP_DESC {
    D3D12_DEPTH_STENCILOP_DESC {
        StencilFailOp: D3D12_STENCIL_OP_KEEP,
        StencilDepthFailOp: D3D12_STENCIL_OP_KEEP,
        StencilPassOp: D3D12_STENCIL_OP_REPLACE,
        StencilFunc: D3D12_COMPARISON_FUNC_ALWAYS,
    }
}

/// A mesh pipeline: `ms`, a pixel shader if `ps` is not empty (then with `rts` colour targets), the depth
/// format, the state.
fn pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    ms: &[u8],
    ps: &[u8],
    rts: u32,
    depth: DXGI_FORMAT,
    s: State,
) -> windows::core::Result<ID3D12PipelineState> {
    #[repr(C, align(8))]
    struct Sub<T>(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, T);
    #[repr(C)]
    struct Stream {
        root: Sub<*mut std::ffi::c_void>,
        ms: Sub<D3D12_SHADER_BYTECODE>,
        ps: Sub<D3D12_SHADER_BYTECODE>,
        blend: Sub<D3D12_BLEND_DESC>,
        mask: Sub<u32>,
        raster: Sub<D3D12_RASTERIZER_DESC>,
        depth: Sub<D3D12_DEPTH_STENCIL_DESC>,
        targets: Sub<D3D12_RT_FORMAT_ARRAY>,
        dsv: Sub<DXGI_FORMAT>,
        samples: Sub<DXGI_SAMPLE_DESC>,
        topology: Sub<D3D12_PRIMITIVE_TOPOLOGY_TYPE>,
    }
    let code = |b: &[u8]| D3D12_SHADER_BYTECODE {
        pShaderBytecode: if b.is_empty() {
            std::ptr::null()
        } else {
            b.as_ptr().cast()
        },
        BytecodeLength: b.len(),
    };
    let mut formats = D3D12_RT_FORMAT_ARRAY::default();
    if !ps.is_empty() {
        formats.NumRenderTargets = rts.max(1);
        for (k, f) in COLOURS.iter().enumerate().take(rts.max(1) as usize) {
            formats.RTFormats[k] = *f;
        }
    }
    let mut blend = D3D12_BLEND_DESC::default();
    for t in blend.RenderTarget.iter_mut() {
        t.RenderTargetWriteMask = 0xf;
        if s.blend {
            t.BlendEnable = true.into();
            t.SrcBlend = D3D12_BLEND_SRC_ALPHA;
            t.DestBlend = D3D12_BLEND_INV_SRC_ALPHA;
            t.BlendOp = D3D12_BLEND_OP_ADD;
            t.SrcBlendAlpha = D3D12_BLEND_ONE;
            t.DestBlendAlpha = D3D12_BLEND_ZERO;
            t.BlendOpAlpha = D3D12_BLEND_OP_ADD;
        }
    }
    let mut stream = Stream {
        root: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE,
            root.as_raw(),
        ),
        ms: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS, code(ms)),
        ps: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PS, code(ps)),
        blend: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND, blend),
        mask: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK, u32::MAX),
        raster: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER,
            D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: s.cull,
                DepthBias: s.bias,
                DepthBiasClamp: 0.0,
                SlopeScaledDepthBias: s.slope,
                DepthClipEnable: s.clip.into(),
                ..Default::default()
            },
        ),
        depth: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL,
            D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: if s.write {
                    D3D12_DEPTH_WRITE_MASK_ALL
                } else {
                    D3D12_DEPTH_WRITE_MASK_ZERO
                },
                DepthFunc: s.func,
                StencilEnable: s.stencil.into(),
                StencilReadMask: 0xff,
                StencilWriteMask: 0xff,
                FrontFace: stencil_ops(),
                BackFace: stencil_ops(),
            },
        ),
        targets: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS,
            formats,
        ),
        dsv: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT,
            depth,
        ),
        samples: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_DESC,
            DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
        ),
        topology: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_PRIMITIVE_TOPOLOGY,
            D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
        ),
    };
    let desc = D3D12_PIPELINE_STATE_STREAM_DESC {
        SizeInBytes: size_of::<Stream>(),
        pPipelineStateSubobjectStream: (&raw mut stream).cast(),
    };
    // SAFETY: the stream holds whole subobjects pointing at live data.
    unsafe {
        device
            .cast::<ID3D12Device2>()
            .unwrap()
            .CreatePipelineState(&desc)
    }
}

/// The pixel shader of a translated mesh shader, which outputs a position only: nothing is read from it.
fn flat_pixel(targets: u32) -> String {
    let mut outputs = String::new();
    let mut body = String::new();
    for k in 0..targets.max(1) {
        outputs.push_str(&format!("    float4 c{k} : SV_Target{k};\n"));
        body.push_str(&format!(
            "    o.c{k} = float4(0.25f * {k}.0f, 0.5f, 0.75f, 1.0f);\n"
        ));
    }
    format!("struct O {{\n{outputs}}};\nO ps()\n{{\n    O o;\n{body}    return o;\n}}\n")
}

/// A resource of `bytes` in the card's own memory, unordered access allowed.
fn default_buffer(
    device: &ID3D12Device,
    bytes: u64,
    uav: bool,
    state: D3D12_RESOURCE_STATES,
) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: if uav {
            D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS
        } else {
            D3D12_RESOURCE_FLAG_NONE
        },
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation.
    unsafe {
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &desc,
                state,
                None,
                &mut made,
            )
            .unwrap();
    }
    made.unwrap()
}

/// A 2D target of the given format and flags, in `state`, with the clear value a target of that kind has.
fn target_texture(
    device: &ID3D12Device,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    state: D3D12_RESOURCE_STATES,
    depth: bool,
) -> ID3D12Resource {
    target_array(device, format, flags, state, depth, 1)
}

/// [`target_texture`] of `slices` array slices.
fn target_array(
    device: &ID3D12Device,
    format: DXGI_FORMAT,
    flags: D3D12_RESOURCE_FLAGS,
    state: D3D12_RESOURCE_STATES,
    depth: bool,
    slices: u16,
) -> ID3D12Resource {
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
        Width: u64::from(WIDTH),
        Height: HEIGHT,
        DepthOrArraySize: slices,
        MipLevels: 1,
        Format: format,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Flags: flags,
        ..Default::default()
    };
    let clear = D3D12_CLEAR_VALUE {
        Format: format,
        Anonymous: if depth {
            D3D12_CLEAR_VALUE_0 {
                DepthStencil: D3D12_DEPTH_STENCIL_VALUE {
                    Depth: 1.0,
                    Stencil: 0,
                },
            }
        } else {
            D3D12_CLEAR_VALUE_0 {
                Color: [0.0, 0.0, 0.0, 0.0],
            }
        },
    };
    // (A clear value is for targets only: a texture that is neither takes none.)
    let target = flags.0
        & (D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET.0 | D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL.0)
        != 0;
    let mut made: Option<ID3D12Resource> = None;
    // SAFETY: plain creation.
    unsafe {
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &desc,
                state,
                target.then_some(&clear),
                &mut made,
            )
            .unwrap();
    }
    made.unwrap()
}

fn transition(
    res: &ID3D12Resource,
    before: D3D12_RESOURCE_STATES,
    after: D3D12_RESOURCE_STATES,
) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_TRANSITION,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            Transition: ManuallyDrop::new(D3D12_RESOURCE_TRANSITION_BARRIER {
                // SAFETY: a borrowed pointer never released.
                pResource: unsafe { std::mem::transmute_copy(res) },
                Subresource: D3D12_RESOURCE_BARRIER_ALL_SUBRESOURCES,
                StateBefore: before,
                StateAfter: after,
            }),
        },
        ..Default::default()
    }
}

fn uav_barrier(res: &ID3D12Resource) -> D3D12_RESOURCE_BARRIER {
    D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                // SAFETY: a borrowed pointer never released.
                pResource: unsafe { std::mem::transmute_copy(res) },
            }),
        },
        ..Default::default()
    }
}

/// The colour twins of a depth buffer (`MESH_COPYBACK=3`): for a format without stencil a colour texture of the same
/// family, for one with a stencil plane an 8-bit texture and the buffer the plane goes through.
struct Twins {
    colour: Option<ID3D12Resource>,
    /// The twin of the stencil planes, the buffer they go through, and a footprint for each plane.
    stencil: Option<(
        ID3D12Resource,
        ID3D12Resource,
        Vec<D3D12_PLACED_SUBRESOURCE_FOOTPRINT>,
    )>,
}

/// Records the copies of a depth buffer (in the copy source state) into its twins and back, as the host does when
/// a depth buffer is read as a colour texture and a compute pass writes the twin. The depth buffer ends in the
/// copy source state again, the twins in the copy destination state.
fn twin_copies(list: &ID3D12GraphicsCommandList6, depth: &ID3D12Resource, twins: &Twins) {
    const SOURCE: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_SOURCE;
    const DEST: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COPY_DEST;
    const COMMON: D3D12_RESOURCE_STATES = D3D12_RESOURCE_STATE_COMMON;
    let sub = |t: &ID3D12Resource, index: u32| D3D12_TEXTURE_COPY_LOCATION {
        // SAFETY: a borrowed pointer never released.
        pResource: unsafe { std::mem::transmute_copy(t) },
        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
            SubresourceIndex: index,
        },
    };
    let placed = |b: &ID3D12Resource, row: &D3D12_PLACED_SUBRESOURCE_FOOTPRINT| {
        D3D12_TEXTURE_COPY_LOCATION {
            // SAFETY: a borrowed pointer never released.
            pResource: unsafe { std::mem::transmute_copy(b) },
            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                PlacedFootprint: *row,
            },
        }
    };
    // SAFETY: an open list and live resources, in the states said above.
    unsafe {
        if let Some(twin) = &twins.colour {
            list.CopyResource(twin, depth);
            list.ResourceBarrier(&[transition(twin, DEST, SOURCE), transition(depth, SOURCE, DEST)]);
            list.CopyResource(depth, twin);
            list.ResourceBarrier(&[transition(twin, SOURCE, DEST), transition(depth, DEST, SOURCE)]);
        }
        if let Some((twin, through, rows)) = &twins.stencil {
            // The stencil planes' subresources follow the depth planes' (one mip: a plane for each slice).
            let first = rows.len() as u32;
            list.ResourceBarrier(&[transition(through, COMMON, DEST)]);
            for (i, row) in rows.iter().enumerate() {
                list.CopyTextureRegion(&placed(through, row), 0, 0, 0, &sub(depth, first + i as u32), None);
            }
            list.ResourceBarrier(&[transition(through, DEST, SOURCE)]);
            for (i, row) in rows.iter().enumerate() {
                list.CopyTextureRegion(&sub(twin, i as u32), 0, 0, 0, &placed(through, row), None);
            }
            // and back: the twin through the buffer into the depth buffer's stencil planes
            list.ResourceBarrier(&[transition(twin, DEST, SOURCE), transition(through, SOURCE, DEST)]);
            for (i, row) in rows.iter().enumerate() {
                list.CopyTextureRegion(&placed(through, row), 0, 0, 0, &sub(twin, i as u32), None);
            }
            list.ResourceBarrier(&[transition(through, DEST, SOURCE), transition(depth, SOURCE, DEST)]);
            for (i, row) in rows.iter().enumerate() {
                list.CopyTextureRegion(&sub(depth, first + i as u32), 0, 0, 0, &placed(through, row), None);
            }
            list.ResourceBarrier(&[
                transition(through, SOURCE, COMMON),
                transition(depth, DEST, SOURCE),
                transition(twin, SOURCE, DEST),
            ]);
        }
    }
}

/// A queue of its own with a list, an allocator and a fence, for a worker thread.
struct Lane {
    queue: ID3D12CommandQueue,
    allocator: ID3D12CommandAllocator,
    list: ID3D12GraphicsCommandList,
    fence: ID3D12Fence,
    event: HANDLE,
    value: u64,
}

impl Lane {
    fn new(device: &ID3D12Device, kind: D3D12_COMMAND_LIST_TYPE) -> Lane {
        // SAFETY: plain creation.
        unsafe {
            let queue = device
                .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                    Type: kind,
                    ..Default::default()
                })
                .unwrap();
            let allocator: ID3D12CommandAllocator = device.CreateCommandAllocator(kind).unwrap();
            let list: ID3D12GraphicsCommandList =
                device.CreateCommandList(0, kind, &allocator, None).unwrap();
            list.Close().unwrap();
            Lane {
                queue,
                allocator,
                list,
                fence: device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap(),
                event: CreateEventW(None, false, false, None).unwrap(),
                value: 0,
            }
        }
    }

    /// Opens the list again (the last one has finished).
    fn open(&self) -> &ID3D12GraphicsCommandList {
        // SAFETY: the last list was waited for.
        unsafe {
            self.allocator.Reset().unwrap();
            self.list.Reset(&self.allocator, None).unwrap();
        }
        &self.list
    }

    /// Closes, runs and waits for the list, for up to `limit_ms`; false if the queue did not finish.
    fn run(&mut self, limit_ms: u32) -> bool {
        // SAFETY: a closed list on its own queue.
        unsafe {
            self.list.Close().unwrap();
            self.queue
                .ExecuteCommandLists(&[Some(self.list.cast().unwrap())]);
            self.value += 1;
            self.queue.Signal(&self.fence, self.value).unwrap();
            self.fence
                .SetEventOnCompletion(self.value, self.event)
                .unwrap();
            WaitForSingleObject(self.event, limit_ms).0 == 0
        }
    }
}

/// What the workers did, for the progress lines.
#[derive(Default)]
struct Counts {
    copies: AtomicU64,
    computes: AtomicU64,
    streamed: AtomicU64,
    rewrites: AtomicU64,
    moved_mb: AtomicU64,
    stuck: AtomicBool,
}

/// The copy queue's traffic: the CPU writes upload memory, the queue copies it to the card's memory and from
/// there into write-back memory, which the CPU reads; with `stream`, textures are made, copied into and
/// released as a title's streaming does.
fn copy_worker(
    device: Share<ID3D12Device>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    stream: bool,
    cpu_slots: Option<(usize, usize)>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let device = device.into_inner();
        let mut lane = Lane::new(&device, D3D12_COMMAND_LIST_TYPE_COPY);
        const SIZE: usize = 64 << 20;
        let (up, up_data) = upload_buffer(&device, SIZE as u64, "upload");
        let (wb, wb_data) = upload_buffer(&device, (SIZE / 2) as u64, "writeback");
        let card = default_buffer(&device, SIZE as u64, false, D3D12_RESOURCE_STATE_COMMON);
        let mut rng = Rng(0x0dd_ba11 ^ 77);
        let mut textures: std::collections::VecDeque<ID3D12Resource> = Default::default();
        let mut n = 0u64;
        while !stop.load(Relaxed) {
            // The CPU writes a piece of what will be copied.
            let at = (rng.next() as usize % (SIZE - (2 << 20))) & !255;
            // SAFETY: inside the mapped upload buffer, which no list reads while this one is open.
            unsafe {
                let words =
                    std::slice::from_raw_parts_mut(up_data.add(at).cast::<u32>(), (2 << 20) / 4);
                for w in words.iter_mut().step_by(16) {
                    *w = rng.next();
                }
            }
            let list = lane.open();
            // SAFETY: an open list; live resources.
            unsafe {
                if n.is_multiple_of(2) {
                    list.CopyBufferRegion(&card, 0, &up, 0, (SIZE / 2) as u64);
                } else {
                    list.CopyBufferRegion(&wb, 0, &card, (SIZE / 2) as u64, (SIZE / 2) as u64);
                }
            }
            if stream && n.is_multiple_of(4) {
                let desc = D3D12_RESOURCE_DESC {
                    Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                    Width: 1024,
                    Height: 1024,
                    DepthOrArraySize: 1,
                    MipLevels: 1,
                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    ..Default::default()
                };
                let mut made: Option<ID3D12Resource> = None;
                // SAFETY: plain creation; the copy reads the upload buffer's first 4 MB.
                unsafe {
                    device
                        .CreateCommittedResource(
                            &D3D12_HEAP_PROPERTIES {
                                Type: D3D12_HEAP_TYPE_DEFAULT,
                                ..Default::default()
                            },
                            D3D12_HEAP_FLAG_NONE,
                            &desc,
                            D3D12_RESOURCE_STATE_COMMON,
                            None,
                            &mut made,
                        )
                        .unwrap();
                    let texture = made.unwrap();
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&up),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            PlacedFootprint: D3D12_PLACED_SUBRESOURCE_FOOTPRINT {
                                Offset: 0,
                                Footprint: D3D12_SUBRESOURCE_FOOTPRINT {
                                    Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                                    Width: 1024,
                                    Height: 1024,
                                    Depth: 1,
                                    RowPitch: 4096,
                                },
                            },
                        },
                    };
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&texture),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            SubresourceIndex: 0,
                        },
                    };
                    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                    // A view of it in a heap slot, as the title's loader writes one.
                    if let Some((cpu, step)) = cpu_slots {
                        let slot = 2_000 + (counts.streamed.load(Relaxed) as usize % 4096);
                        device.CreateShaderResourceView(
                            &texture,
                            None,
                            D3D12_CPU_DESCRIPTOR_HANDLE {
                                ptr: cpu + slot * step,
                            },
                        );
                    }
                    textures.push_back(texture);
                }
                counts.streamed.fetch_add(1, Relaxed);
            }
            if !lane.run(30_000) {
                counts.stuck.store(true, Relaxed);
                println!("THE COPY QUEUE DID NOT ANSWER in 30 s (copy {n})");
                return;
            }
            // (The oldest textures go as the queue is done with them.)
            while textures.len() > 8 {
                textures.pop_front();
            }
            // The CPU reads what the card copied to write-back memory.
            let mut sum = 0u32;
            // SAFETY: inside the mapped write-back buffer.
            unsafe {
                let words = std::slice::from_raw_parts(wb_data.cast::<u32>(), 4096);
                for w in words.iter().step_by(7) {
                    sum = sum.wrapping_add(*w);
                }
            }
            std::hint::black_box(sum);
            n += 1;
            counts.copies.fetch_add(1, Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(5));
        }
    })
}

/// The compute queue's passes: big dispatches with atomics, with a barrier between them.
fn compute_worker(
    device: Share<ID3D12Device>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let device = device.into_inner();
        let mut lane = Lane::new(&device, D3D12_COMMAND_LIST_TYPE_COMPUTE);
        let code = hlsl::dxc(hlsl::CHURN, "main", "cs_6_6", &[]);
        // SAFETY: plain creation from the compiled program, which carries its root signature.
        let (root, pso): (ID3D12RootSignature, ID3D12PipelineState) = unsafe {
            let root: ID3D12RootSignature = device.CreateRootSignature(0, &code).unwrap();
            let pso = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: code.as_ptr().cast(),
                        BytecodeLength: code.len(),
                    },
                    ..Default::default()
                })
                .unwrap();
            (root, pso)
        };
        let a = default_buffer(&device, 64 << 20, true, D3D12_RESOURCE_STATE_COMMON);
        let b = default_buffer(&device, 1 << 20, true, D3D12_RESOURCE_STATE_COMMON);
        while !stop.load(Relaxed) {
            let list = lane.open();
            // SAFETY: an open list; live resources.
            unsafe {
                list.SetComputeRootSignature(&root);
                list.SetPipelineState(&pso);
                list.SetComputeRootUnorderedAccessView(0, a.GetGPUVirtualAddress());
                list.SetComputeRootUnorderedAccessView(1, b.GetGPUVirtualAddress());
                for _ in 0..8 {
                    list.Dispatch(4096, 1, 1);
                    list.ResourceBarrier(&[uav_barrier(&a), uav_barrier(&b)]);
                }
            }
            if !lane.run(30_000) {
                counts.stuck.store(true, Relaxed);
                println!("THE COMPUTE QUEUE DID NOT ANSWER in 30 s");
                return;
            }
            counts.computes.fetch_add(1, Relaxed);
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    })
}

/// What the descriptor heap holds around a draw's tables, and what a rewriting thread puts there.
struct Views {
    device: ID3D12Device,
    /// Bytes each buffer view claims past the end of its buffer.
    extra: usize,
    cpu: usize,
    step: usize,
    meshlets: [ID3D12Resource; 2],
    matrices: [ID3D12Resource; 2],
    constants: [ID3D12Resource; 2],
    vertices: ID3D12Resource,
    texture: ID3D12Resource,
    uav: ID3D12Resource,
}

const MESHLETS: usize = 4 << 20;
const MATRICES: usize = 1 << 20;
const VERTICES: usize = 16 << 20;

/// What a view in slot `slot` is, by its kind.
#[derive(Clone, Copy, PartialEq)]
enum Kind {
    Raw(usize),
    Typed,
    Structured,
    Constants,
    Null,
    Texture,
    Uav,
    Matrices,
}

impl Views {
    fn write(&self, slot: usize, kind: Kind, alt: usize) {
        let at = D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: self.cpu + slot * self.step,
        };
        let buffer =
            |format: DXGI_FORMAT, elements: u32, stride: u32, flags: D3D12_BUFFER_SRV_FLAGS| {
                D3D12_SHADER_RESOURCE_VIEW_DESC {
                    Format: format,
                    ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                    Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                    Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                        Buffer: D3D12_BUFFER_SRV {
                            FirstElement: 0,
                            NumElements: elements,
                            StructureByteStride: stride,
                            Flags: flags,
                        },
                    },
                }
            };
        let raw = |bytes: usize| {
            buffer(
                DXGI_FORMAT_R32_TYPELESS,
                (bytes / 4) as u32,
                0,
                D3D12_BUFFER_SRV_FLAG_RAW,
            )
        };
        // SAFETY: writes one descriptor of a heap this struct's owner keeps alive.
        unsafe {
            match kind {
                Kind::Raw(which) => {
                    let (res, bytes) = match which {
                        0 => (&self.meshlets[alt], MESHLETS),
                        1 => (&self.matrices[alt], MATRICES),
                        _ => (&self.vertices, VERTICES),
                    };
                    self.device.CreateShaderResourceView(
                        res,
                        Some(&raw(bytes + self.extra)),
                        at,
                    );
                }
                Kind::Matrices => {
                    self.device.CreateShaderResourceView(
                        &self.matrices[alt],
                        Some(&raw(MATRICES + self.extra)),
                        at,
                    );
                }
                Kind::Typed => {
                    self.device.CreateShaderResourceView(
                        &self.vertices,
                        Some(&buffer(
                            DXGI_FORMAT_R32G32B32A32_FLOAT,
                            ((VERTICES + self.extra) / 16) as u32,
                            0,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        )),
                        at,
                    );
                }
                Kind::Structured => {
                    self.device.CreateShaderResourceView(
                        &self.matrices[alt],
                        Some(&buffer(
                            DXGI_FORMAT_UNKNOWN,
                            ((MATRICES + self.extra) / 48) as u32,
                            48,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        )),
                        at,
                    );
                }
                Kind::Constants => {
                    self.device.CreateConstantBufferView(
                        Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC {
                            BufferLocation: self.constants[alt].GetGPUVirtualAddress(),
                            SizeInBytes: 65536,
                        }),
                        at,
                    );
                }
                Kind::Null => {
                    self.device.CreateShaderResourceView(
                        None::<&ID3D12Resource>,
                        Some(&buffer(
                            DXGI_FORMAT_R32_TYPELESS,
                            1024,
                            0,
                            D3D12_BUFFER_SRV_FLAG_RAW,
                        )),
                        at,
                    );
                }
                Kind::Uav => {
                    self.device.CreateUnorderedAccessView(
                        &self.uav,
                        None::<&ID3D12Resource>,
                        Some(&D3D12_UNORDERED_ACCESS_VIEW_DESC {
                            Format: DXGI_FORMAT_R32_TYPELESS,
                            ViewDimension: D3D12_UAV_DIMENSION_BUFFER,
                            Anonymous: D3D12_UNORDERED_ACCESS_VIEW_DESC_0 {
                                Buffer: D3D12_BUFFER_UAV {
                                    FirstElement: 0,
                                    NumElements: (MESHLETS / 4) as u32,
                                    StructureByteStride: 0,
                                    CounterOffsetInBytes: 0,
                                    Flags: D3D12_BUFFER_UAV_FLAG_RAW,
                                },
                            },
                        }),
                        at,
                    );
                }
                Kind::Texture => {
                    self.device.CreateShaderResourceView(
                        &self.texture,
                        Some(&D3D12_SHADER_RESOURCE_VIEW_DESC {
                            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
                            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                                Texture2D: D3D12_TEX2D_SRV {
                                    MipLevels: 1,
                                    ..Default::default()
                                },
                            },
                        }),
                        at,
                    );
                }
            }
        }
    }
}

/// The slots a draw's tables hold: eight from the table's first.
fn table_kinds() -> [Kind; 8] {
    [
        Kind::Raw(0),
        Kind::Raw(0),
        Kind::Matrices,
        Kind::Constants,
        Kind::Raw(0),
        Kind::Matrices,
        Kind::Raw(0),
        Kind::Raw(0),
    ]
}

/// The CPU rewrites descriptors across the heap: slots the draws do not read, and with `race` the ones
/// they read too (the same kind, from the other of two equal resources).
fn churn_worker(
    views: Share<Arc<Views>>,
    stop: Arc<AtomicBool>,
    counts: Arc<Counts>,
    race: bool,
    seed: u64,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        let views = views.into_inner();
        let mut rng = Rng(0xfeed_0000 ^ seed);
        let kinds = [
            Kind::Raw(0),
            Kind::Typed,
            Kind::Structured,
            Kind::Constants,
            Kind::Matrices,
            Kind::Null,
            Kind::Texture,
            Kind::Raw(2),
        ];
        while !stop.load(Relaxed) {
            for _ in 0..256 {
                if race && rng.next().is_multiple_of(4) {
                    let t = TABLES[rng.next() as usize % TABLES.len()] as usize;
                    let k = (rng.next() % 8) as usize;
                    views.write(t + k, table_kinds()[k], (rng.next() % 2) as usize);
                } else {
                    let slot = 1100 + rng.next() as usize % (VIEWS as usize - 1100);
                    let near = TABLES
                        .iter()
                        .any(|&t| (slot as i64 - i64::from(t)).abs() < 16);
                    if near {
                        continue;
                    }
                    views.write(
                        slot,
                        kinds[(rng.next() % 8) as usize],
                        (rng.next() % 2) as usize,
                    );
                }
                counts.rewrites.fetch_add(1, Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_micros(200));
        }
    })
}

/// A CPU thread that streams memory: it copies and compares big blocks, as the host does with the title's
/// buffers (2.6 GB a frame were compared on the CPU in the title's world), so that the CPU and the card
/// share a busy memory bus.
fn memory_worker(stop: Arc<AtomicBool>, counts: Arc<Counts>, k: u64) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        const BLOCK: usize = 128 << 20;
        const STEP: usize = 4 << 20;
        let mut a = vec![0x11u8; BLOCK];
        let mut b = vec![0x22u8; BLOCK];
        let mut n = k;
        while !stop.load(Relaxed) {
            for at in (0..BLOCK).step_by(STEP) {
                b[at..at + STEP].copy_from_slice(&a[at..at + STEP]);
            }
            std::hint::black_box(a == b);
            a[(n as usize * 4096) % BLOCK] ^= 1;
            n += 1;
            counts.moved_mb.fetch_add(2 * (BLOCK >> 20) as u64, Relaxed);
        }
    })
}

/// The breadcrumbs of the commands that did not finish, if the device keeps them (`MESH_DRED=1`).
fn breadcrumbs(device: &ID3D12Device) {
    let Ok(dred) = device.cast::<ID3D12DeviceRemovedExtendedData>() else {
        println!("no device-removed data on this device");
        return;
    };
    // SAFETY: the output is the runtime's, valid while the device lives; nodes are walked as linked and
    // histories have their counts.
    unsafe {
        match dred.GetPageFaultAllocationOutput() {
            Ok(fault) if fault.PageFaultVA != 0 => {
                println!("page fault at {:#x}", fault.PageFaultVA)
            }
            _ => println!("no page fault address"),
        }
        let Ok(crumbs) = dred.GetAutoBreadcrumbsOutput() else {
            println!("no breadcrumbs");
            return;
        };
        let mut node = crumbs.pHeadAutoBreadcrumbNode;
        while let Some(p) = node.as_ref() {
            let done = p.pLastBreadcrumbValue.as_ref().copied().unwrap_or(0);
            if done < p.BreadcrumbCount && !p.pCommandHistory.is_null() {
                let history =
                    std::slice::from_raw_parts(p.pCommandHistory, p.BreadcrumbCount as usize);
                let around: Vec<String> = history
                    .iter()
                    .enumerate()
                    .skip(done.saturating_sub(3) as usize)
                    .take(8)
                    .map(|(i, op)| {
                        format!(
                            "{i}{}:op {}",
                            if i == done as usize { "*" } else { "" },
                            op.0
                        )
                    })
                    .collect();
                println!(
                    "a list stopped at command {done} of {}: {around:?} (op 42 is DispatchMesh, 5 ExecuteIndirect, 8 CopyTextureRegion, 15 ResourceBarrier)",
                    p.BreadcrumbCount
                );
            }
            node = p.pNext;
        }
    }
}

/// The state of the machine's memory, in MB: (free, total).
fn memory_mb() -> (u64, u64) {
    let mut status = windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX {
        dwLength: size_of::<windows_sys::Win32::System::SystemInformation::MEMORYSTATUSEX>() as u32,
        ..unsafe { std::mem::zeroed() }
    };
    // SAFETY: the structure's size is set.
    unsafe { windows_sys::Win32::System::SystemInformation::GlobalMemoryStatusEx(&mut status) };
    (status.ullAvailPhys >> 20, status.ullTotalPhys >> 20)
}

pub fn run() {
    let o = Opts::from_env();
    let translated =
        std::env::var_os("MESH_DXIL").map(|p| std::fs::read(p).expect("the DXIL file"));
    if translated.is_none() && !o.own && o.dxil_dir.is_none() {
        println!("MESH_DXIL names no file, MESH_DXIL_DIR no folder and MESH_OWN is not set: nothing to run");
        return;
    }
    println!("load test: {}", o.describe());
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ env_u64("MESH_SEED", 1));
    // Memory held to leave the machine little (never below 3 GB free).
    let mut held: Vec<Vec<u8>> = Vec::new();
    if o.pressure_gb > 0 {
        let (free, total) = memory_mb();
        println!("memory before: {free} MB free of {total} MB");
        let mut left = o.pressure_gb << 10;
        while left > 0 {
            let (free, _) = memory_mb();
            if free < 3 * 1024 + 256 {
                println!(
                    "pressure stopped at {} MB held: {free} MB free",
                    (o.pressure_gb << 10) - left
                );
                break;
            }
            let chunk = left.min(256);
            held.push(vec![0xA5u8; (chunk << 20) as usize]);
            left -= chunk;
        }
        let (free, _) = memory_mb();
        println!("memory held: {} MB, {free} MB free", held.len() * 256);
    }
    // SAFETY: plain D3D12 objects, used as the API says; the buffers outlive every list that reads them
    // (each list is waited for before its memory is reused or released).
    unsafe {
        if std::env::var_os("MESH_DEBUG").is_some() {
            let mut layer: Option<ID3D12Debug> = None;
            if D3D12GetDebugInterface(&mut layer).is_ok() {
                layer.iter().for_each(|l| l.EnableDebugLayer());
            }
        }
        if o.dred {
            let mut settings: Option<ID3D12DeviceRemovedExtendedDataSettings> = None;
            if D3D12GetDebugInterface(&mut settings).is_ok()
                && let Some(s) = settings
            {
                s.SetAutoBreadcrumbsEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
                s.SetPageFaultEnablement(D3D12_DRED_ENABLEMENT_FORCED_ON);
            }
        }
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(
            None,
            windows::Win32::Graphics::Direct3D::D3D_FEATURE_LEVEL_12_0,
            &mut device,
        )
        .unwrap();
        let device = device.unwrap();
        let mut options = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        device
            .CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS7,
                (&raw mut options).cast(),
                size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS7>() as u32,
            )
            .unwrap();
        if options.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
            println!("no mesh shaders on this adapter");
            return;
        }
        let root = root(&device, 32, o.ia);
        // The pipelines: each program with each state (and a pixel shader, as asked).
        let mut pipes: Vec<(ID3D12PipelineState, bool)> = Vec::new();
        let made = std::time::Instant::now();
        let build = |ms: &[u8], flat: bool, pipes: &mut Vec<(ID3D12PipelineState, bool)>| {
            let pixel = hlsl::dxc(
                &if flat {
                    flat_pixel(o.rts)
                } else {
                    hlsl::own_pixel(o.rts)
                },
                "ps",
                "ps_6_6",
                &[],
            );
            for k in 0..o.states {
                let with_ps = match o.ps {
                    0 => false,
                    1 => true,
                    _ => k % 2 == 1,
                };
                match pipeline(
                    &device,
                    &root,
                    ms,
                    if with_ps { &pixel } else { &[] },
                    o.rts,
                    o.depth,
                    state(k, o.stencil),
                ) {
                    Ok(pso) => pipes.push((pso, with_ps)),
                    Err(e) => println!("a pipeline (state {k}, ps {with_ps}) was refused: {e}"),
                }
            }
        };
        if let Some(ms) = &translated {
            build(ms, true, &mut pipes);
        }
        if let Some(dir) = &o.dxil_dir {
            let mut files: Vec<std::path::PathBuf> = std::fs::read_dir(dir)
                .expect("the MESH_DXIL_DIR folder")
                .flatten()
                .map(|e| e.path())
                .filter(|p| p.to_string_lossy().ends_with("_Ms.dxil"))
                .collect();
            files.sort();
            let (mut made_n, mut refused) = (0, 0);
            for f in files.iter().take(o.dxil_max) {
                let ms = std::fs::read(f).expect("a mesh program");
                for k in 0..o.states {
                    match pipeline(&device, &root, &ms, &[], o.rts, o.depth, state(k, o.stencil)) {
                        Ok(pso) => {
                            pipes.push((pso, false));
                            made_n += 1;
                        }
                        Err(e) => {
                            refused += 1;
                            if refused <= 5 {
                                println!("refused {}: {e}", f.file_name().unwrap_or_default().to_string_lossy());
                            }
                        }
                    }
                }
            }
            println!(
                "{made_n} pipelines of {} translated programs made, {refused} refused",
                files.len().min(o.dxil_max)
            );
        }
        if o.own {
            for v in 0..o.shaders {
                let source = format!("#define VARIANT {v}\n{}", hlsl::OWN_MESH);
                let ms = hlsl::dxc(&source, "ms", "ms_6_6", &[]);
                build(&ms, false, &mut pipes);
            }
        }
        assert!(!pipes.is_empty(), "no pipeline was made");
        let vertex_pso: Option<ID3D12PipelineState> = (o.vertex > 0).then(|| {
            let vs = hlsl::dxc(hlsl::VERTEX, "vs", "vs_6_6", &[]);
            let ps = hlsl::dxc(&hlsl::own_pixel(o.rts), "ps", "ps_6_6", &[]);
            vertex_pipe(&device, &root, &vs, &ps, o.rts, o.depth, state(0, o.stencil))
                .expect("a vertex pipeline")
        });
        println!(
            "{} pipelines made in {:.1} s",
            pipes.len(),
            made.elapsed().as_secs_f64()
        );
        let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            as usize;
        let heap: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: VIEWS,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
            .unwrap();
        let (cpu, gpu) = (
            heap.GetCPUDescriptorHandleForHeapStart().ptr,
            heap.GetGPUDescriptorHandleForHeapStart().ptr,
        );
        // The data: (meshlets, matrices, constants, vertices), each in two, so that a view can be
        // swapped for another of the same size.
        let pair = |bytes: usize| {
            [
                upload_buffer(&device, bytes as u64, &o.memory),
                upload_buffer(&device, bytes as u64, &o.memory),
            ]
        };
        let (meshlets, matrices, constants) = (pair(MESHLETS), pair(MATRICES), pair(65536));
        let vertices = upload_buffer(&device, VERTICES as u64, &o.memory);
        let datas: Vec<(*mut u8, usize)> = meshlets
            .iter()
            .map(|b| (b.1, MESHLETS))
            .chain(matrices.iter().map(|b| (b.1, MATRICES)))
            .chain(constants.iter().map(|b| (b.1, 65536)))
            .chain([(vertices.1, VERTICES)])
            .collect();
        for (data, bytes) in &datas {
            if o.own && *data == vertices.1 {
                // Our program reads a few bytes of it: plain small values.
                fill("small", &mut rng, *data, *bytes);
            } else {
                fill(&o.mode, &mut rng, *data, *bytes);
            }
        }
        let views = Arc::new(Views {
            device: device.clone(),
            extra: (o.oversize_mb << 20) as usize,
            cpu,
            step,
            meshlets: [meshlets[0].0.clone(), meshlets[1].0.clone()],
            matrices: [matrices[0].0.clone(), matrices[1].0.clone()],
            constants: [constants[0].0.clone(), constants[1].0.clone()],
            vertices: vertices.0.clone(),
            texture: target_texture(
                &device,
                DXGI_FORMAT_R8G8B8A8_UNORM,
                D3D12_RESOURCE_FLAG_NONE,
                D3D12_RESOURCE_STATE_COMMON,
                false,
            ),
            uav: default_buffer(
                &device,
                MESHLETS as u64,
                true,
                D3D12_RESOURCE_STATE_COMMON,
            ),
        });
        // The views: the root table's 256 (the vertex data), 768 more of the kinds a typed pick finds, the
        // tables' slots, and with `MESH_FILL` all the others.
        let began = std::time::Instant::now();
        if o.fill_views {
            let kinds = [
                Kind::Raw(0),
                Kind::Typed,
                Kind::Structured,
                Kind::Constants,
                Kind::Matrices,
                Kind::Null,
                Kind::Texture,
                Kind::Raw(2),
            ];
            for slot in 0..VIEWS as usize {
                views.write(slot, kinds[slot % 8], 0);
            }
        }
        for slot in 0..256 {
            views.write(slot, Kind::Raw(2), 0);
        }
        for slot in 256..1024 {
            views.write(slot, Kind::Typed, 0);
        }
        for slot in 1024..1056 {
            views.write(slot, Kind::Constants, 0);
        }
        for slot in 1056..1088 {
            views.write(slot, Kind::Uav, 0);
        }
        for &t in &TABLES {
            for (k, kind) in table_kinds().iter().enumerate() {
                views.write(t as usize + k, *kind, 0);
            }
        }
        println!("views written in {:.1} s", began.elapsed().as_secs_f64());
        // The targets.
        let depth = target_array(
            &device,
            o.depth,
            D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            D3D12_RESOURCE_STATE_DEPTH_WRITE,
            true,
            o.slices,
        );
        let dsvs: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                NumDescriptors: 1,
                ..Default::default()
            })
            .unwrap();
        let dsv = dsvs.GetCPUDescriptorHandleForHeapStart();
        if o.slices > 1 {
            // The first slice is drawn to.
            device.CreateDepthStencilView(
                &depth,
                Some(&D3D12_DEPTH_STENCIL_VIEW_DESC {
                    Format: o.depth,
                    ViewDimension: D3D12_DSV_DIMENSION_TEXTURE2DARRAY,
                    Flags: D3D12_DSV_FLAG_NONE,
                    Anonymous: D3D12_DEPTH_STENCIL_VIEW_DESC_0 {
                        Texture2DArray: D3D12_TEX2D_ARRAY_DSV {
                            MipSlice: 0,
                            FirstArraySlice: 0,
                            ArraySize: 1,
                        },
                    },
                }),
                dsv,
            );
        } else {
            device.CreateDepthStencilView(&depth, None, dsv);
        }
        let colour_count = o.rts.max(1) as usize;
        let colours: Vec<ID3D12Resource> = (0..colour_count)
            .map(|k| {
                target_texture(
                    &device,
                    COLOURS[k],
                    D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                    D3D12_RESOURCE_STATE_RENDER_TARGET,
                    false,
                )
            })
            .collect();
        let rtvs: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_RTV,
                NumDescriptors: colour_count as u32,
                ..Default::default()
            })
            .unwrap();
        let rtv_step =
            device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_RTV) as usize;
        let rtv_start = rtvs.GetCPUDescriptorHandleForHeapStart();
        let rtv_handles: Vec<D3D12_CPU_DESCRIPTOR_HANDLE> = (0..colour_count)
            .map(|k| D3D12_CPU_DESCRIPTOR_HANDLE {
                ptr: rtv_start.ptr + k * rtv_step,
            })
            .collect();
        for (c, h) in colours.iter().zip(&rtv_handles) {
            device.CreateRenderTargetView(c, None, *h);
        }
        // The copy-back's memory: write-back memory the CPU reads, a footprint for each target.
        let mut footprints = Vec::new();
        let mut total = 0u64;
        for c in &colours {
            let mut f = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
            let mut bytes = 0u64;
            device.GetCopyableFootprints(
                &c.GetDesc(),
                0,
                1,
                total,
                Some(&mut f),
                None,
                None,
                Some(&mut bytes),
            );
            footprints.push(f);
            total = (total + bytes).next_multiple_of(512);
        }
        // (With `MESH_COPYBACK=3` textures of their own, in the copy destination state for good.)
        let colour_copies: Vec<ID3D12Resource> = if o.copyback >= 3 {
            (0..colour_count)
                .map(|k| {
                    target_texture(
                        &device,
                        COLOURS[k],
                        D3D12_RESOURCE_FLAG_ALLOW_RENDER_TARGET,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        false,
                    )
                })
                .collect()
        } else {
            Vec::new()
        };
        let twins = Twins {
            colour: (o.copyback >= 3 && o.depth == DXGI_FORMAT_D32_FLOAT).then(|| {
                target_array(
                    &device,
                    DXGI_FORMAT_R32_FLOAT,
                    D3D12_RESOURCE_FLAG_NONE,
                    D3D12_RESOURCE_STATE_COPY_DEST,
                    false,
                    o.slices,
                )
            }),
            stencil: (o.copyback >= 3 && has_stencil(o.depth)).then(|| {
                let planes = u32::from(o.slices);
                let mut rows = vec![D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default(); planes as usize];
                let mut bytes = 0u64;
                device.GetCopyableFootprints(
                    &depth.GetDesc(),
                    planes,
                    planes,
                    0,
                    Some(rows.as_mut_ptr()),
                    None,
                    None,
                    Some(&mut bytes),
                );
                (
                    target_array(
                        &device,
                        DXGI_FORMAT_R8_UINT,
                        D3D12_RESOURCE_FLAG_NONE,
                        D3D12_RESOURCE_STATE_COPY_DEST,
                        false,
                        o.slices,
                    ),
                    default_buffer(&device, bytes.max(512), false, D3D12_RESOURCE_STATE_COMMON),
                    rows,
                )
            }),
        };
        // (With `MESH_COPYBACK=2` the depth plane, and the stencil plane of a depth format that has one.)
        let mut depth_footprints = Vec::new();
        if o.copyback >= 2 {
            let planes = u32::from(o.slices) * if has_stencil(o.depth) { 2 } else { 1 };
            for plane in 0..planes {
                let mut f = D3D12_PLACED_SUBRESOURCE_FOOTPRINT::default();
                let mut bytes = 0u64;
                device.GetCopyableFootprints(
                    &depth.GetDesc(),
                    plane,
                    1,
                    total,
                    Some(&mut f),
                    None,
                    None,
                    Some(&mut bytes),
                );
                depth_footprints.push((plane, f));
                total = (total + bytes).next_multiple_of(512);
            }
        }
        let back = upload_buffer(&device, total.max(512), "writeback");
        // The indirect arguments: records the CPU writes, clamped by a compute pass into the buffer the
        // executions read.
        const RECORDS: u32 = 64;
        let args_src = upload_buffer(&device, 65536, "upload");
        let args_dst = default_buffer(&device, 65536, true, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        let budget = default_buffer(&device, 4096, true, D3D12_RESOURCE_STATE_UNORDERED_ACCESS);
        let (clamp_root, clamp_pso, signature) = if o.indirect > 0 {
            let code = hlsl::dxc(hlsl::CLAMP, "main", "cs_6_6", &[]);
            let root: ID3D12RootSignature = device.CreateRootSignature(0, &code).unwrap();
            let pso: ID3D12PipelineState = device
                .CreateComputePipelineState(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: code.as_ptr().cast(),
                        BytecodeLength: code.len(),
                    },
                    ..Default::default()
                })
                .unwrap();
            let argument = D3D12_INDIRECT_ARGUMENT_DESC {
                Type: D3D12_INDIRECT_ARGUMENT_TYPE_DISPATCH_MESH,
                ..Default::default()
            };
            let mut signature: Option<ID3D12CommandSignature> = None;
            device
                .CreateCommandSignature(
                    &D3D12_COMMAND_SIGNATURE_DESC {
                        ByteStride: 12,
                        NumArgumentDescs: 1,
                        pArgumentDescs: &argument,
                        NodeMask: 0,
                    },
                    None::<&ID3D12RootSignature>,
                    &mut signature,
                )
                .unwrap();
            (Some(root), Some(pso), signature)
        } else {
            (None, None, None)
        };
        // `MESH_STATS=1`: a pipeline statistics query around each list's draws, resolved into write-back memory.
        let stats: Option<(ID3D12QueryHeap, (ID3D12Resource, *mut u8))> = o.stats.then(|| {
            let mut made: Option<ID3D12QueryHeap> = None;
            device
                .CreateQueryHeap(
                    &D3D12_QUERY_HEAP_DESC {
                        Type: D3D12_QUERY_HEAP_TYPE_PIPELINE_STATISTICS1,
                        Count: o.inflight as u32,
                        NodeMask: 0,
                    },
                    &mut made,
                )
                .expect("a pipeline statistics query heap");
            (
                made.unwrap(),
                upload_buffer(&device, 4096, "writeback"),
            )
        });
        let (mut ms_invocations, mut ms_primitives, mut ps_invocations) = (0u64, 0u64, 0u64);
        // The lists: one for each in flight.
        struct Slot {
            allocator: ID3D12CommandAllocator,
            list: ID3D12GraphicsCommandList6,
            done: u64,
        }
        let mut slots: Vec<Slot> = (0..o.inflight)
            .map(|_| {
                let allocator: ID3D12CommandAllocator = device
                    .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
                    .unwrap();
                let list: ID3D12GraphicsCommandList6 = device
                    .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
                    .unwrap();
                list.Close().unwrap();
                Slot {
                    allocator,
                    list,
                    done: 0,
                }
            })
            .collect();
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event: HANDLE = CreateEventW(None, false, false, None).unwrap();
        // The workers.
        let stop = Arc::new(AtomicBool::new(false));
        let counts = Arc::new(Counts::default());
        let mut threads = Vec::new();
        if has(&o.queues, "copy") || has(&o.queues, "stream") {
            threads.push(copy_worker(
                Share(device.clone()),
                stop.clone(),
                counts.clone(),
                has(&o.queues, "stream"),
                Some((cpu, step)),
            ));
        }
        if has(&o.queues, "compute") {
            threads.push(compute_worker(
                Share(device.clone()),
                stop.clone(),
                counts.clone(),
            ));
        }
        if has(&o.queues, "churn") || has(&o.queues, "race") {
            for k in 0..3 {
                threads.push(churn_worker(
                    Share(views.clone()),
                    stop.clone(),
                    counts.clone(),
                    has(&o.queues, "race"),
                    k,
                ));
            }
        }
        for k in 0..o.memload {
            threads.push(memory_worker(stop.clone(), counts.clone(), k));
        }
        struct Stop(Arc<AtomicBool>, Vec<std::thread::JoinHandle<()>>);
        impl Drop for Stop {
            fn drop(&mut self) {
                self.0.store(true, Relaxed);
                for t in self.1.drain(..) {
                    let _ = t.join();
                }
            }
        }
        let _stop = Stop(stop.clone(), threads);
        let wait = |value: u64, limit: u32| -> bool {
            if fence.GetCompletedValue() >= value {
                return true;
            }
            fence.SetEventOnCompletion(value, event).unwrap();
            WaitForSingleObject(event, limit).0 == 0
        };
        let stuck = |what: &str, n: u64, began: std::time::Instant| {
            println!(
                "THE CARD DID NOT ANSWER in 25 s: {what}, list {n} after {:.1} s; removed: {:?}",
                began.elapsed().as_secs_f64(),
                device.GetDeviceRemovedReason()
            );
            // Windows resets the card after its TdrDelay: wait for that to show.
            for _ in 0..20 {
                if device.GetDeviceRemovedReason().is_err() {
                    break;
                }
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
            println!(
                "device removed reason: {:?}",
                device.GetDeviceRemovedReason()
            );
            if o.dred {
                breadcrumbs(&device);
            }
        };
        let viewport = D3D12_VIEWPORT {
            TopLeftX: 0.0,
            TopLeftY: 0.0,
            Width: WIDTH as f32,
            Height: HEIGHT as f32,
            MinDepth: 0.0,
            MaxDepth: 1.0,
        };
        let scissor = RECT {
            left: 0,
            top: 0,
            right: WIDTH as i32,
            bottom: HEIGHT as i32,
        };
        let clear_flags = if has_stencil(o.depth) {
            D3D12_CLEAR_FLAG_DEPTH | D3D12_CLEAR_FLAG_STENCIL
        } else {
            D3D12_CLEAR_FLAG_DEPTH
        };
        let started = std::time::Instant::now();
        let mut last_print = std::time::Instant::now();
        let (mut lists, mut dispatches, mut indirects) = (0u64, 0u64, 0u64);
        let limit = std::time::Duration::from_secs(o.seconds);
        while started.elapsed() < limit {
            lists += 1;
            let k = (lists as usize) % slots.len();
            if !wait(slots[k].done, 25_000) {
                stuck("the list before", lists, started);
                return;
            }
            if let Some((_, (_, data))) = &stats
                && slots[k].done > 0
            {
                let q = std::ptr::read_unaligned(
                    data.add(k * 256).cast::<D3D12_QUERY_DATA_PIPELINE_STATISTICS1>(),
                );
                ms_invocations += q.MSInvocations;
                ms_primitives += q.MSPrimitives;
                ps_invocations += q.PSInvocations;
            }
            if let Err(e) = device.GetDeviceRemovedReason() {
                println!(
                    "THE DEVICE WAS REMOVED at list {lists} after {:.1} s: {e}",
                    started.elapsed().as_secs_f64()
                );
                if o.dred {
                    breadcrumbs(&device);
                }
                return;
            }
            if o.copyback > 0 {
                // The CPU reads what the card copied back, of an earlier list (which the next may overwrite).
                let words = std::slice::from_raw_parts(back.1.cast::<u32>(), 4096);
                std::hint::black_box(
                    words
                        .iter()
                        .step_by(11)
                        .fold(0u32, |a, w| a.wrapping_add(*w)),
                );
            }
            // What the CPU writes between lists: a few thousand dwords of each buffer, and the indirect records.
            if o.mode != "zero" && o.mode != "ones" {
                for (data, bytes) in &datas {
                    if o.own && *data == vertices.1 {
                        continue;
                    }
                    let words = std::slice::from_raw_parts_mut(data.cast::<u32>(), bytes / 4);
                    for j in 0..2048 {
                        let at = if j < 512 {
                            j
                        } else {
                            rng.next() as usize % words.len()
                        };
                        words[at] = rng.next();
                    }
                }
            }
            if o.indirect > 0 {
                let records =
                    std::slice::from_raw_parts_mut(args_src.1.cast::<u32>(), RECORDS as usize * 3);
                for r in records.chunks_mut(3) {
                    r[0] = match rng.next() % 16 {
                        0 => u32::MAX,
                        1 => rng.next(),
                        2 => 0,
                        _ => 1 + rng.next() % o.groups.max(4) as u32,
                    };
                    r[1] = if rng.next().is_multiple_of(8) {
                        rng.next()
                    } else {
                        1
                    };
                    r[2] = if rng.next().is_multiple_of(8) {
                        rng.next()
                    } else {
                        1
                    };
                }
            }
            let slot = &mut slots[k];
            slot.allocator.Reset().unwrap();
            let list = &slot.list;
            list.Reset(&slot.allocator, None).unwrap();
            list.SetDescriptorHeaps(&[Some(heap.clone())]);
            list.SetGraphicsRootSignature(&root);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu });
            list.SetGraphicsRootDescriptorTable(
                2,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu + (1024 * step) as u64,
                },
            );
            list.SetGraphicsRootDescriptorTable(
                3,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu + (1056 * step) as u64,
                },
            );
            list.RSSetViewports(&[viewport]);
            list.RSSetScissorRects(&[scissor]);
            if o.clear {
                list.ClearDepthStencilView(dsv, clear_flags, 1.0, 0, None);
                for h in &rtv_handles {
                    list.ClearRenderTargetView(*h, &[0.0, 0.0, 0.0, 0.0], None);
                }
            }
            if let Some((heap, _)) = &stats {
                list.BeginQuery(heap, D3D12_QUERY_TYPE_PIPELINE_STATISTICS1, k as u32);
            }
            let mut bound: Option<bool> = None;
            let mut current = usize::MAX;
            for d in 0..o.draws {
                let at = rng.next() as usize % pipes.len();
                let (pso, with_ps) = &pipes[at];
                if bound != Some(*with_ps) {
                    if *with_ps {
                        list.OMSetRenderTargets(
                            colour_count as u32,
                            Some(rtv_handles.as_ptr()),
                            false,
                            Some(&dsv),
                        );
                    } else {
                        list.OMSetRenderTargets(0, None, false, Some(&dsv));
                    }
                    bound = Some(*with_ps);
                }
                if current != at {
                    list.SetPipelineState(pso);
                    current = at;
                }
                if o.stencil {
                    list.OMSetStencilRef(rng.next() & 0xff);
                }
                // A vertex draw between the mesh ones: the other kind of pipeline, the same targets.
                if let Some(vpso) = &vertex_pso
                    && d % o.vertex == o.vertex - 1
                {
                    if bound != Some(true) {
                        list.OMSetRenderTargets(
                            colour_count as u32,
                            Some(rtv_handles.as_ptr()),
                            false,
                            Some(&dsv),
                        );
                        bound = Some(true);
                    }
                    list.SetPipelineState(vpso);
                    list.IASetPrimitiveTopology(
                        windows::Win32::Graphics::Direct3D::D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST,
                    );
                    list.DrawInstanced(3, 1, 0, 0);
                    // The mesh pipeline goes back on the next draw.
                    current = usize::MAX;
                }
                // The user data: the table of the draw, and by the setting random words.
                let mut user = [0u32; 32];
                if o.user_fuzz {
                    for w in user.iter_mut() {
                        *w = match rng.next() % 6 {
                            0 => 0,
                            1 => u32::MAX,
                            2 => rng.next(),
                            3 => rng.next() & 0x7ff,
                            4 => 1 << (rng.next() % 32),
                            _ => rng.next() | 0x1000_0000,
                        };
                    }
                }
                if o.user_sane {
                    for w in user.iter_mut() {
                        *w = match rng.next() % 4 {
                            0 => 0,
                            1 => TABLES[rng.next() as usize % TABLES.len()] * 32 + 32 * (rng.next() % 4),
                            2 => rng.next() & 0x3ff,
                            _ => rng.next() & 0xffff,
                        };
                    }
                }
                user[9] = TABLES[(d as usize) % TABLES.len()] * 32;
                list.SetGraphicsRoot32BitConstants(0, 32, user.as_ptr().cast(), 0);
                let groups = 1 + rng.next() % o.groups as u32;
                if o.indirect > 0 && d % o.indirect == o.indirect - 1 {
                    let (clamp_root, clamp_pso, signature) = (
                        clamp_root.as_ref().unwrap(),
                        clamp_pso.as_ref().unwrap(),
                        signature.as_ref().unwrap(),
                    );
                    list.SetComputeRootSignature(clamp_root);
                    list.SetPipelineState(clamp_pso);
                    let constants = [RECORDS, o.groups.max(4) as u32, 0, 0];
                    list.SetComputeRoot32BitConstants(0, 4, constants.as_ptr().cast(), 0);
                    list.SetComputeRootShaderResourceView(1, args_src.0.GetGPUVirtualAddress());
                    list.SetComputeRootUnorderedAccessView(2, args_dst.GetGPUVirtualAddress());
                    list.SetComputeRootUnorderedAccessView(3, budget.GetGPUVirtualAddress());
                    list.Dispatch(RECORDS.div_ceil(64), 1, 1);
                    list.ResourceBarrier(&[
                        uav_barrier(&args_dst),
                        transition(
                            &args_dst,
                            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                            D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                        ),
                    ]);
                    list.SetPipelineState(pso);
                    if o.batch == 0 {
                        list.ExecuteIndirect(
                            signature,
                            RECORDS,
                            &args_dst,
                            0,
                            None::<&ID3D12Resource>,
                            0,
                        );
                    } else {
                        // One record each, one after another: new root constants every time, another pipeline now
                        // and then.
                        for r in 0..o.batch {
                            if r % 7 == 6 {
                                let (other, _) = &pipes[rng.next() as usize % pipes.len()];
                                list.SetPipelineState(other);
                            }
                            user[8] = rng.next() & 0xffff;
                            user[10] = r as u32 * 0x120;
                            list.SetGraphicsRoot32BitConstants(0, 32, user.as_ptr().cast(), 0);
                            list.ExecuteIndirect(
                                signature,
                                1,
                                &args_dst,
                                r * 12,
                                None::<&ID3D12Resource>,
                                0,
                            );
                            indirects += 1;
                        }
                        list.SetPipelineState(pso);
                    }
                    list.ResourceBarrier(&[transition(
                        &args_dst,
                        D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                        D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
                    )]);
                    indirects += 1;
                } else {
                    list.DispatchMesh(groups, 1, 1);
                }
                dispatches += 1;
            }
            if let Some((heap, (buffer, _))) = &stats {
                list.EndQuery(heap, D3D12_QUERY_TYPE_PIPELINE_STATISTICS1, k as u32);
                list.ResolveQueryData(
                    heap,
                    D3D12_QUERY_TYPE_PIPELINE_STATISTICS1,
                    k as u32,
                    1,
                    buffer,
                    (k * 256) as u64,
                );
            }
            if o.copyback > 0 {
                let to_source: Vec<D3D12_RESOURCE_BARRIER> = colours
                    .iter()
                    .map(|c| {
                        transition(
                            c,
                            D3D12_RESOURCE_STATE_RENDER_TARGET,
                            D3D12_RESOURCE_STATE_COPY_SOURCE,
                        )
                    })
                    .collect();
                list.ResourceBarrier(&to_source);
                if !depth_footprints.is_empty() {
                    list.ResourceBarrier(&[transition(
                        &depth,
                        D3D12_RESOURCE_STATE_DEPTH_WRITE,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                    )]);
                    for (plane, f) in &depth_footprints {
                        let to = D3D12_TEXTURE_COPY_LOCATION {
                            pResource: std::mem::transmute_copy(&back.0),
                            Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                                PlacedFootprint: *f,
                            },
                        };
                        let from = D3D12_TEXTURE_COPY_LOCATION {
                            pResource: std::mem::transmute_copy(&depth),
                            Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                            Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                                SubresourceIndex: *plane,
                            },
                        };
                        list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                    }
                    if o.copyback >= 3 {
                        twin_copies(list, &depth, &twins);
                    }
                    list.ResourceBarrier(&[transition(
                        &depth,
                        D3D12_RESOURCE_STATE_COPY_SOURCE,
                        D3D12_RESOURCE_STATE_DEPTH_WRITE,
                    )]);
                }
                for (c, f) in colours.iter().zip(&footprints) {
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(&back.0),
                        Type: D3D12_TEXTURE_COPY_TYPE_PLACED_FOOTPRINT,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            PlacedFootprint: *f,
                        },
                    };
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(c),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 {
                            SubresourceIndex: 0,
                        },
                    };
                    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                }
                for (c, copy) in colours.iter().zip(&colour_copies) {
                    let to = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(copy),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                    };
                    let from = D3D12_TEXTURE_COPY_LOCATION {
                        pResource: std::mem::transmute_copy(c),
                        Type: D3D12_TEXTURE_COPY_TYPE_SUBRESOURCE_INDEX,
                        Anonymous: D3D12_TEXTURE_COPY_LOCATION_0 { SubresourceIndex: 0 },
                    };
                    list.CopyTextureRegion(&to, 0, 0, 0, &from, None);
                }
                let to_target: Vec<D3D12_RESOURCE_BARRIER> = colours
                    .iter()
                    .map(|c| {
                        transition(
                            c,
                            D3D12_RESOURCE_STATE_COPY_SOURCE,
                            D3D12_RESOURCE_STATE_RENDER_TARGET,
                        )
                    })
                    .collect();
                list.ResourceBarrier(&to_target);
            }
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
            slot.done = lists;
            queue.Signal(&fence, lists).unwrap();
            if o.sleep_ms > 0 {
                std::thread::sleep(std::time::Duration::from_millis(o.sleep_ms));
            }
            if last_print.elapsed().as_secs() >= 10 {
                last_print = std::time::Instant::now();
                println!(
                    "  {:.0} s: {lists} lists, {dispatches} dispatches ({indirects} indirect); copies {}, computes {}, streamed {}, rewrites {}, memory moved {} MB; the card ran {ms_invocations} mesh invocations, {ms_primitives} primitives, {ps_invocations} pixels",
                    started.elapsed().as_secs_f64(),
                    counts.copies.load(Relaxed),
                    counts.computes.load(Relaxed),
                    counts.streamed.load(Relaxed),
                    counts.rewrites.load(Relaxed),
                    counts.moved_mb.load(Relaxed)
                );
                if counts.stuck.load(Relaxed) {
                    stuck("a worker queue", lists, started);
                    return;
                }
            }
        }
        // The last lists.
        for s in &slots {
            if !wait(s.done, 25_000) {
                stuck("the last lists", lists, started);
                return;
            }
        }
        if let Err(e) = device.GetDeviceRemovedReason() {
            println!(
                "THE DEVICE WAS REMOVED by the end after {:.1} s: {e}",
                started.elapsed().as_secs_f64()
            );
            return;
        }
        println!(
            "no hang in {:.0} s: {lists} lists, {dispatches} dispatches ({indirects} indirect); copies {}, computes {}, streamed {}, rewrites {}",
            started.elapsed().as_secs_f64(),
            counts.copies.load(Relaxed),
            counts.computes.load(Relaxed),
            counts.streamed.load(Relaxed),
            counts.rewrites.load(Relaxed)
        );
    }
    drop(held);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_list_is_split_at_its_commas() {
        assert!(has("copy, compute", "compute"));
        assert!(!has("copy,compute", "comp"));
        assert!(!has("", "copy"));
    }

    #[test]
    fn the_first_state_is_the_plain_one_and_the_others_differ() {
        let key = |s: State| {
            (
                s.cull.0,
                s.func.0,
                s.write,
                s.bias,
                s.slope.to_bits(),
                s.clip,
                s.blend,
            )
        };
        assert_eq!(
            key(state(0, false)),
            (
                D3D12_CULL_MODE_NONE.0,
                D3D12_COMPARISON_FUNC_LESS.0,
                true,
                0,
                0,
                true,
                false
            )
        );
        let keys: std::collections::HashSet<_> = (0..1152).map(|k| key(state(k, false))).collect();
        assert_eq!(keys.len(), 1152);
    }

    #[test]
    fn pixel_programs_write_every_target_and_no_more() {
        let own = hlsl::own_pixel(4);
        assert!((0..4).all(|k| own.contains(&format!("SV_Target{k}"))));
        assert!(!own.contains("SV_Target4"));
        assert!(flat_pixel(2).contains("SV_Target1"));
        assert!(!flat_pixel(2).contains("SV_Target2"));
    }

    #[test]
    fn only_the_formats_with_a_stencil_plane_say_so() {
        assert!(has_stencil(DXGI_FORMAT_D32_FLOAT_S8X24_UINT));
        assert!(has_stencil(DXGI_FORMAT_D24_UNORM_S8_UINT));
        assert!(!has_stencil(DXGI_FORMAT_D32_FLOAT));
        assert!(!has_stencil(DXGI_FORMAT_D16_UNORM));
    }
}
