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

Decoded CS2 normals follow glTF's convention, +Y up in the texture (checked on three de_inferno textures: the normal's
Y correlates with the height map's slope down the image rows, r ≈ +0.2 to +0.35, and X against the slope along
the columns, r ≈ −0.15 to −0.35). Unreal's convention is +Y down: Interchange sets `flip_green_channel` on the glTF
normal textures it imports, and `import_map.py` sets it on the ones it imports itself for the blend materials (it
did not before 2026-10-16, so their bumps were lit upside down).

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
each blend material an instance of it. Unreal hands `COLOR_0` to materials as the linear value written (checked
with `tools/unreal/probe_vertex_color.py`: no sRGB encoding on the way).

Only `csgo_lightmappedgeneric` materials count as these blends (de_mirage 13, de_dust2 77, de_nuke 11).
`csgo_simple_3layer_parallax` (de_inferno's windows, 9 materials) has a `g_tLayer2Color` too, but its layers are a fake
room at depths behind the glass (`g_flLayer1Offset`, `g_flLayer2Offset`), not a painted blend; it shows its surface
layer only for now. Treating it as a blend crashed the Unreal import, which found no layer-2 normal map.

## Environment materials (`csgo_environment`, `csgo_environment_blend`)

The newer maps' shaders: 334 + 369 of de_inferno's 922 materials (walls, streets, props). Sources: Valve's compiled
shaders as decompiled by Source 2 Viewer (tracked in SteamTracking's GameTracking-CS2) for parameter names, defaults,
channels and the default pixel-shader path; Source 2 Viewer's reconstruction of the colour-matrix functions, which
Valve's files only name. Code: `crates/ofast-core/src/export/environment.rs`.

| Texture (layer N) | R | G | B | A |
|---|---|---|---|---|
| `g_tColorN` (sRGB) | colour | colour | colour | occlusion (translucency with `F_ALPHA_TEST`) |
| `g_tHeightN` | height | tint mask | occlusion with `F_ALPHA_TEST` | metalness (if `g_bMetalnessN`, default on) |
| `g_tNormalN` | hemi-octahedral normal | hemi-octahedral normal | roughness | — |

Per layer, the shader adjusts the colour (`c` linear, `avg` = the colour texture's stored average, its
"reflectivity"; parameters missing from the material take the shader's defaults):

```
mask      = saturate(((height.G − 0.5) · g_fTintMaskContrastN + 0.5) · g_fTintMaskBrightnessN)
corrected = colour correction: contrast around avg, brightness, then saturation around the grey Σ cᵢlᵢ² / Σ lⱼ²
            (l = Rec. 709 weights)                    g_fTextureColorContrastN / BrightnessN / SaturationN
adjusted  = corrected pulled towards g_vTextureColorTintN by its saturation s (chroma × s, luminance × s²)
colour    = lerp(mode 1 ? corrected : c, adjusted, mask)          g_nColorCorrectionModeN: 0 "Mask All", 1 "Mask only Tint"
colour    = model tint (rendercolor): towards the tint's hue at the same luminance, by mask × (1 − darkest channel)
roughness = saturate(((normal.B − 0.5) · g_fTextureRoughnessContrastN + 0.5) · g_fTextureRoughnessBrightnessN)
occlusion = saturate(lerp(x, z, a^y)), g_vAmbientOcclusionLevelsN → (−x, −log2(1 − y), 2 − z)
```

"Mask All" means the tint mask limits every adjustment, not only the tint. Unset occlusion levels are (0, 0.5, 0), so
occlusion = saturate(2a); most materials set (0, 0.5, 1), i.e. occlusion = a.

glTF has no place for these, so the exporter bakes each layer: `<colour>_cc.png` (colour after all adjustments and
the model tint — tinted variants get their own baked texture instead of a `baseColorFactor`) and
`<normal>_surface.png` (glTF metallic-roughness: R occlusion, G roughness, B metalness; also the occlusion texture).
Layer 1 is the glTF material.

`csgo_environment_blend` mixes its two layers by height. The weights are painted per vertex in `TEXCOORD 4`
(`VertexPaintBlendParams`): x = layer 2, w = extra softness; OFAST writes them as `COLOR_0`. Layer 2's weight
(without the shader's view-distance softening):

```
paint = saturate(1.1 x − 0.05)        soft = clamp(w + g_flBlendSoftness2, 0.0001, 1)
Sᵢ = g_flHeightMapScaleᵢ + soft       zᵢ = g_flHeightMapZeroPointᵢ (default 0.5)
H1 = (h1 − z1) S1
H2 = (h2 − z2)(S2 − 2 soft) + lerp(−z1 S1 − (1 − z2) S2 − soft, (1 − z1) S1 + z2 S2, paint)
wᵢ = max(Hᵢ − max(H1, H2) + soft, 0)
weight = saturate(w2 / (w1 + w2 + 0.0001) · lerp(0.5, h2, g_flMaskWithHeight2) · 2)
```

Paint 0 always gives layer 1 and paint 1 layer 2; in between, the higher surface (stones above mortar) wins. The
exporter lists both layers' files (colour, normal, surface, height) and the settings in `extras.ofast.height_blend`;
`import_map.py` builds `M_OFAST_HeightBlend_v1` with the formula in a Custom (HLSL) node and gives each blend
material an instance of it.

Not reproduced yet: `F_BLEND_EFFECTS_2` (a border tint and bevel along the blend line, 333 of the 369), the shared
colour overlay (`F_SHARED_COLOR_OVERLAY`, 167: a grime texture on a second UV set), normal contrast
(`g_fTextureNormalContrastN`), per-vertex tint (`COLOR0`), UVs scaled by the model's scale
(`g_nScaleTexCoordUByModelScaleAxis`), and the parallax windows.

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
