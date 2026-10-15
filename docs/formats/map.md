# Maps (`world.vwrld_c`, `*.vwnod_c`, `*.vents_c`)

OFAST's code: `crates/ofast-core/src/map/` (reader), `src/content.rs` (search path), `src/export/map.rs` (export).

## Where a map lives

`game/csgo/maps/<map>.vpk` is a **single-file VPK** (all entries embedded). Mount it in front of `pak01_dir.vpk`:
materials, textures and prop models referenced by the map come from pak01. Stored references omit `_c` and may
use backslashes.

de_mirage (2026-10-15): 699 files — 680 `vmdl_c` (595 baked world/prop models in `maps/de_mirage/worldnodes/`, 84
brush-entity models, `world_physics`), 7 lightmap/cubemap textures, `world.vwrld_c`, one world node, one entity lump,
visibility, nav and occlusion data. The 3D skybox is a separate map: `maps/prefabs/de_mirage/3dskybox_mirage_legacy.vpk`.

## World (`world.vwrld_c` DATA)

| Key | Meaning |
|---|---|
| `m_worldNodes[]` | `m_worldNodePrefix` (e.g. `maps\de_mirage\worldnodes\n0` → `.vwnod_c`), bounds, origin |
| `m_entityLumps[]` | entity lump paths (`.vents`) |
| `m_worldLightingInfo` | lightmap version, `m_vLightmapUvScale`, `m_lightMaps[]` |
| `m_builderParams` | compile settings |

## World node (`*.vwnod_c` DATA)

**`m_sceneObjects[]`** — whole models: `m_renderableModel`, `m_vTransform` (three 4-vectors = row-major 3×4,
translation in the last column), `m_vTintColor`, `m_nObjectTypeFlags` (an **int for combinations, an enum name
string for single flags**), `m_nOverlayRenderOrder`. In de_mirage every transform is identity: the baked geometry is
already in world space.

| Flag | Value | OFAST |
|---|---|---|
| `OBJECT_TYPE_BLOCK_LIGHT` | 16 | skipped (shadow-only tool geometry) |
| `OBJECT_TYPE_OVERLAY` | 8192 | exported (decals) |

**`m_aggregateSceneObjects[]`** — per-draw-call placement of one model: `m_renderableModel`, `m_aggregateMeshes[]`
(`m_nDrawCallIndex`, `m_bHasTransform`, `m_vTintColor` RGB 0–255, `m_nLODGroupMask`), `m_fragmentTransforms[]`
(flat 12 floats each, consumed in order by the fragments that have `m_bHasTransform`). Several fragments can draw
the same draw call with different transforms (instanced static props).

de_mirage: 335 scene objects (301 overlays), 260 aggregates, 3,023 fragments (10 with their own transform).

## Entity lump (`*.vents_c` DATA)

`m_entityKeyValues[]`: each entity's properties are in `keyValues3Data.values` (KV3) in current maps; an older
binary blob form (`m_keyValuesData`, MurmurHash2-hashed keys, seed `0x31415926`) is recognized but not read yet.
`m_childLumps[]` lists further lumps.

Values are loosely typed: `origin` may be a float array or `"5208.0 -968.0 44.5"`; numbers are often strings.

| Entity | Used for |
|---|---|
| `light_environment` | sun direction (`angles`), `color`, `brightness`, sky colour → manifest |
| `light_omni2`, `light_rect`, `light_barn` (`light_spot`, `light_omni`) | glTF point/spot lights |
| `prop_dynamic(_override)`, `prop_physics_multiplayer`, `func_brush`, `func_breakable` | placed models |
| `info_player_counterterrorist` / `_terrorist` | spawn viewpoints → manifest |
| `env_sky` (`skyname`), `skybox_reference` | sky (M2) |

**Static props are not entities in CS2**: they are baked into scene objects and aggregates.

Entity transform = `Rz(yaw) · Ry(pitch) · Rx(roll)` scaled by `scales`, translated by `origin` — the same matrix as
the Source SDK's `AngleMatrix` (positive pitch looks down).

de_mirage: 561 entities, among them 1 sun, 20 omni, 45 rect and 7 barn lights, 92 dynamic props, 43 brushes,
16 CT and 17 T spawns.

## Lightmaps (not used yet)

`irradiance` (BC6H 8192²), `directional_irradiance` (DXT1), `direct_light_shadows` (BC4); lightmap UVs are vertex
attribute `TEXCOORD` 3 (`R16G16_UNORM`) scaled by `m_vLightmapUvScale`. OFAST relights maps with Lumen instead.

## Sources

- ValveResourceFormat (MIT): `Resource/ResourceTypes/{World,WorldNode,EntityLump}.cs`,
  `IO/Gltf/GltfModelExporter.World.cs`, `Renderer/Renderer/World/*`, `Renderer/Renderer/SceneAggregate.cs`
- SteamDatabase GameTracking-CS2 schema dumps: `DumpSource2/schemas/worldrenderer`
- Real data: `crates/ofast-core/tests/cs2_map.rs` (de_mirage counts above)
