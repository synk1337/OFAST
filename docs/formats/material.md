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

glTF has no place for these, so the exporter bakes each layer: `<colour>_cc_<hash>.png` (colour after its
adjustments, without the model tint — see "Model tint" below) and `<normal>_surface_<hash>.png` (glTF
metallic-roughness: R occlusion, G roughness, B metalness; also the occlusion texture). Layer 1 is the glTF material.

### Which bakes are written

A bake is keyed by what changes its pixels, nothing else (`ColorJob::new`, `SurfaceJob::new` in `environment.rs`):

| Setting | Counts for the colour bake when | Counts for the surface bake |
|---|---|---|
| contrast / saturation / brightness | not all 1 | never |
| `g_vTextureColorTintN` | it has a saturation (white and greys change nothing) | never |
| `g_nColorCorrectionModeN` | there is a colour correction | never |
| tint mask (`g_tHeightN`.G, its contrast and brightness) | something reads the mask (a tint, a baked model tint, or a correction in "Mask All") | never |
| model tint (`rendercolor`, scene-object tint) | only with `--bake-model-tints`: not white, and the layer takes one (`g_bModelTintN` × `g_flModelTintAmount` ≠ 0) | never |
| roughness, metalness, occlusion levels | never | always |
| `g_tColorN` alpha / `g_tHeightN` | — | colour alpha unless alpha-tested; height when it holds metalness or (alpha-tested) occlusion |

A colour that nothing changes is not baked: the material uses the plain colour texture (exact, where a bake would
round dark values through the linear table). The height texture (as stored, `<height>_raw.png`) is written where
Unreal reads it: for `csgo_environment_blend` (the height blend) and for a layer whose model tint Unreal applies (the
tint mask). Then the glTF writer stores identical pixels once, whatever made them, and names a bake after the first
8 hex digits of its pixel hash, so a file name always means the same pixels (`import_map.py` imports each texture
name once and would otherwise keep a stale bake after a re-export).

Why it matters: de_inferno's placements give one plaster texture over 100 tints. Baking every (texture, settings,
tint) wrote 1049 colour bakes (8.9 GB, at 2048² mostly). Keyed by what changes the pixels, with the tints still
baked: 555 (3.3 GB), export 5.4 GB. With the tints left to Unreal: 126 bakes (0.76 GB) — the environment layers use
269 colour textures, bakes and plain ones — plus 39 height textures for tinted single-layer materials (10 MB); the
`textures` folder holds 1099 files, 2.6 GB, instead of 1467 files, 5.1 GB, and the export 3.3 GB.

### Model tint

The model tint (`rendercolor`, a placement's tint colour) recolours a layer's tint-masked area: towards the tint's
hue at the same luminance, capped at 3 × luminance × the tint's brightest channel, by strength × mask × (1 − the
tint's darkest channel), strength = `g_flModelTintAmount` × `g_bModelTintN` (both default 1). A white tint changes
nothing, and a layer with the strength at 0 ignores tints.

Every placement tint gets its own glTF material (`<name>_tintRRGGBB`), and by default all of them share the
untinted colour texture. The tint travels in `extras.ofast.environment`, written for every environment material:

```
"environment": {
  "model_tint": [r, g, b],            linear RGB (white = none)
  "model_tint_baked": false,          true with --bake-model-tints: already in the colour, do not apply again
  "layer1": { "color", "normal", "surface", "height": "textures/….png" or null,
              "tint_strength": g_flModelTintAmount × g_bModelTint1, "tint_mask": [contrast, brightness] },
  "layer2": { … }                     csgo_environment_blend only
}
```

`import_map.py` applies it with the material function `MF_OFAST_ModelTint_v1` (the formula in HLSL; the reference
is `model_tint` in `environment.rs`), the mask being the height texture's G after the layer's contrast and
brightness, as in the bake. Its users:

- `M_OFAST_HeightBlend_v2` (every `csgo_environment_blend`): each layer's colour is tinted, then the two are blended.
- `M_OFAST_Environment_v1` (every single-layer `csgo_environment`, tinted or not): colour, normal, surface and height
  of layer 1; an instance overrides the blend mode for alpha-tested (masked at the glTF `alphaCutoff`) and
  translucent materials, and two-sidedness. Not Interchange's glTF material: a re-import keeps existing material
  instances unchanged, and de_inferno's project still showed the conversion from before the environment bakes
  (plain colour × the tint as a factor): its terrace tablecloths were solid dark red instead of red and white check.
- `M_OFAST_Skybox_v3` (3D-skybox geometry): layer 1's tint, like the rest of its glTF stand-in.

Checked on de_inferno against the same export with `--bake-model-tints`, both through these masters: base-colour
renders (no lighting) of the three views differ by 0.3–0.6 sRGB levels on average.

Other glTF readers (Blender, viewers) see the untinted colour. `ofast export-map --bake-model-tints` bakes each
tint into its own colour texture instead (the earlier behaviour: de_inferno 555 colour bakes), and sets
`model_tint_baked` so that Unreal does not tint twice.

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
exporter lists both layers' files (colour, normal, surface, height) in `extras.ofast.environment` (above) and the
blend settings (`softness`, `zero_point`, `scale`, `mask_with_height`) in `extras.ofast.height_blend`;
`import_map.py` builds `M_OFAST_HeightBlend_v2` with the formula in a Custom (HLSL) node and gives each blend
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
| tints (`m_vTintColor`, `rendercolor`) | `baseColorFactor` (sRGB → linear), material named `<name>_tintRRGGBB`; environment shaders tint only their tint mask, in Unreal's material ("Model tint" above) |
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
