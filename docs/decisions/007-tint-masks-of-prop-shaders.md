# 007 — Tint masks of prop shaders applied in Unreal's material, not as a glTF factor

- **Date:** 2026-10-17
- **Status:** accepted

## Context

A placement's tint colour (`rendercolor`) was written as a whole-surface glTF `baseColorFactor` for every material
that is not an environment one. CS2 does that only without a tint mask. `csgo_complex`, `csgo_vertexlitgeneric`,
`csgo_foliage` and `csgo_simple_3layer_parallax` materials with `F_TINT_MASK` tint only where `g_tTintMask` is set.
de_inferno's T-spawn potted tree (`cress_plant_kit_01_tintff0000`, `csgo_foliage`) shows it best: CS2 draws green
leaves and red flowers (the mask covers only the atlas's white flowers), while Unreal drew the whole tree red-brown.
Decision 006's rebuild of stale materials did not change it: the export itself was wrong. Such materials are common:
622 tint-masked materials over the 24 official maps (`shader_census`), and as tinted placements de_dust2 has 264
(doors, window frames, crates), de_nuke 198, de_inferno 80, de_anubis 76.

How the shaders tint, from Valve's shaders as decompiled in GameTracking-CS2 (`<shader>.slang`):

- the tint per placement, in the vertex shader of all four: `lerp(1, rendercolor, g_flModelTintAmount) ×
  g_vColorTint`, sRGB → linear. `g_flModelTintAmount` 0 (120 `csgo_complex` materials, mostly de_nuke's) means
  "ignore placement tints", and `csgo_vertexlitgeneric`'s `F_NOTINT` turns tinting off. OFAST ignored both and tinted
  those materials anyway;
- without a mask, `albedo × tint`;
- `g_tTintMask` is read in R (BC4), white by default, on the second UV set with `g_bUseSecondaryUvForTintMask`. It
  has no contrast or brightness of its own;
- with a mask, `albedo × lerp(1, tint, mask)`: a plain multiply. It is **not** the environment shaders'
  luminance-keeping tint (decision 005). This part is not in Valve's files: they decompile only each shader's
  default variant, which has no mask. It is Source 2 Viewer's reconstruction, and the form of
  `csgo_vertexlitgeneric`'s default `lerp(albedo, albedo × tint, saturate(a + g_flInverseBlendTint))` with
  `g_flInverseBlendTint` set to 0 by `F_TINT_MASK`. The CS2 comparison below agrees.

## Decision

- **The exporter computes the tint as CS2 does** for those four shaders (`crates/ofast-core/src/export/tint.rs`,
  with tests). Without a mask it stays the `baseColorFactor`, now with `g_flModelTintAmount`, `g_vColorTint` and
  `F_NOTINT` taken into account. Other shaders keep a plain multiply by the placement tint: their code is not checked.
- **With a mask, the tint travels in `extras.ofast.tint_mask`** (tint, mask PNG, UV set, `baked`). The colour texture
  stays untinted and shared by every tint, and the factor stays white.
- **Unreal applies it in a new master, `M_OFAST_TintMask_v1`.** It has glTF's metallic-roughness inputs as
  Interchange's glTF material uses them, with the colour × lerp(1, `ModelTint`, mask.R), and the mask on UV 0 or 1.
  `import_map.py` gives each such material an instance, set up before the scene import and swapped onto the meshes
  like the environment ones. Untinted placements keep Interchange's material.
- **`ofast export-map --bake-model-tints` bakes the masked tint** into a colour texture per tint
  (`<texture>_tint_<hash>.png`) for other glTF readers. A mask on the second UV set cannot be baked (another layout)
  and is left to Unreal either way.

Result on de_inferno (`test-data/shots/tint_mask/`, local only):

- **The export.** 87 materials get `extras.ofast.tint_mask`: 86 tinted placements (44 foliage, 36 `csgo_complex`,
  6 parallax windows) and one untinted decal with a `g_vColorTint`. The masks add 14 PNGs (3 MB). The export has
  1111 texture files (2.5 GB) and was written in 27 s.
- **The T-spawn tree** has green leaves and red flowers, like CS2. Of the saturated pixels in the tree crop
  (`inferno_t0_tree_cs2_before_after.jpg`), 24 % are red (CS2 23 %, before 93 %) and 61 % green (CS2 44 %,
  before 0 %). The second potted tree is green, as in CS2.
- **The CT spawn's flower boxes and wall pots** have green leaves with red flowers instead of red leaves
  (`inferno_ct0_planters_cs2_before_after.jpg`).
- **Nothing else moved.** Against the previous renders, 0.40 % of the CT view's pixels changed (only the plants),
  4.4 % of the T view's and 0.03 % of the top view's. Luminance against CS2 (`compare_shots.py`): CT 0.850
  (before 0.851), T 1.50 (before 1.48, from the greener tree; the gap itself is the tone level, not materials).

Found on the way: the first editor runs after an import rendered the CT view with its sunlit walls in shadow and
blurry textures. A later run of the same level was correct, so this was not caused by the change. Before trusting
a render that differs, shoot the view again with `shoot_views.py`.

### Checked on de_dust2 and de_nuke (2026-10-17)

The two maps that use the rule most, re-exported (their exports predated decision 005's format) and imported again
(`test-data/shots/tint_mask_maps/`, local only; "before" = the previous renders in `fresh_materials/`):

- **The exports.** de_dust2: 263 materials with `extras.ofast.tint_mask` (260 `csgo_vertexlitgeneric`, 2 foliage,
  1 `csgo_complex`), and 21 `<name>_tint…` materials (placement tints) of `F_NOTINT` materials that are now
  untinted. de_nuke: 200 (199 `csgo_complex`), and 57 `_tint` materials now untinted (50 `csgo_complex` with
  `g_flModelTintAmount` 0: ventilation exhausts, web joist bases, railings, ladders; 7 `F_NOTINT`). No
  mask on the second UV set on either map.
- **de_dust2's T spawn**, against CS2's screenshot. The kerb's white segments are `dust_curbs_001_painted_notint`
  (`F_NOTINT`): OFAST tinted them pink, mean sRGB (214, 154, 143); now (255, 250, 236), CS2 (252, 241, 229). The red
  segments (masked, tint d8d2c7) went from (212, 154, 141) to (217, 167, 153), CS2 (213, 166, 151). 1.8 % of the
  view's pixels changed, the CT view 0.2 %, the overview 0.4 %. The T spawn's green door (`tdoor` view, masked
  a5e9b9) renders green behind its grille; there is no CS2 shot of that view yet (below).
- **de_nuke's T spawn** (`summary_cs2_before_now.jpg`). The blue door (`metal_door_001`, masked, tint 7ea7bc) has
  a light kick plate again, as in CS2 (the factor had painted it blue), and CS2's hue (203°, CS2 202°, before 210°).
  The orange-yellow bin's sticker (masked, edb33f) is white and blue as in CS2 instead of yellow, and the bin's hue
  moved towards CS2's (45°, CS2 52°, before 41°). Door and bin stay less saturated than CS2 (door 0.23, CS2 0.37).
  That gap is not the mask: the bollards next to them are tinted with the same edb33f by a plain factor (no mask,
  untouched by this decision) and show the same gap (43°, CS2 49°), and the shaded concrete wall, untinted, is
  bluish in CS2 (saturation 0.15) and neutral in Unreal (0.05). Candidates are the sky light's colour in the shade
  and CS2's colour-correction LUT (slightly more saturated, not applied in Unreal, [lighting](../formats/lighting.md)).
- **de_nuke's CT spawn.** The blue sign (`hr_area_signs_decal_001`, masked 7ea7bc): mean sRGB (137, 162, 174), CS2
  (130, 165, 181), before (108, 134, 150). The red crate (`metal_crate_002b`, masked b74d3c) moved towards CS2 too.
- **Brightness against CS2** (`compare_shots.py`): de_nuke CT 0.97 (before 0.78), T 0.96 (before 0.86), mostly from
  the re-export (the height blends no longer fall back to red asphalt); de_dust2 T 0.96; de_dust2 CT 0.11, its known
  dark spawn room (decision 006's notes).
- **de_anubis** was re-exported and imported too, for its skybox boat: 76 masked materials; only tinted doors and
  window frames changed (0.8 % and 1.5 % of the spawn views' pixels), brightness against CS2 unchanged (1.37 and
  1.32, the tone level of the Hable session).

Not a tint mask, found on the way: de_nuke's yellow road paint (parking bays, "no parking" lines) is white in
Unreal, before and after. The overlays are `csgo_lightmappedgeneric` (`hr_nuke_parkingstripe_single`,
`hr_nuke_noparking_tiled`) whose colour is white: the texture is translucency only with a white
`TextureLayer1Color`, and the scene object's `m_vTintColor`, `g_vColorTint` and `g_vLayer1Tint` are white. The meshes
have no vertex colour, and the asphalt under them is grey. Where CS2 takes the yellow from is open.

Views of tinted props were added, chosen from the export's placements and tried with `shoot_views.py -Views`
(no re-import needed): de_dust2 `tdoor`, `tbluedoor`, `tcar`, `tsalmon` (doors, frames, awnings, window bars, the
taxi, bins), de_nuke `ctcrates` and `tdoors` (`tools/compare/views/<map>.json`, and `tools/cs2/cfg/ofast_v_<map>_<view>.cfg`
for CS2). Their CS2 screenshots were not taken: driving CS2 was declined for this session. The Unreal renders are
in the same folder.

## Alternatives considered

- **Bake the masked tint into the colour by default.** Exact for every glTF reader, but one colour texture per
  (texture, tint): de_dust2 261 over 44 prop textures (670 MB of PNGs), de_nuke 190 over 61 (251 MB), de_inferno
  85 over 14. Decision 005 moved environment tints out of the textures for the same reason. Unreal would also import
  each copy.
- **Keep the factor, or use the mask's average as a weaker factor.** Wrong where it matters: the mask separates the
  painted parts (flowers, door paint) from the rest, and no single factor can.
- **Reuse `M_OFAST_Environment_v1` / `MF_OFAST_ModelTint_v1`.** That tint keeps luminance and reads the mask from a
  height texture's G with a contrast and brightness. The prop shaders multiply, read R, and their surface textures
  differ (separate occlusion, factors). A master of their own keeps both formulas simple.
- **Reference Interchange's imported textures** instead of importing the PNGs again into `Blend`. That saves the
  duplicate copies, but these instances are set up before the scene import, while no mesh uses them (decision 005:
  parameter writes on used instances took minutes and crashed the GPU). It would also tie `import_map.py` to
  Interchange's asset names. Same choice as the environment masters.

## Consequences

- Tinted props with a tint mask depend on `import_map.py`, like environment materials. Imported without it (plain
  Interchange), they are untinted.
- Untinted placements of a masked material with a non-white `g_vColorTint` also get the master (de_inferno has
  some decals).
- Materials with `g_flModelTintAmount` 0 or `F_NOTINT` lose the tint they wrongly had: de_nuke's crates and signs,
  de_dust2's `F_NOTINT` props. Their placements' colours were never CS2's.
- The master's second-UV path (`TintMaskUV` 1) is not checked in a render: no tinted placement on the six maps
  exported so far uses a mask on the second UV set.
- The 3D-skybox master applies tint masks since `M_OFAST_Skybox_v4` (a `MaskedTint` parameter of its own, white
  without a mask). v3 showed the three masked skybox materials (de_anubis, de_dust2, de_nuke) untinted. All three
  were read back after their maps' re-import (parent v4, tint and mask set); they are too small or far away to
  check in a render.
- Not reproduced: `csgo_foliage`'s colour correction (neutral on every map exported so far) and height gradient
  (vertex colour R brightening: de_inferno's downey willow bushes), the vertex-colour occlusion, and the tint rules
  of the shaders not checked.
