//! The programs of our own that the load test (`load.rs`) runs next to the translated mesh shader,
//! compiled by the Windows SDK's `dxc.exe` (the newest kit's, as `replay.rs` finds it).

/// Compiles `source` for `target` (`ms_6_6`, `ps_6_6`, `cs_6_6`) with the SDK's dxc; `defines` are `-D` values.
/// Panics with the compiler's text when there is no dxc or it refuses the source.
pub fn dxc(source: &str, entry: &str, target: &str, defines: &[(&str, &str)]) -> Vec<u8> {
    let kits = std::fs::read_dir("C:/Program Files (x86)/Windows Kits/10/bin")
        .expect("the Windows SDK (dxc.exe)");
    let exe = kits
        .flatten()
        .map(|e| e.path().join("x64/dxc.exe"))
        .filter(|p| p.exists())
        .max()
        .expect("a dxc.exe in the Windows SDK");
    let dir = tempfile::tempdir().expect("a temporary folder");
    let (from, to) = (dir.path().join("a.hlsl"), dir.path().join("a.dxil"));
    std::fs::write(&from, source).expect("the source file");
    let mut command = std::process::Command::new(exe);
    command.args(["-T", target, "-E", entry, "-Fo"]).arg(&to);
    for (name, value) in defines {
        command.arg("-D").arg(format!("{name}={value}"));
    }
    let out = command.arg(&from).output().expect("dxc.exe ran");
    assert!(
        out.status.success(),
        "dxc {target}: {}{}",
        String::from_utf8_lossy(&out.stdout),
        String::from_utf8_lossy(&out.stderr)
    );
    std::fs::read(to).expect("the compiled shader")
}

/// A mesh shader that draws real, valid triangles: a patch of 128 of them (a 16 x 8 grid of vertices, each
/// triangle joining a vertex with its neighbour and the one a row below), placed by the group number and the
/// first root constants, over depth values that spread over the range, so a draw covers pixels, is depth
/// tested and written. It declares the counts the translator's programs do (128 vertices, 128 primitives),
/// reads a little of the vertex buffer (t2 in space 1: the bindings of the translated programs) and writes a
/// position, a texture coordinate and a normal.
///
/// `VARIANT` (a `#define` before the source) makes a program of its own of each number.
///
/// `u[0].x` shifts the placement, `u[0].y` (low byte) sets the size of a patch, `u[0].z` where in the vertex
/// buffer it reads (up to 64 MB in: past the end of the buffer, where a view claims more than it holds).
pub const OWN_MESH: &str = r#"
#ifndef VARIANT
#define VARIANT 0
#endif
cbuffer UserData : register(b0, space0) { uint4 u[4]; };
ByteAddressBuffer vtx : register(t2, space1);
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; float3 n : NORMAL; };

[numthreads(128, 1, 1)]
[outputtopology("triangle")]
void ms(uint gi : SV_GroupIndex, uint3 gid : SV_GroupID, out vertices V verts[128], out indices uint3 tris[128])
{
    SetMeshOutputCounts(128, 128);
    uint g = gid.x * 7919u + u[0].x;
    float cx = frac(g * 0.6180339f) * 1.8f - 0.9f;
    float cy = frac(g * 0.7548776f) * 1.8f - 0.9f;
    float size = (0.02f + 0.002f * (u[0].y & 255u)) * (1.0f + 0.01f * VARIANT);
    float z = 0.05f + 0.9f * frac(g * 0.5698403f);
    // Each variant is another program: a few more steps of arithmetic.
    [unroll]
    for (uint i = 0; i < (VARIANT % 7); i++) {
        z = frac(z * 1.37f + 0.11f * (i + 1));
        size = size * 0.999f + z * 0.0001f;
    }
    z = 0.05f + 0.9f * z;
    uint ix = gi & 15u;
    uint iy = gi >> 4u;
    uint bits = vtx.Load(((gi * 16u + u[0].z * 16u) & 0x3fffffcu));
    float jitter = (bits & 255u) * (0.0005f / 255.0f);
    V v;
    v.at = float4(cx + ix * (size / 16.0f) + jitter, cy + iy * (size / 8.0f) + jitter, z, 1.0f);
    v.uv = float2(ix / 15.0f, iy / 7.0f);
    v.n = normalize(float3(ix - 7.5f, iy - 3.5f, 8.0f));
    verts[gi] = v;
    tris[gi] = uint3(gi, (gi + 1u) & 127u, (gi + 16u) & 127u);
}
"#;

/// The pixel shader for [`OWN_MESH`] (and a stand-in for the title's own): `targets` colour outputs.
pub fn own_pixel(targets: u32) -> String {
    let mut outputs = String::new();
    let mut body = String::new();
    for k in 0..targets.max(1) {
        outputs.push_str(&format!("    float4 c{k} : SV_Target{k};\n"));
        body.push_str(&format!(
            "    o.c{k} = float4(v.uv.x + {k}.0f, v.uv.y, v.n.z, 1.0f);\n"
        ));
    }
    format!(
        r#"
struct V {{ float4 at : SV_Position; float2 uv : TEXCOORD0; float3 n : NORMAL; }};
struct O {{
{outputs}}};
O ps(V v)
{{
    O o;
{body}    return o;
}}
"#
    )
}

/// A compute pass that works like the host's argument clamp: it reads records of mesh dispatch arguments
/// (three dwords each), cuts each to what a dispatch may run, adds the work of the group's records to one
/// word with a compare and swap loop, and writes the clamped records for `ExecuteIndirect`.
pub const CLAMP: &str = r#"
#define RS "RootConstants(num32BitConstants=4, b0), SRV(t0), UAV(u0), UAV(u1)"
cbuffer C : register(b0) { uint records; uint most; uint seed; uint pad; };
ByteAddressBuffer src : register(t0);
RWByteAddressBuffer dst : register(u0);
RWByteAddressBuffer budget : register(u1);
groupshared uint costs[64];
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID, uint gi : SV_GroupIndex)
{
    uint cost = 0;
    uint3 g = uint3(0, 0, 0);
    if (id.x < records) {
        g = src.Load3(id.x * 12u);
        if (g.x > 65535u) g = uint3(0, 0, 0);
        g = uint3(min(g.x, most), min(g.y, 1u), min(g.z, 1u));
        cost = g.x * 128u;
    }
    costs[gi] = cost;
    GroupMemoryBarrierWithGroupSync();
    if (gi == 0) {
        uint first = id.x - gi;
        uint n = records > first ? min(64u, records - first) : 0u;
        if (n > 0) {
            uint before;
            budget.InterlockedOr(0, 0, before);
            [allow_uav_condition]
            while (before < (1u << 27)) {
                uint next = before;
                for (uint j = 0; j < n; j++) next += costs[j];
                uint prev;
                budget.InterlockedCompareExchange(0, before, next, prev);
                if (prev == before) break;
                before = prev;
            }
            if (before >= (1u << 27)) {
                uint old;
                budget.InterlockedExchange(0, 0, old);
            }
        }
    }
    if (id.x < records) dst.Store3(id.x * 12u, g);
}
"#;

/// A compute pass of the kind the title's culling runs next to the draws: reads and writes a big buffer and
/// adds into a small one, with atomics.
pub const CHURN: &str = r#"
#define RS "UAV(u0), UAV(u1)"
RWByteAddressBuffer a : register(u0);
RWByteAddressBuffer b : register(u1);
[RootSignature(RS)]
[numthreads(64, 1, 1)]
void main(uint3 id : SV_DispatchThreadID)
{
    uint at = (id.x * 4u) & 0x3fffffcu;
    uint x = a.Load(at);
    uint prev;
    b.InterlockedAdd((id.x & 1023u) * 4u, x | 1u, prev);
    a.Store(at, x * 1664525u + 1013904223u + prev);
}
"#;

/// A vertex shader for the vertex draws between the mesh ones (`MESH_VERTEX`): a triangle by vertex id with the
/// outputs of [`OWN_MESH`], so that [`own_pixel`] is its pixel shader.
pub const VERTEX: &str = r#"
struct V { float4 at : SV_Position; float2 uv : TEXCOORD0; float3 n : NORMAL; };
V vs(uint i : SV_VertexID)
{
    float2 p = float2((i << 1) & 2, i & 2);
    V v;
    v.at = float4(p - 0.5f, 0.5f + 0.0001f * i, 1.0f);
    v.uv = p;
    v.n = float3(0.0f, 0.0f, 1.0f);
    return v;
}
"#;
