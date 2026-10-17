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
  translucent materials, and two-sidedness. Not Interchange's glTF material, which has no model tint. (That is how
  stale Interchange materials were found: de_inferno's project still showed the conversion from before the
  environment bakes, its terrace tablecloths solid dark red instead of red and white check. `import_map.py` now
  rebuilds the Interchange materials an older export left, see
  [decision 006](../decisions/006-reimport-rebuilds-stale-interchange-assets.md).)
- `M_OFAST_Skybox_v4` (3D-skybox geometry): layer 1's tint, like the rest of its glTF stand-in.

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

## Model tint of prop shaders and their tint masks

`csgo_complex`, `csgo_vertexlitgeneric`, `csgo_foliage` and `csgo_simple_3layer_parallax` (props, plants, doors,
crates) **multiply** the colour by the placement's tint. Unlike the environment shaders they do not keep luminance:
white becomes the tint, green under a red tint becomes nearly black. Code: `crates/ofast-core/src/export/tint.rs`.

The tint each placement gets (the shaders' vertex code, decompiled in GameTracking-CS2; colours sRGB → linear):

```
tint   = lerp(1, rendercolor, g_flModelTintAmount) × g_vColorTint      defaults 1 and white
         csgo_simple_3layer_parallax has no g_flModelTintAmount (as if 1)
         csgo_vertexlitgeneric with F_NOTINT ("Prevent Tinting"): no tint at all
colour = albedo × tint                                   without F_TINT_MASK
colour = albedo × lerp(1, tint, g_tTintMask.R)           with F_TINT_MASK
```

`g_flModelTintAmount` 0 means the material ignores placement tints (120 `csgo_complex` materials on the six maps
exported so far, mostly de_nuke's); `g_vColorTint` tints a material whatever its placement (54 `csgo_complex`
ones, mostly light greys). `g_tTintMask` is a single-channel texture (BC4), read in R, white by default; with
`g_bUseSecondaryUvForTintMask` it uses the second UV set (20 materials). It has no contrast or brightness of its
own. The masked formula is the one part not read from Valve's files: they decompile only each shader's default
variant, which has no tint mask. It is Source 2 Viewer's reconstruction, and `csgo_vertexlitgeneric`'s default
variant has the same form (`lerp(albedo, albedo × tint, saturate(colour.a + g_flInverseBlendTint))`, with
`g_flInverseBlendTint` 1 without a mask, 0 with one, −1 with `F_NOTINT`). Checked in CS2 on de_inferno's T-spawn
tree ([decision 007](../decisions/007-tint-masks-of-prop-shaders.md)).

Where masks are used (`cargo run --release --example shader_census`, the 24 official maps): `csgo_complex` 426
materials, `csgo_vertexlitgeneric` 132, `csgo_foliage` 40, `csgo_simple_3layer_parallax` 24, and
`csgo_static_overlay` 5 (not handled: its "Material Reference" mode takes an environment-style mask, unchecked).
Each placement tint makes one more glTF material: de_dust2 has 264 tinted `csgo_vertexlitgeneric` ones (doors, window
frames, crates), de_nuke 198 `csgo_complex` ones, de_inferno 80 (its potted plants and bushes, hanging clothes,
parasols, luggage), de_anubis 76, de_vertigo 28, de_mirage 1.

How OFAST writes them:

- **Without a tint mask** (or a mask without a colour texture): the tint is the glTF `baseColorFactor`, exact.
- **With a tint mask** under a tint that is not white: the colour texture stays untinted (shared by every tint, the
  factor white) and the tint goes into `extras.ofast.tint_mask`:

  ```
  "tint_mask": {
    "tint": [r, g, b],     linear, the formula above
    "texture": "textures/<mask>_raw.png" (the mask in R), null when baked
    "uv": 0 | 1,           the mask's UV set
    "baked": false         true: already in baseColorTexture, do not apply again
  }
  ```

  `import_map.py` gives these materials an instance of `M_OFAST_TintMask_v1`: glTF's metallic-roughness inputs as
  Interchange's glTF material uses them (colour × `BaseColorFactor`, normal, roughness and metalness × their
  factors, occlusion, alpha mode and sidedness as instance overrides) with the colour × lerp(1, `ModelTint`,
  `TintMaskTexture`.R), the mask sampled with UV set 0 or 1 (`TintMaskUV`; set 1 is not checked in a render yet: no
  tinted placement on the maps exported so far uses it). Untinted placements keep Interchange's glTF material.
  Checked on de_inferno against CS2: the T-spawn tree and the CT spawn's flower boxes have green leaves and red
  flowers, as in CS2, instead of being red all over (decision 007). On de_dust2 the T spawn's kerb is red and white
  as in CS2 (its white segments are `F_NOTINT` and were tinted pink), and on de_nuke the T spawn's blue door has
  CS2's hue. Tinted props on de_nuke stay a little less saturated than CS2, masked or not, like its untinted shaded
  walls: lighting or CS2's colour LUT, not the tint (decision 007).
- **`ofast export-map --bake-model-tints`**: the masked tint is baked into a colour texture of its own
  (`<texture>_tint_<hash>.png`, `"baked": true`), for other glTF readers. Masks on the second UV set have another
  layout than the colour and are never baked: Unreal applies those either way.

Why not bake by default: de_dust2 has 261 (colour, tint) pairs over 44 prop textures (670 MB of PNGs), de_nuke 190
over 61 (251 MB); the masks the material reads instead are small (BC4, 512² for de_inferno's plants).

Not reproduced: `csgo_foliage`'s colour correction (`g_vTextureColorCorrectionTint`, contrast / saturation /
brightness, masked or not by `g_nColorCorrectionMode`; neutral on every map exported so far), its height gradient
(vertex colour R brightens the colour by up to `g_flHeightGradientAmount`: de_inferno's downey willow bushes have 1)
and vertex-colour occlusion; the tint of shaders not checked (`csgo_lightmappedgeneric`, `csgo_static_overlay`,
`csgo_simple`, …) stays a plain multiply by the placement tint over the whole surface, ignoring their
`g_flModelTintAmount`, `g_vColorTint` and masks.

The 3D skybox's master (`M_OFAST_Skybox_v4`, see `setup_skybox` in `import_map.py`) applies the same masked tint:
colour × lerp(1, `MaskedTint`, `TintMaskTexture`.R), the mask on UV set `TintMaskUV`. `MaskedTint` is a parameter of
its own, apart from the environment shaders' `ModelTint`, and stays white for materials without a mask. Three masked
materials sit in skyboxes: de_anubis `venice_boat_2_tintc7c5c5`, de_dust2 `dust_windows_01_tint000000` and de_nuke
`curbs_001_tint000000`. They are small or far away (boats off de_anubis's coast, an antenna card, a strip of
floor), so they were checked by reading the three imported instances back (parent v4, `MaskedTint` = the tint,
`TintMaskTexture` = the mask), not in a render.

## Draw-call tint

A model's draw call can carry a colour of its own, next to its material: `m_vTintColor` (three floats) and
`m_flAlpha` in the mesh's `MDAT` ([model.md](model.md#mdat-mesh--draw-calls)). Hammer stores there the colour picked
for a mesh face or an overlay (materials whose searchable user data has `VertexPaintUIPickColor`), so the material
can stay white and be reused in any colour. de_nuke's road paint is the case that showed it: the parking bays,
"no parking" lines and "NO-IDLE ZONE" text are `hr_nuke_parkingstripe_single`, `hr_nuke_noparking_tiled` and
friends, `csgo_lightmappedgeneric` overlays whose colour texture is translucency only (RGB white,
`TextureLayer1Color` white), placed by scene objects with a white `m_vTintColor`, yet CS2 draws them yellow. The
yellow is the draw call's (0.851, 0.459, 0.046) in the overlay model (`n0_lr0_c202_s_mesh_overlay202.vmdl`).

- **The floats are linear.** Through gamma 2.2 every value found gives whole sRGB bytes (the road paint 237.0,
  179.0, 63.0; the floor arrows 118.0), through the sRGB curve they do not (237.5, 180.5, 60.7): Hammer's colour
  picker bytes, raised to 2.2.
- **How CS2 applies it.** Its shaders get one 8-bit sRGB tint per instance and decode it in the vertex shader
  (`csgo_complex`, `csgo_static_overlay`: `lerp(1, tint, g_flModelTintAmount) × g_vColorTint`, opacity × the tint's
  alpha). The draw call's colour reaches them that way, like a placement tint.
- **How OFAST applies it.** The draw call's tint is encoded back to sRGB (gamma 2.2) and multiplies the placement's
  tint (a scene object's `m_vTintColor`, an aggregate fragment's, an entity's `rendercolor`), its alpha the
  placement's alpha (`DrawCall::placement_tint` in `crates/ofast-core/src/model/mod.rs`). The product then goes
  through the material's tint rules like any placement tint (the sections above): a `baseColorFactor` for most,
  `extras.ofast.tint_mask` for masked prop shaders, `extras.ofast.environment.model_tint` for environment shaders
  (which ignore the alpha; no tinted draw call of theirs has one below 1). Materials with `g_flModelTintAmount` 0 or
  `F_NOTINT` ignore it, as in CS2 (de_nuke's `airduct_hvac_001` and `hr_metal_railing_001_card`, painted grey,
  stay as their texture), and a masked one tints its mask only (`hazard_stripe_001_metal_railing`, painted the road
  paint's yellow). A variant with less than full alpha is named `<name>_tintRRGGBBAA`.
- **Which order the engine multiplies them in is not known**: on the nine maps counted below, every tinted draw call
  is placed with a white tint, and no aggregate fragment draws a tinted draw call (the fragments' own `m_vTintColor`
  carries Hammer's colour there).

Tinted draw calls in the main world (`cargo run --release --example drawcall_tints -- <map>`, which lists each
material with its tint, alpha and shader; the 3D skybox adds its own, e.g. de_nuke's tinted far buildings):

| Map | Overlays | Other scene objects |
|---|---|---|
| de_train | 578 of 707 | 72 of 378 |
| de_overpass | 148 of 384 | 22 of 214 |
| de_nuke | 145 of 237 | 31 of 149 |
| de_anubis | 75 of 135 | 55 of 127 |
| de_vertigo | 44 of 92 | 8 of 52 |
| de_inferno | 32 of 263 | 23 of 203 |
| de_dust2 | 14 of 56 | 37 of 88 |
| de_ancient | 9 of 25 | 2 of 92 |
| de_mirage | 8 of 301 | 3 of 56 |

Checked on de_nuke against CS2 ([decision 008](../decisions/008-draw-call-tints.md)): the parking bays and
"no parking" lines are yellow at both spawns (hue 36–38°, CS2 43–44° in the sun; white before). In direct sun the
paint stayed paler, as the white lines did before: Unreal lit the translucent overlay apart from the asphalt under
it, now fixed with mesh decals ("Translucent overlays: mesh decals" below).

Many are opacity only: de_nuke's 35 scuff decals (`scuffs_overlay_001`, alpha 0.12–0.73) were drawn at full
strength before. Others are colours: de_nuke's grey floor arrows (118), red stripes and "X" marks, its silo and pipe
trims. Most shaders are involved (`csgo_static_overlay`, `csgo_lightmappedgeneric`, `csgo_complex`, `csgo_glass`,
a few `csgo_environment(_blend)` on de_train and de_overpass; `csgo_effects` is not exported).

## Translucent overlays: mesh decals

An overlay is a surface lying on another one: road paint, signs, stains. CS2 lights it like the surface under it
(same lightmap texel, same sun shadow), so blending α·lit(paint) + (1 − α)·lit(ground) is the same as lighting
α·paint + (1 − α)·ground once. A translucent Unreal material is lit on its own instead: Interchange's glTF material
for `BLEND` (`MI_Default_Blend`, parent `/InterchangeAssets/gltf/M_Default`) uses the translucency lighting mode
Surface TranslucencyVolume, i.e. direct light from the translucency lighting volume and indirect light from Lumen's
translucency volume, both coarse grids around the camera. de_nuke's yellow paint got less sun than the asphalt it
lies on and turned orange-brown; the white lines before it were grey (175 on 167).

So `import_map.py` turns the overlays blended with plain alpha into **mesh decals**: `csgo_lightmappedgeneric` with
`F_OVERLAY` + `F_TRANSLUCENT`, lit `csgo_static_overlay` with `F_BLEND_MODE` 1, and translucent `csgo_complex` with
`F_DEPTH_BIAS` (`is_decal_overlay`). The last are models lying on a wall or floor, placed by Hammer's mesh overlays:
de_inferno's trim decals (plaster patches, leaks and cracks over the house fronts, 67 placements), de_nuke's drain
covers and the SWAT van's lettering, de_vertigo's `sign_number_0` ([decision
010](../decisions/010-translucent-decals-and-per-pixel-lighting.md)). Each gets an instance of
`M_OFAST_OverlayDecal_v2` (Deferred Decal domain, translucent): colour texture × `baseColorFactor` (the draw-call or
placement tint) × lerp(1, `ModelTint`, `TintMaskTexture`.R) (a tint mask, as for the prop shaders above:
`inferno_trim_decal_01`'s; white otherwise), opacity = colour alpha × the factor's alpha (a draw call's alpha),
normal, roughness and metalness with their factors. Unreal draws a decal material on a static mesh as a mesh decal:
it blends colour, normal and roughness into the GBuffer before lighting (DBuffer decals), which is CS2's arrangement.
The instances go on the world's placements as component overrides; the 3D skybox's copies keep the translucent
material. The glTF is not changed.

| de_nuke, paint / asphalt luminance | CT spawn, sun: bays / line | T spawn, shade: bays |
|---|---|---|
| CS2 | 2.35 / 2.07 | 2.29 (hue 62°) |
| translucent (before) | 0.97 / 0.95 | 1.62 (hue 38°) |
| mesh decal | 2.07 / 1.86 | 1.86 (hue 57°) |

Measured with `tools/compare/overlay_contrast.py` on the most saturated 5 % of yellow pixels against the asphalt
beside them, linear luminance; details, the per-pixel translucent alternative and the remaining 10–20 % (CS2's
opacity or its asphalt's albedo, open) in [decision 009](../decisions/009-overlays-as-mesh-decals.md). In Unreal's
base-colour view the paint is a plain alpha blend at the texture's opacity (0.43–0.45 measured, texture 0.49).

`csgo_static_overlay`'s other blend modes (`F_BLEND_MODE` 0 Opaque, 2 Alpha Test, 3 Mod2x, 4 Additive, 5 Multiply,
6 ModThenAdd, from its feature list in GameTracking-CS2) are still exported as plain `BLEND`, which is wrong for
them; de_nuke has 30 Mod2x overlays (unlit grime).

## Other translucent surfaces: lit per pixel

Translucent surfaces that do not lie on another one (glass, chain-link fence and palm cards, signs such as
de_dust2's container sign) stay translucent, lit with the translucency lighting mode **Surface ForwardShading**
(direct light per pixel with its shadows; [lighting.md](lighting.md#translucent-surfaces-and-overlays)). Interchange's
glTF master uses Surface TranslucencyVolume, which an instance cannot change, and the plugin's content is not edited,
so `import_map.py` keeps copies in `/Game/OFAST/Materials`: `M_OFAST_glTFDefault_v1` (Interchange's `M_Default` with
the per-pixel mode) and `MI_OFAST_glTFDefault_Blend_v1` / `MI_OFAST_glTFDefault_Blend_DS_v1` (its one- and two-sided
translucent instances, parented to the copy). After the scene import, `light_translucent_per_pixel` re-parents the
Interchange instance of every `BLEND` material `lit_per_pixel` names from `MI_Default_Blend(_DS)` to the copy; the
parameters are the same, so the instance keeps its textures and factors. Left out: the decal overlays above, the
static overlays in their other blend modes (Mod2x, alpha test …; neither lighting mode is right for them, they await
their own handling) and the unlit `csgo_unlitgeneric` cards. OFAST's environment and tint-mask
masters, whose instances can be translucent through `set_alpha_overrides`, have the per-pixel mode too. Measured in
[decision 010](../decisions/010-translucent-decals-and-per-pixel-lighting.md): in the sun the SWAT lettering and the
container sign were at 0.81 and 0.70 of their per-pixel luminance before.

## Other rules

| Case | Handling |
|---|---|
| `F_TRANSLUCENT`, `F_BLEND_MODE`, `csgo_static_overlay` | glTF `BLEND`; in Unreal, overlays blended with plain alpha and `csgo_complex` decals (`F_DEPTH_BIAS`) are mesh decals ("Translucent overlays" above), the other translucent surfaces are lit per pixel ("Other translucent surfaces" above) |
| `F_ALPHA_TEST` | glTF `MASK`, cutoff 0.5 |
| overlays (decals, `OBJECT_TYPE_OVERLAY`) | pushed 0.1 inch along their normal so they win the depth test |
| `csgo_effects` (dust and light-shaft cards) | not exported (they appeared as white sheets; to be rebuilt as Unreal effects) |
| tints (`m_vTintColor`, `rendercolor`, a draw call's own tint) | material named `<name>_tintRRGGBB` (`<name>_tintRRGGBBAA` when the alpha is not full); `baseColorFactor` (sRGB → linear) when the tint covers the whole surface; environment shaders tint only their tint mask, in Unreal's material ("Model tint" above), and so do prop shaders with `F_TINT_MASK` ("Model tint of prop shaders" above); the draw call's tint multiplies the placement's first ("Draw-call tint" above) |
| same file name in two folders | the second material is renamed `<name>_<folder>` (Unreal matches slots by name) |

## Unreal project requirement

Interchange imports meshes as **Nanite**. Nanite needs **DirectX 12 + Shader Model 6**; without it Unreal draws a
simplified fallback mesh (railings become thick blobs, some surfaces vanish). `unreal/Sandbox/Config/DefaultEngine.ini`
enables both.

## Sources

- ValveResourceFormat (MIT): `Renderer/Shaders/complex.frag.slang` (also the masked model tint),
  `Renderer/Shaders/common/utils.slang` (`ApplyBlendModulation`) — https://github.com/ValveResourceFormat/ValveResourceFormat
- Valve's shaders decompiled by Source 2 Viewer, in SteamDatabase's GameTracking-CS2:
  `game/csgo/shaders_vulkan_dir/shaders/vfx/<shader>.slang` (parameter declarations and defaults, each shader's
  default variant) — https://github.com/SteamDatabase/GameTracking-CS2
- CS2 ground truth: de_mirage CT spawn screenshot vs Unreal render (`test-data/shots/compare_de_mirage_ct0.png`,
  local only).
