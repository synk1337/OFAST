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
| BLENDINDICES | 30 `R8G8B8A8_UINT` (12 `R16G16B16A16_UINT` read too) | four bone indices into the mesh's remapping slice (see "Skeleton and skinning") |
| BLENDWEIGHT (stored as `blendweight`) | 28 `R8G8B8A8_UNORM` | four weights, bytes summing to exactly 255 |

**`R32_UINT` tangent frame**: bit 0 = bitangent sign (0 → −1), bits 1–11 = tangent angle (`/2047 × 2π`), bits 12–21
= normal X, bits 22–31 = normal Y (octahedral, `/1023 × 2 − 1`, folded when `z < 0`). The tangent is a reference
vector perpendicular to the normal, rotated around the normal by the stored angle.

## Skeleton and skinning

Player agents are `agents/models/<agent>/<agent>.vmdl_c` (0.5 – 1.4 MB; `characters/models/*` are 4.8 KB stubs with
an orange box and one `dummy` bone). Everything is embedded, with no LODs. Meshes: `thirdperson_body`,
`thirdperson_default_gloves`, `firstperson_default_gloves_arms`, `firstperson_sleeves`, and on 35 CT agents a
`defusekit`. The default mesh-group mask picks body + default gloves, the full third-person agent. Glove skins are
separate models (`agents/models/shared/arms/glove_*`, two root bones) meant to be bone-merged onto the agent.

**`DATA → m_modelSkeleton`**, parallel arrays (`src/model/skeleton.rs`): `m_boneName`, `m_nParent` (−1 root),
`m_bonePosParent` (inches, parent frame), `m_boneRotParent` (quaternion x, y, z, w), `m_boneScaleParent` (~1,
ignored), `m_nFlag` (0x10 physics, 0x20 attachment, 0x40 animation, 0x80 mesh, 0x100 hitbox, 0x400… LODs, 0x400000
procedural twist bones). Parents come before children in all 93 agent and glove models. Bind pose in model space:
`W = W_parent · T(position) · R(rotation)`. Agents have 86 – 94 bones from `root_motion`, whose rotation (0.5, 0.5,
0.5, 0.5) turns the A-pose to face +X: pelvis at (−2.85, 0, 42.82), head at 70.8 inches.

**Remapping**: `DATA m_remappingTable` holds one slice per mesh, starting at `m_remappingTableStarts[mesh index]` and
ending at the next start. A vertex's bone index `i` is skeleton bone `table[start + i]`. A slice begins with the
mesh's own render bones (`MDAT m_skeleton.m_bones`, same order, each with an `m_invBindPose`) and on agents ends with
19 hitbox bones that vertices never use. `MDAT m_skeleton.m_nBoneWeightCount` is 1 – 4; with 1 the mesh stores no
weights (six `firstperson_sleeves` buffers): the first index takes the whole vertex. Slots with weight 0 can hold
leftover indices.

**glTF** (`GltfWriter::skin`, `export_model`): bones become nodes in bone order (glTF joint `i` = bone `i`); a root
bone carries the Source → glTF rotation (−90° about X) and a converted translation, the others their CS2 rotation and
their translation in metres. The inverse bind matrices are the inverses of the same chain, so they match the nodes
exactly; Unreal takes its reference pose from them (`GLTFReader.cpp`). Joints are remapped to skeleton bones, unused
slots set to 0, weights written as normalized bytes. Mesh nodes that share a bone's name get `_mesh` (Unreal renames
clashing node names). Checked on `ctm_sas` in Unreal (2026-10-18): one Skeleton with 94 bones, pelvis 108.8 cm and
head 179.8 cm above the feet, hands ±53.2 cm to the sides, every vertex weight sum 255; screenshot in
`test-data/shots/agents/`. `tm_phoenix` (86 bones) and the AK-47 world model (7 bones) export skinned too.

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
  `IO/Gltf/GltfModelExporter*.cs`, `Compression/MeshOptimizer*.cs`; skinning: `ModelAnimation/Skeleton.cs`,
  `Bone.cs`, `ModelData/BoneRemapTable.cs`, `GltfModelExporter.Skeleton.cs`, `GltfModelExporter.Mesh.cs`
- Unreal 5.8 Interchange source for the glTF skin import: `Parsers/GLTFCore/Private/GLTFReader.cpp`,
  `Import/Private/Gltf/InterchangeGltfMesh.cpp` (4 influences read)
- meshoptimizer (MIT): `src/vertexcodec.cpp`, `src/indexcodec.cpp`, `demo/tests.cpp` — https://github.com/zeux/meshoptimizer
- Real data: `cargo run --release --example resource_dump -- weapons/models/ak47/weapon_rif_ak47.vmdl_c`
