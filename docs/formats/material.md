# Materials (`.vmat_c`) and how OFAST turns them into Unreal materials

OFAST's code: `crates/ofast-core/src/export/gltf.rs` (glTF materials) and `tools/unreal/import_map.py`
(Unreal post-processing). Knowledge from ValveResourceFormat's shaders (MIT), checked against CS2 screenshots.

## What a compiled material holds

The `DATA` block (KV3) has `m_shaderName` and parameter lists: `m_textureParams` (`m_name` → `m_pValue`
resource), `m_intParams` (`F_*` feature flags and `g_n*`), `m_floatParams` (`g_fl*`), `m_vectorParams` (`g_v*`).
OFAST keeps the shader name, textures and floats in the glTF material's `extras.ofast`.

## Channel conventions of CS2 (`csgo_*`) shaders

These were the source of the "missing textures" and the blue-grey mirror walls in the first de_mirage exports:

| Texture | Channels |
|---|---|
| `g_tColor` | RGB colour. **Alpha is a mask, not opacity**, unless `F_TRANSLUCENT` / `F_ALPHA_TEST` / overlay. |
| `g_tNormal`, `g_tLayer*NormalRoughness` | **Hemi-octahedral normal in RG, roughness in B** — for every `csgo_*` shader, whatever the texture format (BC7 or BC5). |
| `g_tMetalness` | **Metalness in G**. R is roughness only for `csgo_weapon` (otherwise retro-reflectivity); B is a cloth mask on characters. |
| no `g_tMetalness` | uniform `g_flMetalness`. |

glTF wants roughness in G and metalness in B of one texture, so OFAST assembles that texture from the channels
above (sampling the smaller source at the larger size).

## Two-layer blend materials (`csgo_lightmappedgeneric`, `F_LAYERS`)

Ground and painted walls mix two texture sets (`g_tColor` / `g_tLayer2Color`, …) with a per-vertex weight
(vertex attribute `TEXCOORD 4`, `R8G8B8A8_UNORM`; all four channels hold the same value) shaped by
`g_tBlendModulation`:

```
blend = smoothstep(max(0, mask − softness), min(1, mask + softness), weight)
F_FANCY_BLENDING 1: mask = modulation.G, softness = modulation.R
F_FANCY_BLENDING 2: mask = modulation.G, softness = g_flBlendSoftness
F_FANCY_BLENDING 3: mask = modulation.A, softness = g_flBlendSoftness
F_FANCY_BLENDING 0: blend = weight
```

glTF has no layered materials: OFAST writes the weight as `COLOR_0` and both layers' PNGs in
`extras.ofast.layers`. `import_map.py` builds `M_OFAST_LayerBlend_v2` (by script, from material nodes) and gives
each blend material an instance of it.

Only `csgo_lightmappedgeneric` materials count as these blends (de_mirage 13, de_dust2 77, de_nuke 11). Other
shaders with layers are different things and show their first layer only for now:

- `csgo_simple_3layer_parallax` (de_inferno's windows, 9 materials) has a `g_tLayer2Color` too, but its layers are a
  fake room at depths behind the glass (`g_flLayer1Offset`, `g_flLayer2Offset`), not a painted blend. Treating it as a
  blend crashed the Unreal import, which found no layer-2 normal map.
- `csgo_environment_blend` (369 of de_inferno's 922 materials: walls and streets) mixes `g_tColor1` / `g_tColor2`
  using height maps (`g_tHeight1/2`) and its own colour adjustments.

## Other rules

| Case | Handling |
|---|---|
| `F_TRANSLUCENT`, `F_BLEND_MODE`, `csgo_static_overlay` | glTF `BLEND` |
| `F_ALPHA_TEST` | glTF `MASK`, cutoff 0.5 |
| overlays (decals, `OBJECT_TYPE_OVERLAY`) | pushed 0.1 inch along their normal so they win the depth test |
| `csgo_effects` (dust and light-shaft cards) | not exported (they appeared as white sheets; to be rebuilt as Unreal effects) |
| tints (`m_vTintColor`, `rendercolor`) | `baseColorFactor` (sRGB → linear), material named `<name>_tintRRGGBB` |
| same file name in two folders | the second material is renamed `<name>_<folder>` (Unreal matches slots by name) |

## Unreal project requirement

Interchange imports meshes as **Nanite**. Nanite needs **DirectX 12 + Shader Model 6**; without it Unreal draws a
simplified fallback mesh (railings become thick blobs, some surfaces vanish). `unreal/Sandbox/Config/DefaultEngine.ini`
enables both.

## Sources

- ValveResourceFormat (MIT): `Renderer/Shaders/complex.frag.slang`, `Renderer/Shaders/common/utils.slang`
  (`ApplyBlendModulation`) — https://github.com/ValveResourceFormat/ValveResourceFormat
- CS2 ground truth: de_mirage CT spawn screenshot vs Unreal render (`test-data/shots/compare_de_mirage_ct0.png`,
  local only).
