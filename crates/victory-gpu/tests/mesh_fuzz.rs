//! A stress test of one translated mesh shader on the PC's own adapter: the DXIL the translator
//! made of a title program (`VICTORY_EMIT_DUMP=<dir outside the repo> victory.exe translate
//! --validate <shaders.rpf>`, with `VICTORY_TRANSLATE_ONLY=<hash>`), dispatched many times over
//! buffers of garbage or of small values, with the bindings the host gives such a program: its user
//! data as root constants, two raw views picked from a heap of 1,000,000 views by an index in the user
//! data, a constant buffer picked the same way, and the vertex buffer as a one-view table.
//!
//! Why: on an Arc B390 every run of the title that issued mesh draws stopped the card within a
//! minute or two of the world (`docs/intel-arc.md`), whichever shaders were drawn, and none that
//! issued none did. This says whether the card stops on the shader alone, and with what data.
//!
//! ```text
//! MESH_DXIL=<file> MESH_SRV=<register of its vertex data, t<n> in space 1: see `dxc -dumpbin`>
//!   MESH_VIEWS=raw|uninit|null2d|nullbuf|tex|typed|structured|uav|swap MESH_MODE=garbage|small|zero|ones MESH_ITERS=2000 MESH_DRAWS=40 MESH_GROUPS=1 MESH_DEBUG=1 \
//!   cargo test -p victory-gpu --test mesh_fuzz -- --ignored --nocapture
//! ```
//!
//! It prints how many lists ran before the card stopped (or that none did). The card resets when it
//! stops: the screen may flicker.
//!
//! A second test, `the_titles_load_around_a_mesh_shader`, adds what the title's run has and this one lacks
//! (root constants that vary, real triangles, the title's targets and pixel shaders, more queues, a heap
//! that is rewritten, many pipelines, indirect dispatches, memory pressure), a switch each: see `load.rs`.
//!
//! ```text
//! MESH_DXIL=<file> MESH_OWN=1 MESH_USER=fuzz MESH_QUEUES=copy,compute,churn MESH_SECONDS=120 \
//!   cargo test -p victory-gpu --test mesh_fuzz the_titles_load -- --ignored --nocapture
//! ```
#![cfg(windows)]

#[path = "mesh_fuzz/hlsl.rs"]
mod hlsl;
#[path = "mesh_fuzz/load.rs"]
mod load;

use windows::Win32::Foundation::HANDLE;
use windows::Win32::Graphics::Direct3D::Fxc::D3DCompile;
use windows::Win32::Graphics::Direct3D::{
    D3D_FEATURE_LEVEL_12_0, D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST, ID3DBlob,
};
use windows::Win32::Graphics::Direct3D12::*;
use windows::Win32::Graphics::Dxgi::Common::*;
use windows::Win32::System::Threading::{CreateEventW, WaitForSingleObject};
use windows::core::{Interface, s};

const VIEWS: u32 = 1_000_000;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
/// The slots of the draws' tables: the first of five (a raw view, nothing, a raw view, a constant
/// buffer, nothing).
const TABLES: [u32; 4] = [847_414, 927_683, 933_191, 935_184];

fn env_u64(name: &str, default: u64) -> u64 {
    std::env::var(name)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

/// A xorshift generator: the same data for the same seed.
struct Rng(u64);

impl Rng {
    fn next(&mut self) -> u32 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        (self.0 >> 16) as u32
    }
}

/// A buffer the CPU writes and the card reads: `upload` memory (write-combined), or `writeback`, the PC's
/// memory the CPU caches (the host's "shared" buffers: a custom heap of the PC's pool).
fn upload_buffer(device: &ID3D12Device, bytes: u64, memory: &str) -> (ID3D12Resource, *mut u8) {
    let heap = if memory == "writeback" {
        D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_CUSTOM,
            CPUPageProperty: D3D12_CPU_PAGE_PROPERTY_WRITE_BACK,
            MemoryPoolPreference: D3D12_MEMORY_POOL_L0,
            ..Default::default()
        }
    } else {
        D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_UPLOAD,
            ..Default::default()
        }
    };
    let state = if memory == "writeback" {
        D3D12_RESOURCE_STATE_COMMON
    } else {
        D3D12_RESOURCE_STATE_GENERIC_READ
    };
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
        ..Default::default()
    };
    let mut made: Option<ID3D12Resource> = None;
    let mut data = std::ptr::null_mut();
    // SAFETY: plain creation and mapping.
    unsafe {
        device
            .CreateCommittedResource(&heap, D3D12_HEAP_FLAG_NONE, &desc, state, None, &mut made)
            .unwrap();
        let made = made.unwrap();
        made.Map(0, None, Some(&mut data)).unwrap();
        (made, data.cast())
    }
}

fn root_signature(
    device: &ID3D12Device,
    vertex_register: u32,
    input_assembler: bool,
) -> ID3D12RootSignature {
    // (Leaked: the description is read once, here.)
    let range = |register: u32, space: u32| -> &'static D3D12_DESCRIPTOR_RANGE1 {
        Box::leak(Box::new(D3D12_DESCRIPTOR_RANGE1 {
            RangeType: D3D12_DESCRIPTOR_RANGE_TYPE_SRV,
            NumDescriptors: 1,
            BaseShaderRegister: register,
            RegisterSpace: space,
            Flags: D3D12_DESCRIPTOR_RANGE_FLAG_DESCRIPTORS_VOLATILE
                | D3D12_DESCRIPTOR_RANGE_FLAG_DATA_VOLATILE,
            OffsetInDescriptorsFromTableStart: 0,
        }))
    };
    let table = |ranges: &'static D3D12_DESCRIPTOR_RANGE1| D3D12_ROOT_PARAMETER1 {
        ParameterType: D3D12_ROOT_PARAMETER_TYPE_DESCRIPTOR_TABLE,
        Anonymous: D3D12_ROOT_PARAMETER1_0 {
            DescriptorTable: D3D12_ROOT_DESCRIPTOR_TABLE1 {
                NumDescriptorRanges: 1,
                pDescriptorRanges: ranges,
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
                    Num32BitValues: 16,
                },
            },
            ShaderVisibility: D3D12_SHADER_VISIBILITY_ALL,
        },
        table(range(255, 1)),
        table(range(vertex_register, 1)),
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
    // SAFETY: the description points at live arrays; the blob is read in full before it is released.
    unsafe {
        let mut blob: Option<ID3DBlob> = None;
        let mut error: Option<ID3DBlob> = None;
        if let Err(e) = D3D12SerializeVersionedRootSignature(&desc, &mut blob, Some(&mut error)) {
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

fn mesh_pipeline(
    device: &ID3D12Device,
    root: &ID3D12RootSignature,
    ms: &[u8],
) -> windows::core::Result<ID3D12PipelineState> {
    #[repr(C, align(8))]
    struct Sub<T>(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE, T);
    #[repr(C)]
    struct Stream {
        root: Sub<*mut std::ffi::c_void>,
        ms: Sub<D3D12_SHADER_BYTECODE>,
        blend: Sub<D3D12_BLEND_DESC>,
        mask: Sub<u32>,
        raster: Sub<D3D12_RASTERIZER_DESC>,
        depth: Sub<D3D12_DEPTH_STENCIL_DESC>,
        targets: Sub<D3D12_RT_FORMAT_ARRAY>,
        dsv: Sub<DXGI_FORMAT>,
        samples: Sub<DXGI_SAMPLE_DESC>,
        topology: Sub<D3D12_PRIMITIVE_TOPOLOGY_TYPE>,
    }
    let mut stream = Stream {
        root: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_ROOT_SIGNATURE,
            root.as_raw(),
        ),
        ms: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_MS,
            D3D12_SHADER_BYTECODE {
                pShaderBytecode: ms.as_ptr().cast(),
                BytecodeLength: ms.len(),
            },
        ),
        blend: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_BLEND,
            D3D12_BLEND_DESC::default(),
        ),
        mask: Sub(D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_SAMPLE_MASK, u32::MAX),
        raster: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RASTERIZER,
            D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                DepthClipEnable: true.into(),
                ..Default::default()
            },
        ),
        depth: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL,
            D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_LESS,
                ..Default::default()
            },
        ),
        targets: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_RENDER_TARGET_FORMATS,
            D3D12_RT_FORMAT_ARRAY::default(),
        ),
        dsv: Sub(
            D3D12_PIPELINE_STATE_SUBOBJECT_TYPE_DEPTH_STENCIL_FORMAT,
            DXGI_FORMAT_D32_FLOAT,
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

/// A vertex shader pipeline: a triangle by vertex id into the depth target. The draws that the title's mesh
/// draws are mixed with have root signatures of their own, so lists switch between the two kinds.
fn vertex_pipeline(device: &ID3D12Device) -> (ID3D12RootSignature, ID3D12PipelineState) {
    const SOURCE: &str = r#"
#define RS "RootFlags(ALLOW_INPUT_ASSEMBLER_INPUT_LAYOUT)"
[RootSignature(RS)]
float4 vs(uint i : SV_VertexID) : SV_Position {
    float2 p = float2((i << 1) & 2, i & 2);
    return float4(p * 2 - 1, 0.5 + 0.001 * i, 1);
}
"#;
    // SAFETY: the source and names live through the call; the blobs' own bytes.
    unsafe {
        let (mut code, mut errors): (Option<ID3DBlob>, Option<ID3DBlob>) = (None, None);
        let r = D3DCompile(
            SOURCE.as_ptr().cast(),
            SOURCE.len(),
            None,
            None,
            None,
            s!("vs"),
            s!("vs_5_1"),
            1 << 20,
            0,
            &mut code,
            Some(&mut errors),
        );
        if r.is_err() {
            let e = errors.unwrap();
            panic!(
                "{}",
                String::from_utf8_lossy(std::slice::from_raw_parts(
                    e.GetBufferPointer().cast::<u8>(),
                    e.GetBufferSize()
                ))
            );
        }
        let code = code.unwrap();
        let bytes =
            std::slice::from_raw_parts(code.GetBufferPointer().cast::<u8>(), code.GetBufferSize());
        let root: ID3D12RootSignature = device.CreateRootSignature(0, bytes).unwrap();
        let desc = D3D12_GRAPHICS_PIPELINE_STATE_DESC {
            pRootSignature: std::mem::transmute_copy(&root),
            VS: D3D12_SHADER_BYTECODE {
                pShaderBytecode: bytes.as_ptr().cast(),
                BytecodeLength: bytes.len(),
            },
            RasterizerState: D3D12_RASTERIZER_DESC {
                FillMode: D3D12_FILL_MODE_SOLID,
                CullMode: D3D12_CULL_MODE_NONE,
                ..Default::default()
            },
            DepthStencilState: D3D12_DEPTH_STENCIL_DESC {
                DepthEnable: true.into(),
                DepthWriteMask: D3D12_DEPTH_WRITE_MASK_ALL,
                DepthFunc: D3D12_COMPARISON_FUNC_LESS,
                ..Default::default()
            },
            SampleMask: u32::MAX,
            PrimitiveTopologyType: D3D12_PRIMITIVE_TOPOLOGY_TYPE_TRIANGLE,
            DSVFormat: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            ..Default::default()
        };
        let pso: ID3D12PipelineState = device.CreateGraphicsPipelineState(&desc).unwrap();
        (root, pso)
    }
}

/// The debug layer's stored messages (`MESH_DEBUG=1`).
fn messages(device: &ID3D12Device) {
    // SAFETY: the info queue's own buffers, read as it says.
    unsafe {
        let Ok(queue) = device.cast::<ID3D12InfoQueue>() else {
            return;
        };
        for i in 0..queue.GetNumStoredMessages() {
            let mut len = 0usize;
            let _ = queue.GetMessage(i, None, &mut len);
            let mut buffer = vec![0u8; len];
            let message = buffer.as_mut_ptr().cast::<D3D12_MESSAGE>();
            if queue.GetMessage(i, Some(message), &mut len).is_ok() {
                let text = std::ffi::CStr::from_ptr((*message).pDescription.cast())
                    .to_string_lossy()
                    .into_owned();
                println!("D3D12: {text}");
            }
        }
    }
}

/// What the shader reads is filled by the mode: garbage (every dword random), small (random,
/// under 256), zero or ones.
fn fill(mode: &str, rng: &mut Rng, data: *mut u8, bytes: usize) {
    // SAFETY: the buffer's whole mapping.
    let words = unsafe { std::slice::from_raw_parts_mut(data.cast::<u32>(), bytes / 4) };
    match mode {
        "zero" => words.fill(0),
        "ones" => words.fill(u32::MAX),
        "small" => words.iter_mut().for_each(|w| *w = rng.next() & 0xff),
        _ => words.iter_mut().for_each(|w| *w = rng.next()),
    }
}

/// Stops the writer threads however the test ends.
struct Writers(
    std::sync::Arc<std::sync::atomic::AtomicBool>,
    Vec<std::thread::JoinHandle<()>>,
);

impl Drop for Writers {
    fn drop(&mut self) {
        self.0.store(true, std::sync::atomic::Ordering::Relaxed);
        for t in self.1.drain(..) {
            let _ = t.join();
        }
    }
}

#[test]
#[ignore = "runs a program on the PC's adapter until it stops it: the screen may flicker"]
fn a_translated_mesh_shader_over_garbage() {
    let Some(path) = std::env::var_os("MESH_DXIL") else {
        println!("MESH_DXIL names no file: nothing to run");
        return;
    };
    let ms = std::fs::read(&path).expect("the DXIL file");
    let (mode, iterations) = (
        std::env::var("MESH_MODE").unwrap_or_else(|_| "garbage".into()),
        env_u64("MESH_ITERS", 2000),
    );
    let (draws, groups, seed) = (
        env_u64("MESH_DRAWS", 40),
        env_u64("MESH_GROUPS", 1) as u32,
        env_u64("MESH_SEED", 1),
    );
    let mut rng = Rng(0x9e37_79b9_7f4a_7c15 ^ seed);
    // SAFETY: plain D3D12 objects, used as the API says; the buffers outlive every list that reads them
    // (each list is waited for).
    unsafe {
        if std::env::var_os("MESH_DEBUG").is_some() {
            let mut layer: Option<ID3D12Debug> = None;
            if D3D12GetDebugInterface(&mut layer).is_ok() {
                layer.iter().for_each(|l| l.EnableDebugLayer());
            }
        }
        let mut device: Option<ID3D12Device> = None;
        D3D12CreateDevice(None, D3D_FEATURE_LEVEL_12_0, &mut device).unwrap();
        let device = device.unwrap();
        let mut o = D3D12_FEATURE_DATA_D3D12_OPTIONS7::default();
        device
            .CheckFeatureSupport(
                D3D12_FEATURE_D3D12_OPTIONS7,
                (&raw mut o).cast(),
                size_of::<D3D12_FEATURE_DATA_D3D12_OPTIONS7>() as u32,
            )
            .unwrap();
        if o.MeshShaderTier == D3D12_MESH_SHADER_TIER_NOT_SUPPORTED {
            println!("no mesh shaders on this adapter");
            return;
        }
        let root = root_signature(
            &device,
            env_u64("MESH_SRV", 3) as u32,
            env_u64("MESH_IA", 0) != 0,
        );
        let mix = env_u64("MESH_MIX", 0);
        let (vertex_root, vertex_pso) = vertex_pipeline(&device);
        let pso = match mesh_pipeline(&device, &root, &ms) {
            Ok(pso) => pso,
            Err(e) => {
                messages(&device);
                panic!("the mesh pipeline: {e}");
            }
        };
        let step = device.GetDescriptorHandleIncrementSize(D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV)
            as usize;
        let views: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_CBV_SRV_UAV,
                NumDescriptors: VIEWS,
                Flags: D3D12_DESCRIPTOR_HEAP_FLAG_SHADER_VISIBLE,
                NodeMask: 0,
            })
            .unwrap();
        let (cpu, gpu) = (
            views.GetCPUDescriptorHandleForHeapStart().ptr,
            views.GetGPUDescriptorHandleForHeapStart().ptr,
        );
        // The buffers: what the first view shows (meshlets), the second (matrices), the constant buffer, and the
        // vertex data the table reads.
        const MESHLETS: usize = 4 << 20;
        const MATRICES: usize = 1 << 20;
        const VERTICES: usize = 16 << 20;
        let memory = std::env::var("MESH_MEMORY").unwrap_or_else(|_| "upload".into());
        let buffers = [
            upload_buffer(&device, MESHLETS as u64, &memory),
            upload_buffer(&device, MATRICES as u64, &memory),
            upload_buffer(&device, 65536, &memory),
            upload_buffer(&device, VERTICES as u64, &memory),
        ];
        let raw_view = |resource: &ID3D12Resource, bytes: usize, at: usize| {
            let desc = D3D12_SHADER_RESOURCE_VIEW_DESC {
                Format: DXGI_FORMAT_R32_TYPELESS,
                ViewDimension: D3D12_SRV_DIMENSION_BUFFER,
                Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
                Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                    Buffer: D3D12_BUFFER_SRV {
                        FirstElement: 0,
                        NumElements: (bytes / 4) as u32,
                        StructureByteStride: 0,
                        Flags: D3D12_BUFFER_SRV_FLAG_RAW,
                    },
                },
            };
            device.CreateShaderResourceView(
                resource,
                Some(&desc),
                D3D12_CPU_DESCRIPTOR_HANDLE {
                    ptr: cpu + at * step,
                },
            );
        };
        // What the draws' tables hold: `raw` is right (two raw views and a constant buffer); the others put what
        // the host might have left there: nothing written, null views of a texture or of a buffer, a texture, typed
        // or structured views of the buffers, unordered access views, kinds crossed.
        let views_kind = std::env::var("MESH_VIEWS").unwrap_or_else(|_| "raw".into());
        let default_heap = D3D12_HEAP_PROPERTIES {
            Type: D3D12_HEAP_TYPE_DEFAULT,
            ..Default::default()
        };
        let texture = {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
                Width: 256,
                Height: 256,
                DepthOrArraySize: 1,
                MipLevels: 1,
                Format: DXGI_FORMAT_R8G8B8A8_UNORM,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                ..Default::default()
            };
            let mut made: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &default_heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut made,
                )
                .unwrap();
            made.unwrap()
        };
        let uav_buffer = {
            let desc = D3D12_RESOURCE_DESC {
                Dimension: D3D12_RESOURCE_DIMENSION_BUFFER,
                Width: MESHLETS as u64,
                Height: 1,
                DepthOrArraySize: 1,
                MipLevels: 1,
                SampleDesc: DXGI_SAMPLE_DESC {
                    Count: 1,
                    Quality: 0,
                },
                Layout: D3D12_TEXTURE_LAYOUT_ROW_MAJOR,
                Flags: D3D12_RESOURCE_FLAG_ALLOW_UNORDERED_ACCESS,
                ..Default::default()
            };
            let mut made: Option<ID3D12Resource> = None;
            device
                .CreateCommittedResource(
                    &default_heap,
                    D3D12_HEAP_FLAG_NONE,
                    &desc,
                    D3D12_RESOURCE_STATE_COMMON,
                    None,
                    &mut made,
                )
                .unwrap();
            made.unwrap()
        };
        let at = |slot: usize| D3D12_CPU_DESCRIPTOR_HANDLE {
            ptr: cpu + slot * step,
        };
        let srv = |resource: Option<&ID3D12Resource>,
                   desc: D3D12_SHADER_RESOURCE_VIEW_DESC,
                   slot: usize| {
            device.CreateShaderResourceView(resource, Some(&desc), at(slot));
        };
        let texture_srv = D3D12_SHADER_RESOURCE_VIEW_DESC {
            Format: DXGI_FORMAT_R8G8B8A8_UNORM,
            ViewDimension: D3D12_SRV_DIMENSION_TEXTURE2D,
            Shader4ComponentMapping: D3D12_DEFAULT_SHADER_4_COMPONENT_MAPPING,
            Anonymous: D3D12_SHADER_RESOURCE_VIEW_DESC_0 {
                Texture2D: D3D12_TEX2D_SRV {
                    MipLevels: 1,
                    ..Default::default()
                },
            },
        };
        let buffer_srv =
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
        let uav = |resource: &ID3D12Resource, slot: usize| {
            let desc = D3D12_UNORDERED_ACCESS_VIEW_DESC {
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
            };
            device.CreateUnorderedAccessView(
                resource,
                None::<&ID3D12Resource>,
                Some(&desc),
                at(slot),
            );
        };
        let cbv = |buffer: &ID3D12Resource, slot: usize| {
            device.CreateConstantBufferView(
                Some(&D3D12_CONSTANT_BUFFER_VIEW_DESC {
                    BufferLocation: buffer.GetGPUVirtualAddress(),
                    SizeInBytes: 65536,
                }),
                at(slot),
            );
        };
        for &table in &TABLES {
            let (a, b, c) = (table as usize, table as usize + 2, table as usize + 3);
            match views_kind.as_str() {
                "uninit" => {}
                "null2d" => {
                    for slot in [a, b, c] {
                        srv(None, texture_srv, slot);
                    }
                }
                "nullbuf" => {
                    for slot in [a, b] {
                        srv(
                            None,
                            buffer_srv(
                                DXGI_FORMAT_R32_TYPELESS,
                                1024,
                                0,
                                D3D12_BUFFER_SRV_FLAG_RAW,
                            ),
                            slot,
                        );
                    }
                    device.CreateConstantBufferView(None, at(c));
                }
                "tex" => {
                    for slot in [a, b, c] {
                        srv(Some(&texture), texture_srv, slot);
                    }
                }
                "typed" => {
                    srv(
                        Some(&buffers[0].0),
                        buffer_srv(
                            DXGI_FORMAT_R32G32B32A32_FLOAT,
                            (MESHLETS / 16) as u32,
                            0,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        ),
                        a,
                    );
                    srv(
                        Some(&buffers[1].0),
                        buffer_srv(
                            DXGI_FORMAT_R32G32B32A32_FLOAT,
                            (MATRICES / 16) as u32,
                            0,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        ),
                        b,
                    );
                    cbv(&buffers[2].0, c);
                }
                "structured" => {
                    srv(
                        Some(&buffers[0].0),
                        buffer_srv(
                            DXGI_FORMAT_UNKNOWN,
                            (MESHLETS / 48) as u32,
                            48,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        ),
                        a,
                    );
                    srv(
                        Some(&buffers[1].0),
                        buffer_srv(
                            DXGI_FORMAT_UNKNOWN,
                            (MATRICES / 48) as u32,
                            48,
                            D3D12_BUFFER_SRV_FLAG_NONE,
                        ),
                        b,
                    );
                    cbv(&buffers[2].0, c);
                }
                "uav" => {
                    uav(&uav_buffer, a);
                    uav(&uav_buffer, b);
                    cbv(&buffers[2].0, c);
                }
                "swap" => {
                    cbv(&buffers[2].0, a);
                    cbv(&buffers[2].0, b);
                    raw_view(&buffers[0].0, MESHLETS, c);
                }
                _ => {
                    raw_view(&buffers[0].0, MESHLETS, a);
                    raw_view(&buffers[1].0, MATRICES, b);
                    cbv(&buffers[2].0, c);
                }
            }
        }
        // The one-view tables of the root views: the second of them (t3) the vertex data, the first (t255) the same.
        raw_view(&buffers[3].0, VERTICES, 0);
        raw_view(&buffers[3].0, VERTICES, 1);
        // A depth target, and its view.
        let depth_desc = D3D12_RESOURCE_DESC {
            Dimension: D3D12_RESOURCE_DIMENSION_TEXTURE2D,
            Width: u64::from(WIDTH),
            Height: HEIGHT,
            DepthOrArraySize: 1,
            MipLevels: 1,
            Format: DXGI_FORMAT_D32_FLOAT,
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            Flags: D3D12_RESOURCE_FLAG_ALLOW_DEPTH_STENCIL,
            ..Default::default()
        };
        let mut depth: Option<ID3D12Resource> = None;
        device
            .CreateCommittedResource(
                &D3D12_HEAP_PROPERTIES {
                    Type: D3D12_HEAP_TYPE_DEFAULT,
                    ..Default::default()
                },
                D3D12_HEAP_FLAG_NONE,
                &depth_desc,
                D3D12_RESOURCE_STATE_DEPTH_WRITE,
                None,
                &mut depth,
            )
            .unwrap();
        let depth = depth.unwrap();
        let dsvs: ID3D12DescriptorHeap = device
            .CreateDescriptorHeap(&D3D12_DESCRIPTOR_HEAP_DESC {
                Type: D3D12_DESCRIPTOR_HEAP_TYPE_DSV,
                NumDescriptors: 1,
                ..Default::default()
            })
            .unwrap();
        let dsv = dsvs.GetCPUDescriptorHandleForHeapStart();
        device.CreateDepthStencilView(&depth, None, dsv);
        // Queue, list, fence.
        let queue: ID3D12CommandQueue = device
            .CreateCommandQueue(&D3D12_COMMAND_QUEUE_DESC {
                Type: D3D12_COMMAND_LIST_TYPE_DIRECT,
                ..Default::default()
            })
            .unwrap();
        let allocator: ID3D12CommandAllocator = device
            .CreateCommandAllocator(D3D12_COMMAND_LIST_TYPE_DIRECT)
            .unwrap();
        let list: ID3D12GraphicsCommandList6 = device
            .CreateCommandList(0, D3D12_COMMAND_LIST_TYPE_DIRECT, &allocator, None)
            .unwrap();
        list.Close().unwrap();
        let fence: ID3D12Fence = device.CreateFence(0, D3D12_FENCE_FLAG_NONE).unwrap();
        let event: HANDLE = CreateEventW(None, false, false, None).unwrap();
        println!(
            "mesh shader {} bytes, mode {mode}, views {views_kind}, memory {memory}, mix {mix}, ia {}, {iterations} lists of {draws} dispatches of {groups} groups, seed {seed}",
            ms.len(),
            env_u64("MESH_IA", 0)
        );
        let began = std::time::Instant::now();
        let sizes = [MESHLETS, MATRICES, 65536, VERTICES];
        // The CPU writes the same memory the card reads while the lists run, as the host's refreshes do (threads
        // writing runs of random bytes; `MESH_WRITERS` of them).
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let writers: Vec<_> = (0..env_u64("MESH_WRITERS", 0))
            .map(|k| {
                let stop = stop.clone();
                let targets: Vec<(usize, usize)> = buffers
                    .iter()
                    .zip(sizes)
                    .map(|(b, n)| (b.1 as usize, n))
                    .collect();
                std::thread::spawn(move || {
                    let mut rng = Rng(0x1234_5678_9abc_def1 ^ (k + 1));
                    let mut chunk = vec![0u8; 1 << 14];
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        let (base, bytes) = targets[rng.next() as usize % targets.len()];
                        let at = (rng.next() as usize % (bytes - chunk.len())) & !63;
                        for b in chunk.iter_mut().step_by(4) {
                            *b = rng.next() as u8;
                        }
                        // SAFETY: inside the buffer's mapping; the card may read it meanwhile, which is the point.
                        std::ptr::copy_nonoverlapping(
                            chunk.as_ptr(),
                            (base + at) as *mut u8,
                            chunk.len(),
                        );
                    }
                })
            })
            .collect();
        // `MESH_BURN=n`: n threads that only compute, so that the CPU is as busy as the host makes it.
        let burners: Vec<_> = (0..env_u64("MESH_BURN", 0))
            .map(|k| {
                let stop = stop.clone();
                std::thread::spawn(move || {
                    let mut x = 0x2545_f491_4f6c_dd1du64 ^ k;
                    while !stop.load(std::sync::atomic::Ordering::Relaxed) {
                        for _ in 0..100_000 {
                            x = x
                                .wrapping_mul(6364136223846793005)
                                .wrapping_add(1442695040888963407);
                        }
                        std::hint::black_box(x);
                    }
                })
            })
            .collect();
        let _writers = Writers(stop, writers.into_iter().chain(burners).collect());
        let pause = env_u64("MESH_SLEEP_MS", 0);
        for (buffer, bytes) in buffers.iter().zip(sizes) {
            fill(&mode, &mut rng, buffer.1, bytes);
        }
        for n in 1..=iterations {
            // Between lists a few thousand dwords of each buffer change (all of them is slow in a debug build).
            if mode != "zero" && mode != "ones" {
                for (buffer, bytes) in buffers.iter().zip(sizes) {
                    // SAFETY: the buffer's whole mapping; the last list has run.
                    let words = std::slice::from_raw_parts_mut(buffer.1.cast::<u32>(), bytes / 4);
                    for k in 0..4096 {
                        let at = if k < 1024 {
                            k
                        } else {
                            rng.next() as usize % words.len()
                        };
                        words[at] = if mode == "small" {
                            rng.next() & 0xff
                        } else {
                            rng.next()
                        };
                    }
                }
            }
            allocator.Reset().unwrap();
            list.Reset(&allocator, &pso).unwrap();
            list.SetDescriptorHeaps(&[Some(views.clone())]);
            list.SetGraphicsRootSignature(&root);
            list.SetGraphicsRootDescriptorTable(1, D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu });
            list.SetGraphicsRootDescriptorTable(
                2,
                D3D12_GPU_DESCRIPTOR_HANDLE {
                    ptr: gpu + step as u64,
                },
            );
            list.OMSetRenderTargets(0, None, false, Some(&dsv));
            list.RSSetViewports(&[D3D12_VIEWPORT {
                TopLeftX: 0.0,
                TopLeftY: 0.0,
                Width: WIDTH as f32,
                Height: HEIGHT as f32,
                MinDepth: 0.0,
                MaxDepth: 1.0,
            }]);
            list.RSSetScissorRects(&[windows::Win32::Foundation::RECT {
                left: 0,
                top: 0,
                right: WIDTH as i32,
                bottom: HEIGHT as i32,
            }]);
            for d in 0..draws {
                // With MESH_MIX=n a vertex draw follows each n mesh dispatches: the lists switch between the two
                // kinds of pipeline and their root signatures, as the title's do.
                if mix > 0 && d % mix == mix - 1 {
                    list.SetGraphicsRootSignature(&vertex_root);
                    list.SetPipelineState(&vertex_pso);
                    list.IASetPrimitiveTopology(D3D_PRIMITIVE_TOPOLOGY_TRIANGLELIST);
                    list.DrawInstanced(3, 1, 0, 0);
                    list.SetGraphicsRootSignature(&root);
                    list.SetPipelineState(&pso);
                    list.SetDescriptorHeaps(&[Some(views.clone())]);
                    list.SetGraphicsRootDescriptorTable(
                        1,
                        D3D12_GPU_DESCRIPTOR_HANDLE { ptr: gpu },
                    );
                    list.SetGraphicsRootDescriptorTable(
                        2,
                        D3D12_GPU_DESCRIPTOR_HANDLE {
                            ptr: gpu + step as u64,
                        },
                    );
                }
                let mut user = [0u32; 16];
                user[9] = TABLES[(d as usize) % TABLES.len()] * 32;
                list.SetGraphicsRoot32BitConstants(0, 16, user.as_ptr().cast(), 0);
                list.DispatchMesh(groups, 1, 1);
            }
            list.Close().unwrap();
            queue.ExecuteCommandLists(&[Some(list.cast().unwrap())]);
            queue.Signal(&fence, n).unwrap();
            fence.SetEventOnCompletion(n, event).unwrap();
            if WaitForSingleObject(event, 8000).0 != 0 {
                println!(
                    "THE CARD DID NOT ANSWER in 8 s: list {n} of {iterations} after {:.1} s; removed: {:?}",
                    began.elapsed().as_secs_f64(),
                    device.GetDeviceRemovedReason()
                );
                return;
            }
            if let Err(e) = device.GetDeviceRemovedReason() {
                println!(
                    "THE DEVICE WAS REMOVED at list {n} of {iterations} after {:.1} s: {e}",
                    began.elapsed().as_secs_f64()
                );
                return;
            }
            if pause > 0 {
                std::thread::sleep(std::time::Duration::from_millis(pause));
            }
            if n % 500 == 0 {
                println!("  {n} lists, {:.1} s", began.elapsed().as_secs_f64());
            }
        }
        println!(
            "no hang in {iterations} lists ({:.1} s)",
            began.elapsed().as_secs_f64()
        );
    }
}

#[test]
#[ignore = "runs programs on the PC's adapter until it stops them: the screen may flicker"]
fn the_titles_load_around_a_mesh_shader() {
    load::run();
}
