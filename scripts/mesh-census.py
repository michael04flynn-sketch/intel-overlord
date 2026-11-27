"""Census of the title's mesh shaders as the translator emits them (docs/intel-arc.md, "A compute twin").

Reads DXIL containers dumped by `victory.exe translate --list` with VICTORY_EMIT_DUMP (outside the repo),
prints the maxima, the output scalars and the bytes one thread group writes, and, with the list of
(mesh, pixel) pairs a run's log names ("host mesh pipeline shaders=[mesh <hash>, pixel <hash>]"), whether every
pixel input is a mesh output of the same semantic, index, register and mask.

  victory.exe translate --list --jobs 3 <game dir>\\shaders.rpf     (VICTORY_TRANSLATE_ONLY=<hashes>,
                                                                    VICTORY_EMIT_DUMP=<dir>)
  grep -h -o "host mesh pipeline shaders=\\[[^]]*\\]" logs/*.log | sort -u > pairs.txt
  python -I scripts/mesh-census.py <dir> pairs.txt

Bytes per group are what the compute twin would write: 4 bytes per vertex scalar for each of the 128 vertices
(lane n writes vertex n), plus three 32-bit indices for each of the 128 primitives and any per-primitive scalars.
"""
import collections
import os
import re
import statistics
import struct
import sys


def container_parts(b):
    assert b[:4] == b"DXBC", b[:4]
    n = struct.unpack_from("<I", b, 28)[0]
    out = {}
    for o in struct.unpack_from("<%dI" % n, b, 32):
        size = struct.unpack_from("<I", b, o + 4)[0]
        out[b[o:o + 4].decode("latin1")] = b[o + 8:o + 8 + size]
    return out


def signature(data):
    """Elements of an ISG1/OSG1/PSG1 part (the rows `container::signature` writes)."""
    if not data:
        return []
    n, off = struct.unpack_from("<II", data, 0)
    out = []
    for i in range(n):
        base = off + 32 * i
        name_off, index, sv, comp, row = struct.unpack_from("<IIIII", data, base + 4)
        mask = data[base + 24]
        name = data[name_off:data.index(0, name_off)].decode("latin1")
        out.append(dict(name=name, index=index, sv=sv, comp=comp, row=row, mask=mask, cols=bin(mask).count("1")))
    return out


def mesh_info(path):
    parts = container_parts(open(path, "rb").read())
    info = parts["PSV0"][4:52]  # PSV0 runtime info, version 2 (`container::psv`)
    max_v, max_p = struct.unpack_from("<HH", info, 12)
    threads = 1
    for n in struct.unpack_from("<III", info, 36):
        threads *= n
    vertex, prim = signature(parts.get("OSG1")), signature(parts.get("PSG1"))
    v_scalars, p_scalars = sum(e["cols"] for e in vertex), sum(e["cols"] for e in prim)
    written_v = min(max_v, threads)
    return dict(max_v=max_v, max_p=max_p, threads=threads, vertex=vertex, v_scalars=v_scalars, p_scalars=p_scalars,
                bytes=written_v * 4 * v_scalars + max_p * (12 + 4 * p_scalars),
                d3d12=4 * (v_scalars * -(-max_v // 32) * 32 + p_scalars * -(-max_p // 32) * 32))


def stat(name, xs):
    xs = sorted(xs)
    n = len(xs)
    print(f"{name}: min {xs[0]} median {xs[n // 2]} p90 {xs[int(n * .9)]} max {xs[-1]} mean {statistics.mean(xs):.0f}")


def main(dump, pairs_file):
    by_hash = {}
    for f in os.listdir(dump):
        for h in re.findall(r"([0-9a-f]{16})", f):
            by_hash.setdefault(h, os.path.join(dump, f))
    pairs = []
    for line in open(pairs_file):
        m, p = re.search(r"mesh ([0-9a-f]{16})", line), re.search(r"pixel ([0-9a-f]{16})", line)
        if m:
            pairs.append((m.group(1), p.group(1) if p else None))
    meshes = {m: mesh_info(by_hash[m]) for m, _ in pairs if m in by_hash}
    pixels = {}
    for _, p in pairs:
        if p and p in by_hash and p not in pixels:
            pixels[p] = signature(container_parts(open(by_hash[p], "rb").read()).get("ISG1"))
    print(f"pipelines {len(pairs)}, mesh programs {len(meshes)}, pixel programs {len(pixels)}")
    print("(threads, max vertices, max primitives):", dict(collections.Counter(
        (r["threads"], r["max_v"], r["max_p"]) for r in meshes.values())))
    stat("vertex scalars", [r["v_scalars"] for r in meshes.values()])
    stat("primitive scalars", [r["p_scalars"] for r in meshes.values()])
    stat("bytes a thread group writes", [r["bytes"] for r in meshes.values()])
    print("position only (4 scalars):", sum(r["v_scalars"] <= 4 for r in meshes.values()),
          "| clip distance:", sum(any(e["sv"] == 2 for e in r["vertex"]) for r in meshes.values()),
          "| past D3D12's 32 KiB:", sum(r["d3d12"] > 32768 for r in meshes.values()))
    # Pixel inputs the rasteriser supplies itself: position, primitive id, front face, sample index, barycentrics...
    supplied = {1, 7, 9, 12, 23, 25, 8, 6}
    checked = bad = alone = 0
    why = collections.Counter()
    for m, p in pairs:
        if m not in meshes:
            continue
        if not p:
            alone += 1
            continue
        if p not in pixels:
            continue
        checked += 1
        ok = True
        for e in pixels[p]:
            if e["sv"] in supplied:
                continue
            out = [o for o in meshes[m]["vertex"] if o["name"] == e["name"] and o["index"] == e["index"]]
            if not out:
                ok = False
                why["no mesh output " + e["name"] + str(e["index"])] += 1
            elif out[0]["row"] != e["row"]:
                ok = False
                why["register differs"] += 1
            elif e["mask"] & ~out[0]["mask"]:
                ok = False
                why["mask not covered"] += 1
        bad += not ok
    print(f"pairs with a pixel shader: {checked} checked, {bad} not covered; mesh-only pipelines: {alone}", dict(why))
    sv = collections.Counter(e["sv"] for els in pixels.values() for e in els if e["sv"])
    print("pixel system-value inputs (D3D name code: programs):", dict(sv))


if __name__ == "__main__":
    main(sys.argv[1], sys.argv[2])
