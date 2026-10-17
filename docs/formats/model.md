# Models (`.vmdl_c`) and meshes

A compiled CS2 model holds its meshes, their vertex and index buffers, and the information needed to pick the
default look. OFAST's code: `crates/ofast-core/src/model/` (reader) and `src/meshopt.rs` (buffer decompression).

## Blocks (current CS2 layout)

```text
.vmdl_c
 ├─ DATA  (KV3)  model: mesh groups ("bodygroups"), LOD masks, skeleton, material groups, physics refs
 ├─ CTRL  (KV3)  embedded_meshes[]: for each mesh, which blocks hold its data and buffers
 ├─ MDAT  (KV3)  one per mesh: draw calls (material, buffers, index range)
 ├─ MVTX         one per vertex buffer (raw or meshopt-compressed)
 ├─ MIDX         one per index buffer (raw or meshopt-compressed)
 ├─ ANIM / ASEQ / AGRP / PHYS   animations, sequences, physics (KV3)
 ├─ RERL, RED2   references, editor info
```

Block numbers in `CTRL` (`m_nDataBlock`, `m_nBlockIndex`) are **0-based indices into the resource's block table**.
Older files use one binary `VBIB`/`MBUF` block per mesh instead of `MVTX`/`MIDX` (also supported).

A model without a `CTRL` block has no embedded meshes (external `.vmesh_c` files listed in `m_refMeshes`, or none at
all — e.g. physics-only gibs).

## Choosing what to show

| DATA key | Meaning |
|---|---|
| `m_meshGroups` | group names, e.g. `body_@0_#&body_hd`, `body_@1_#&body_legacy` |
| `m_refMeshGroupMasks[mesh]` | bit `g` set → mesh belongs to group `g` |
| `m_nDefaultMeshGroupMask` | groups shown by default |
| `m_refLODGroupMasks[mesh]` | bit `L` set → mesh is used at LOD `L` |

OFAST's default selection: meshes in the default groups and in the most detailed LOD. For the AK-47 that is
`body_hd` (the legacy CS:GO body is mesh group 1).

## `CTRL → embedded_meshes[]` (current layout)

```text
m_Name, m_nMeshIndex, m_nDataBlock (→ MDAT), m_nMorphBlock
m_vertexBuffers[] / m_indexBuffers[]:
  m_nBlockIndex (→ MVTX / MIDX), m_nElementCount, m_nElementSizeInBytes,
  m_bMeshoptCompressed, m_bCompressedZSTD, m_inputLayoutFields[]
m_inputLayoutFields[]: m_pSemanticName, m_nSemanticIndex, m_Format (DXGI), m_nOffset, m_nSlot, m_szShaderSemantic
```

Decompression order: ZSTD first (if flagged), then meshopt (if flagged and the size is not already right).

## `MDAT` (mesh) → draw calls

`m_sceneObjects[].m_drawCalls[]`:
`m_material` (resource path), `m_nPrimitiveType` (`RENDER_PRIM_TRIANGLES`), `m_nBaseVertex`, `m_nStartIndex`,
`m_nIndexCount`, `m_vertexBuffers[0].m_hBuffer` / `m_indexBuffer.m_hBuffer` (indices into the mesh's buffer lists),
`m_bUseCompressedNormalTangent` or `m_nFlags & 2` (packed normals), `m_vTintColor` (three floats, **linear**: the
colour Hammer painted on that mesh face or overlay, white for most models) and `m_flAlpha` (its opacity, 1 for most).
The tint is how map meshes get a colour their material does not have: de_nuke's yellow road paint is a white
material drawn with (0.851, 0.459, 0.046) = sRGB (237, 179, 63) through gamma 2.2. See
[material.md](material.md#draw-call-tint) for how it reaches the shader; the scene object above the draw call has
an `m_vTintColor` too (four floats, 0 0 0 0 on map meshes), unused by OFAST.

Triangle `k` of a draw call uses `IB[m_nStartIndex + k] + m_nBaseVertex`.

## Vertex attributes seen in CS2

| Semantic | DXGI format | Decoding |
|---|---|---|
| POSITION | 6 `R32G32B32_FLOAT` | inches, Source axes (X forward, Y left, Z up) |
| TEXCOORD | 34 `R16G16_FLOAT` (also 16, 35, 37) | half floats; origin top-left, no V flip needed for glTF |
| NORMAL | 42 `R32_UINT` "CompressedTangentFrame" | see below |
| NORMAL | 28 `R8G8B8A8_UNORM` (older) | two folded-octahedral byte pairs: normal, tangent |
| BLENDINDICES | 30 `R8G8B8A8_UINT` | bone indices (through the mesh's remap table) |

**`R32_UINT` tangent frame**: bit 0 = bitangent sign (0 → −1), bits 1–11 = tangent angle (`/2047 × 2π`), bits 12–21
= normal X, bits 22–31 = normal Y (octahedral, `/1023 × 2 − 1`, folded when `z < 0`). The tangent is a reference
vector perpendicular to the normal, rotated around the normal by the stored angle.

## Meshopt buffers

`MVTX` starts with `0xA1` (meshopt vertex codec v1) and `MIDX` with `0xE1` (index codec v1) in current CS2 files.
OFAST's own decoder (`src/meshopt.rs`) implements versions 0 and 1 of both, written from the meshoptimizer source
and the glTF `EXT_meshopt_compression` spec, and passes meshoptimizer's own test vectors.

## Verified

`crates/ofast-core/tests/cs2_model.rs` with `OFAST_FULL_SCAN=1` (2026-10-15): **all 13,105 models in CS2 decode
— 17,758 draw calls, 25.9 million triangles, every index in range — in under 3 seconds.** The AK-47's HD body is
21,410 vertices / 26,133 triangles and measures 37.3 × 2.8 × 10.7 inches.

## Sources

- ValveResourceFormat (MIT): `Resource/ResourceTypes/Model.cs`, `Mesh.cs`, `Resource/Blocks/VBIB.cs`,
  `IO/Gltf/GltfModelExporter*.cs`, `Compression/MeshOptimizer*.cs`
- meshoptimizer (MIT): `src/vertexcodec.cpp`, `src/indexcodec.cpp`, `demo/tests.cpp` — https://github.com/zeux/meshoptimizer
- Real data: `cargo run --release --example resource_dump -- weapons/models/ak47/weapon_rif_ak47.vmdl_c`
