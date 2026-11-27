//! The compute twin of the title's mesh programs (`VICTORY_MESH_TWIN`, off by
//! default; `docs/intel-arc.md`, "A compute twin for the mesh draws"): the
//! driver's mesh pipeline stops the Arc B390 within a minute or two of the
//! world, so a mesh program the setting names runs as a compute program of
//! the same code that stores the vertices and primitive indices the mesh
//! program would export into one arena buffer
//! (`victory_shader::emit::SLOT_ARENA` has the layout).
//!
//! `VICTORY_MESH_TWIN=<hash>,<hash>...` names mesh programs by the first digits
//! of the hashes a run's log gives them (`all` names every one).
//!
//! - Stage A (`VICTORY_MESH_TWIN` alone): the twin runs in place of each mesh
//!   draw of those programs and writes the arena; nothing is drawn. A run
//!   that still loses the card in the world says the hang is in what the
//!   programs do and read, not in the mesh pipeline.
//! - Stage B (`VICTORY_MESH_TWIN_DRAW=1` too): a vertex shader reads the arena
//!   and draws the triangles with the title's pixel shader and state, one
//!   draw of `3 * 128 * groups` vertices per mesh draw. Each draw owns the
//!   arena from its start: a pass of ours reads the title's record, clamps the
//!   groups to what the arena holds and writes the twin's dispatch and the
//!   draw's arguments; a UAV barrier follows each step.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::ID3DBlob;
use windows::Win32::Graphics::Direct3D12::*;
use windows::core::{Interface, s};

use crate::host::host;
use crate::object::This;
use crate::pipeline::host::HostPipeline;

/// Which mesh programs run as twins.
enum Named {
    Nothing,
    All,
    Some(Vec<String>),
}

fn named() -> &'static Named {
    static NAMED: OnceLock<Named> = OnceLock::new();
    NAMED.get_or_init(|| match std::env::var("VICTORY_MESH_TWIN") {
        Ok(v) if v.trim() == "all" => {
            tracing::warn!("VICTORY_MESH_TWIN=all: every mesh program runs as a compute twin (drawn: {})", draws());
            Named::All
        }
        Ok(v) => {
            let hashes: Vec<String> = v
                .split(',')
                .map(|h| h.trim().to_lowercase())
                .filter(|h| !h.is_empty())
                .collect();
            if hashes.is_empty() {
                Named::Nothing
            } else {
                tracing::warn!(?hashes, "VICTORY_MESH_TWIN: these mesh programs run as compute twins (drawn: {})", draws());
                Named::Some(hashes)
            }
        }
        Err(_) => Named::Nothing,
    })
}

/// Whether the twins' triangles are drawn (`VICTORY_MESH_TWIN_DRAW=1`).
pub(crate) fn draws() -> bool {
    static ON: OnceLock<bool> = OnceLock::new();
    *ON.get_or_init(|| std::env::var_os("VICTORY_MESH_TWIN_DRAW").is_some_and(|v| v == "1"))
}

/// Whether the mesh pipeline `p` runs as a twin.
pub(crate) fn wanted(p: &HostPipeline) -> bool {
    if !p.mesh {
        return false;
    }
    match named() {
        Named::Nothing => false,
        Named::All => true,
        Named::Some(hashes) => p
            .stages
            .first()
            .is_some_and(|s| hashes.iter().any(|h| s.shader.hash.starts_with(h.as_str()))),
    }
}

/// What a mesh pipeline's twin is made of.
pub(crate) struct Made {
    /// The compute pipeline.
    pub(crate) compute: HostPipeline,
    /// The most groups of one draw the arena has room for.
    pub(crate) cap: u32,
    /// The vertex and pixel pipeline, made on the first draw.
    draw: OnceLock<Option<HostPipeline>>,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Made {}
// SAFETY: as above.
unsafe impl Sync for Made {}

impl Made {
    /// The pipeline that draws the arena with the title's pixel shader and
    /// state: the title's pipeline `this` is the mesh pipeline `p` was made
    /// for.
    pub(crate) fn draw(&self, this: This) -> Option<&HostPipeline> {
        self.draw
            .get_or_init(|| {
                let (shaders, g) = crate::pipeline::parts(this)?;
                let ms = shaders.iter().find(|s| s.stage == "mesh")?;
                let mut stages = vec![crate::pipeline::twin_vertex_of(ms)];
                stages.extend(shaders.iter().find(|s| s.stage == "pixel").cloned());
                let made = HostPipeline::new_for(&stages, Some(g), true);
                match &made {
                    Some(_) => tracing::info!(mesh = %ms.hash, "compute twin's drawing pipeline made"),
                    None => tracing::warn!(mesh = %ms.hash, "a mesh program's twin has no drawing pipeline"),
                }
                made
            })
            .as_ref()
    }
}

/// The twin of the mesh pipeline `p`, made on first use (`None`: it does not
/// translate or the host refused it; logged once).
pub(crate) fn pipeline(p: &HostPipeline) -> Option<&'static Made> {
    static MADE: Mutex<Option<HashMap<usize, Option<&'static Made>>>> = Mutex::new(None);
    let key = p.pso.as_raw() as usize;
    let mut made = MADE.lock().unwrap_or_else(|e| e.into_inner());
    let made = made.get_or_insert_with(HashMap::new);
    if let Some(t) = made.get(&key) {
        return *t;
    }
    let twin = p.stages.first().and_then(|ms| {
        let shader = crate::pipeline::twin_of(&ms.shader);
        let cap = victory_shader::emit::host::twin_groups(&ms.translated().host).ok()?;
        let compute = HostPipeline::new(&[shader], None);
        match &compute {
            Some(_) => tracing::info!(mesh = %ms.shader.hash, cap, "compute twin made"),
            None => tracing::warn!(mesh = %ms.shader.hash, "a mesh program has no compute twin (it does not translate, or the host refused it)"),
        }
        let compute = compute?;
        let twin: &'static Made = Box::leak(Box::new(Made { compute, cap, draw: OnceLock::new() }));
        Some(twin)
    });
    made.insert(key, twin);
    twin
}

struct Buffer(ID3D12Resource, u64);

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Buffer {}
// SAFETY: as above.
unsafe impl Sync for Buffer {}

/// A buffer of `bytes` in the card's memory for unordered access, in that
/// state, and its GPU address.
fn buffer(bytes: u64) -> Option<Buffer> {
    let device = &host()?.device;
    let heap = D3D12_HEAP_PROPERTIES {
        Type: D3D12_HEAP_TYPE_DEFAULT,
        ..Default::default()
    };
    let desc = D3D12_RESOURCE_DESC {
        Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
        Width: bytes,
        Height: 1,
        DepthOrArraySize: 1,
        MipLevels: 1,
        SampleDesc: windows::Win32::Graphics::Dxgi::Common::DXGI_SAMPLE_DESC { Count: 1, Quality: 0 },
        Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
        Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
        ..Default::default()
    };
    let mut out: Option<ID3D12Resource> = None;
    // SAFETY: a plain buffer.
    let made = unsafe {
        device.CreateCommittedResource(
            &heap,
            D3D12_HEAP_FLAG_NONE,
            &desc,
            D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            None,
            &mut out,
        )
    };
    match (made, out) {
        (Ok(()), Some(buffer)) => {
            // SAFETY: a live buffer.
            let gpu = unsafe { buffer.GetGPUVirtualAddress() };
            Some(Buffer(buffer, gpu))
        }
        (made, _) => {
            tracing::warn!(error = ?made.err(), bytes, "a mesh twin buffer could not be made");
            None
        }
    }
}

/// The arena the twins write: one buffer of `ARENA_BYTES` in the card's
/// memory, kept in the unordered-access state (every use is a UAV barrier
/// away from the last), and its GPU address.
pub(crate) fn arena() -> Option<(&'static ID3D12Resource, u64)> {
    static ARENA: OnceLock<Option<Buffer>> = OnceLock::new();
    ARENA
        .get_or_init(|| {
            let made = buffer(u64::from(victory_shader::emit::ARENA_BYTES));
            if made.is_some() {
                tracing::info!(bytes = victory_shader::emit::ARENA_BYTES, "mesh twin arena made");
            }
            made
        })
        .as_ref()
        .map(|a| (&a.0, a.1))
}

/// The twin's argument records: a dispatch (`x, 1, 1`) at byte 0 and a
/// draw (`vertices, 1, 0, 0`) at byte 16, written by [`prepare`]. Left in the
/// unordered-access state between draws.
pub(crate) fn arguments() -> Option<&'static ID3D12Resource> {
    static ARGUMENTS: OnceLock<Option<Buffer>> = OnceLock::new();
    ARGUMENTS.get_or_init(|| buffer(256)).as_ref().map(|a| &a.0)
}

/// A barrier that orders all unordered access before it (the arena's, and
/// whatever else the list wrote) against all after it.
pub(crate) fn barrier(list: &ID3D12GraphicsCommandList) {
    let barrier = D3D12_RESOURCE_BARRIER {
        Type: D3D12_RESOURCE_BARRIER_TYPE_UAV,
        Flags: D3D12_RESOURCE_BARRIER_FLAG_NONE,
        Anonymous: D3D12_RESOURCE_BARRIER_0 {
            UAV: std::mem::ManuallyDrop::new(D3D12_RESOURCE_UAV_BARRIER {
                pResource: std::mem::ManuallyDrop::new(None),
            }),
        },
    };
    // SAFETY: an open list.
    unsafe { list.ResourceBarrier(&[barrier]) };
}

const PREPARE: &str = r#"
#define RS "RootConstants(num32BitConstants=2, b0), SRV(t0), UAV(u0)"
cbuffer Limits : register(b0) {
    uint cap;
    uint corners;
};
ByteAddressBuffer record : register(t0);
RWByteAddressBuffer arguments : register(u0);
[RootSignature(RS)]
[numthreads(1, 1, 1)]
void main() {
    uint3 g = record.Load3(0);
    // The clamp's rule for a mesh record: more than a dispatch runs is a slot
    // the title has not filled (nothing), and its dispatches are one-dimensional.
    uint x = (g.x > 65535u || g.y == 0u || g.z == 0u) ? 0u : min(g.x, cap);
    arguments.Store3(0, uint3(x, 1u, 1u));
    arguments.Store4(16, uint4(x * corners, 1u, 0u, 0u));
}
"#;

struct Prepare {
    root: ID3D12RootSignature,
    pso: ID3D12PipelineState,
}

// SAFETY: D3D12 objects are free-threaded.
unsafe impl Send for Prepare {}
// SAFETY: as above.
unsafe impl Sync for Prepare {}

fn prepare_pass() -> Option<&'static Prepare> {
    static PASS: OnceLock<Option<Prepare>> = OnceLock::new();
    PASS.get_or_init(|| {
        let device = &host()?.device;
        let mut code: Option<ID3DBlob> = None;
        let mut errors: Option<ID3DBlob> = None;
        // SAFETY: the source and names live through the call; out pointers ours.
        let compiled = unsafe {
            D3DCompile(
                PREPARE.as_ptr().cast(),
                PREPARE.len(),
                s!("twin"),
                None,
                None,
                s!("main"),
                s!("cs_5_1"),
                0,
                0,
                &mut code,
                Some(&mut errors),
            )
        };
        if let Err(e) = compiled {
            if let Some(errors) = errors {
                // SAFETY: the blob's own bytes.
                let text = unsafe {
                    std::slice::from_raw_parts(errors.GetBufferPointer().cast::<u8>(), errors.GetBufferSize())
                };
                tracing::warn!(error = %e, "mesh twin argument pass: {}", String::from_utf8_lossy(text));
            }
            return None;
        }
        let code = code?;
        // SAFETY: the blob's own bytes; the root signature is inside the shader.
        let made = unsafe {
            let bytes = std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
            device.CreateRootSignature::<ID3D12RootSignature>(0, bytes).and_then(|root| {
                let pso = device.CreateComputePipelineState::<ID3D12PipelineState>(&D3D12_COMPUTE_PIPELINE_STATE_DESC {
                    pRootSignature: std::mem::transmute_copy(&root),
                    CS: D3D12_SHADER_BYTECODE {
                        pShaderBytecode: bytes.as_ptr().cast(),
                        BytecodeLength: bytes.len(),
                    },
                    ..Default::default()
                })?;
                Ok(Prepare { root, pso })
            })
        };
        match made {
            Ok(pass) => Some(pass),
            Err(e) => {
                tracing::warn!(error = %e, "the mesh twin's argument pass was refused");
                None
            }
        }
    })
    .as_ref()
}

/// Records the argument pass: the twin's dispatch and draw records, from the
/// title's mesh record at `record` (a GPU address; its first 12 bytes), for
/// at most `cap` groups, into [`arguments`], which is then readable as
/// indirect arguments (move it back with [`release`]). False if it cannot be
/// recorded.
pub(crate) fn prepare(list: &ID3D12GraphicsCommandList, record: u64, cap: u32) -> bool {
    let (Some(pass), Some(arguments)) = (prepare_pass(), arguments()) else {
        return false;
    };
    if !record.is_multiple_of(4) {
        return false;
    }
    let constants = [cap, 3 * 128];
    // SAFETY: an open list; our root signature and pipeline; live buffers.
    unsafe {
        // The title's earlier shader writes (the record's), and the last
        // draw's reads of the arguments, first.
        barrier(list);
        list.SetComputeRootSignature(&pass.root);
        list.SetPipelineState(&pass.pso);
        list.SetComputeRoot32BitConstants(0, 2, constants.as_ptr().cast(), 0);
        list.SetComputeRootShaderResourceView(1, record);
        list.SetComputeRootUnorderedAccessView(2, arguments.GetGPUVirtualAddress());
        list.Dispatch(1, 1, 1);
        list.ResourceBarrier(&[
            crate::host::transition(arguments, D3D12_RESOURCE_STATE_UNORDERED_ACCESS, D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT),
        ]);
    }
    true
}

/// Moves [`arguments`] back to the unordered-access state after the twin's
/// draw read it.
pub(crate) fn release(list: &ID3D12GraphicsCommandList) {
    if let Some(arguments) = arguments() {
        // SAFETY: an open list.
        unsafe {
            list.ResourceBarrier(&[crate::host::transition(
                arguments,
                D3D12_RESOURCE_STATE_INDIRECT_ARGUMENT,
                D3D12_RESOURCE_STATE_UNORDERED_ACCESS,
            )]);
        }
    }
}
