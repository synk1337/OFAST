# 010 — Static overlays by their blend mode; Mod2x grime multiplies what lies under it

- **Date:** 2026-10-17
- **Status:** accepted

## Context

Decision 009 found that every `csgo_static_overlay` material (Hammer's overlays: grime, signs, floor markings) was
exported as glTF `BLEND`, whatever its `F_BLEND_MODE`. The shader's feature list (decompiled in GameTracking-CS2,
`csgo_static_overlay.slang`) numbers the modes `0="Opaque", 1="Translucent", 2="Alpha Test", 3="Mod2x",
4="Additive", 5="Multiply", 6="ModThenAdd"`; only modes 0–2 can be lit. On the six exported maps:

| Map | Opaque | Translucent | Alpha Test | Mod2x | other modes |
|---|---|---|---|---|---|
| de_nuke | 2 | 7 | 1 | 30 | — |
| de_inferno | — | 13 | 6 | 1 | — |
| de_anubis | — | — | 7 | 4 | — |
| de_vertigo | — | — | 1 | 3 | — |
| de_mirage | — | 2 | — | 1 | — |
| de_dust2 | — | 1 | — | 1 | — |

(`csgo_unlitgeneric` has an `F_BLEND_MODE` too, one material each on de_dust2 and de_mirage; its numbering is not
checked.) de_nuke's Mod2x materials are its scuffs (`scuffs_overlay_001`, 20 draw-call alphas from 0.12 to 0.73),
floor markings and arrows, a metal hatch's frame and three animated caustics. Drawn as a plain alpha blend, their
colour textures — mostly 128 grey, the neutral value of a multiply by two — put grey veils over the floors.

Alpha-tested materials of every shader were also cut at a fixed 0.5, while their `g_flAlphaTestReference` ranges
from 0.05 to 0.9 on the six maps (`csgo_foliage` 0.2–0.9, de_nuke's railing cards 0.225, its hanging wires 0.089).

### How CS2 draws Mod2x

- **Render state** of the shader: `DstBlend (F_BLEND_MODE==3) ? One : InvSrcAlpha`, depth bias −64, no depth writes,
  unlit only (`FeatureRule(Requires(F_LIT==1, F_BLEND_MODE==0, F_BLEND_MODE==1, F_BLEND_MODE==2))`).
- **The colour is read raw**: in mode 3 the shader declares `g_tColor` with `SrgbRead(false)` (every other mode reads
  it with sRGB decoding), so a texel of 128 is 0.502 in the shader.
- **Source 2 Viewer's renderer** (`complex.frag.slang`, reference only) undoes the sRGB decode for mode 3, then writes
  `mix(vec3(0.5), colour, alpha)` for a blend of 2 × source × destination; Source 1's `DecalModulate` did the same.
- **The tint**: the vertex shader decodes the placement / draw-call tint from sRGB to linear (CS2's
  `pow(x × 0.948 + 0.052, 2.4)`) and multiplies the colour by it; its alpha multiplies the colour's.

So the picture under a Mod2x overlay is multiplied by

```text
m = lerp(1, 2 × colour × tint, colour alpha × tint alpha)     colour: raw texture value, tint: linear
```

128 grey changes nothing, darker texels darken (a footprint at 40 → × 0.31), brighter ones brighten up to 2×
(scratches, caustics). de_nuke's floor arrows are a white texture under the draw-call tint 767676 (sRGB 118, linear
0.18): × 0.36 where the arrow is opaque, which is why CS2 shows them dark grey.

### What Unreal can do

- **A Modulate translucent material** multiplies the scene colour by its output: blend `BF_DestColor × source`
  (`TranslucentRendering.cpp`), the output not pre-exposed and faded to 1 by fog (`BasePassPixelShader.usf`,
  `MATERIALBLENDING_MODULATE`), on the HDR scene colour before the tone curve. Values above 1 brighten. Rendered in
  the before-depth-of-field pass, standard translucency draws straight into the scene colour (float).
- **A Modulate decal cannot**: on a DBuffer platform (`r.DBuffer`, which decision 009's overlay decals need) Unreal
  turns a Modulate decal into a plain translucent one (`FinalizeBlendDesc` in `DecalRenderingCommon.cpp`:
  `if (bIsDBufferPlatform && IsModulateBlendMode(Desc)) Desc.BlendMode = BLEND_Translucent`). And the DBuffer itself
  holds a premultiplied colour and one coverage value, 8 bits each, applied as
  `BaseColor × ColorOpacity + PreMulColor` (`DBufferDecalShared.ush`, `ApplyDBufferData`): it can scale the base
  colour by a single value from 0 to 1 and add a colour, so no brightening and no per-channel multiply. A grey
  darkening would fit (`ColorOpacity = m`), the scratches, caustics and tinted marks would not.

## Decision

- **Exporter** (`OverlayBlend` in `crates/ofast-core/src/export/gltf.rs`): static overlays by their blend mode —
  Opaque → glTF `OPAQUE` (colour without alpha), Translucent → `BLEND`, Alpha Test → `MASK`, Mod2x and the modes not
  checked → `BLEND` (glTF has no multiply blend; `extras.ofast.ints.F_BLEND_MODE` says which). Other shaders keep
  their features. **Every `MASK` is cut at the material's `g_flAlphaTestReference`** (0.5 when the material keeps
  the shaders' default).
- **Unreal** (`import_map.py`: `is_mod2x_overlay`, `mod2x_instances`, `ensure_mod2x_master`): Mod2x overlays get
  instances of `M_OFAST_OverlayMod2x_v1` — Surface domain, **Modulate** blend, unlit, before depth of field, emissive
  = `lerp(1, 2 × colour × BaseColorFactor.rgb, colour alpha × BaseColorFactor.a)` with the colour texture imported as
  linear (raw, like CS2's `SrgbRead(false)`). They go on the meshes like the other OFAST materials
  (`swap_material_slots`), so the 3D skybox's copies get them too (a Modulate surface, unlike a decal, works there).
- **Texture animation** (`F_TEXTURE_ANIMATION`, de_nuke's caustics: 60 cells of an 8 × 8 atlas, 0.05 s each): a
  flipbook in the same material (`MOD2X_FLIPBOOK_HLSL`), cells row by row from the top left, in the sequential mode.
- **Opaque and alpha-tested overlays keep Interchange's lit materials** (`MI_Default_Opaque`, `MI_Default_Mask`):
  the exporter lifts overlays 0.1 inch off their surface, which wins the depth test, and opaque or masked surfaces
  are lit per pixel like the wall they lie on (the trouble of decision 009 was translucency's lighting only).

Result on de_nuke, re-exported and re-imported with this change (renders in `test-data/shots/overlay_blend/`, local
only). CS2 shots: the 12 interior and yard views of the view-check session (`test-data/shots/view_checks/cs2/`, its
`tools/compare/views/de_nuke.json` on branch `claude/map-view-checks`, e.g. `asite` = `setpos 916 -1406 -450;
setang -3 119.5 0`, `ramproom` = `636 -133 -415; 5 45 0`, `lobby` = `-274 -1096 -408; 3 24 0`); "before" = that
session's renders of the level as imported before this change. Measured with the new
`tools/compare/mark_contrast.py`: the darkest share of a box (the mark) against the box's median (the floor around
it), linear luminance, so the light cancels out:

| Mark (view, box) | What the formula says | CS2 | before | after |
|---|---|---|---|---|
| dark square of a floor marking (`ramproom`, `0.600,0.618,0.633,0.640`, 20 %) | texel 60: × 0.47 | 0.44 | 0.12 | 0.42 |
| floor arrow (`asite`, `0.36,0.685,0.50,0.745`, 12 %) | white × tint 767676 at alpha 0.76: × 0.51 | 0.47 | 0.25 | 0.47 |
| straight arrow (`lobby`, `0.04,0.66,0.20,0.78`, 10 %) | as above | 0.47 | 0.20 | 0.45 |
| curved arrow (`lobby`, `0.17,0.80,0.30,0.97`, 10 %) | as above | 0.39 | 0.11 | 0.40 |
| arrow by the door (`lobby`, `0.67,0.78,0.76,0.97`, 10 %) | as above | 0.35 | 0.16 | 0.37 |
| light band of a floor marking (`ramproom`, `0.584,0.614,0.608,0.632`, brightest 20 %) | texel ~190: × 1.49 | 1.24 | 1.36 | 1.24 |

- **Every mark within 0.03 of CS2**, from 0.11–0.25 before (Interchange's translucent material drew the arrows'
  0.18 tint as a flat blue-black paint, `crop_lobby_arrow_cs2_before_after.png`). The floor markings' dark lines,
  light bands and corner squares look like CS2's; before, their lines were black and the light bands barely showed.
- **The grey veils are gone**: B site's reactor has its red band and "B1" lettering clean again (a scuff quad
  washed them grey), the garage floor under a scuff quad (`scuffs_overlay_001`, draw alpha 0.33) is no longer ≈ 13 %
  darker than around it, and only the scuffs' marks show.
- Pixels changed per view (more than 8 sRGB steps): ramp room 5.0 %, garage 5.9 %, tunnels 6.1 %, lobby 2.6 %, A
  site 2.0 %, yard 1.1 %, decontamination rooms, secret and observation room ≤ 0.3 %. B site's 8.8 % is mostly its
  blocky sun patches, which change from one render to the next (virtual shadow map page pool overflow on de_nuke,
  found by the view-check session), not overlays. Whole views against CS2 (`compare_shots.py`) moved by at most 3 %:
  the interiors' remaining gaps (A site 0.22, lobby 0.45 of CS2's luminance) are lighting.
- **Opaque bombsite signs** (the lobby's "A", the ramp room's "B"): no depth fighting with their walls; lit per
  pixel, the lobby's "A" is brighter and shows its white border as in CS2 (dimmer before, as translucent surfaces
  are in Unreal, decision 009).
- 25 other de_nuke materials changed cutoff (railing cards 0.225, hanging wires 0.089, tree cards 0.42–0.9, roof fans
  0.4); not compared with CS2 one by one.

## Alternatives considered

- **A mesh decal modulating the base colour**, like decision 009's overlay decals: Unreal turns a Modulate decal
  into a translucent one when DBuffer decals are on, and the DBuffer can only darken by one grey value (see
  Context): the brightening half of every Mod2x texture and the tinted marks would be lost. It would multiply the
  albedo before lighting rather than the lit picture; for an unlit multiply of a lit surface that is the same apart
  from specular highlights, which CS2 multiplies too.
- **A translucent darkening approximation** (black at opacity `1 − m` where `m < 1`), which a plain glTF reader could
  show: needs a baked texture per material and still cannot brighten.
- **Keeping the overlays out** (not exporting Mod2x): de_nuke loses its floor markings, arrows and wear.

## Consequences

- The grey veils are gone; Mod2x marks darken or brighten the floor under them as in CS2 (numbers above).
- What is still not CS2's: the order among overlapping translucent surfaces (Unreal sorts its translucency by
  distance, CS2 draws overlays in `m_nOverlayRenderOrder`); fog fades the multiplier towards 1 (CS2's fog handling
  of Mod2x is unknown); the colour textures are re-compressed by Unreal (BC3, a step or two around 128); the
  flipbook's mipmaps bleed between neighbouring cells; the random and scripted animation modes play sequentially.
- Other glTF readers still draw Mod2x overlays as a plain blend (grey veils); `extras.ofast.ints.F_BLEND_MODE`
  tells them which.
- Alpha-tested foliage, railings and wires are cut where CS2 cuts them; distant mip levels may still thin out
  (Unreal's mipmaps do not preserve alpha coverage as CS2's `PreserveCoverage` channels do).
- Additive, Multiply and ModThenAdd static overlays and `csgo_unlitgeneric`'s blend modes are still plain `BLEND`;
  none is on the six exported maps apart from one `csgo_unlitgeneric` on de_dust2 (mode 4) and de_mirage (mode 1).
