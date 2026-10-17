# 008 — A draw call's own tint is part of its placement tint

- **Date:** 2026-10-17
- **Status:** accepted

## Context

de_nuke's road paint (the parking bays outside CT spawn, the "no parking" lines along the wall, the "NO-IDLE ZONE"
text) is yellow in CS2 and was white in Unreal, before and after decision 007. Decision 007's check ruled out the
material (`hr_nuke_parkingstripe_single`, `hr_nuke_noparking_tiled` and their corner and text variants:
`csgo_lightmappedgeneric` overlays with `F_OVERLAY` + `F_TRANSLUCENT` whose `g_tColor` is compiled from a
translucency PSD only, `TextureLayer1Color`, `g_vColorTint` and `g_vLayer1Tint` white), the scene objects that place
them (`m_vTintColor` 1, 1, 1, 1 in `maps/de_nuke/worldnodes/n0.vwnod_c`), the vertex streams (position, UV,
lightmap UV, normal: no colour) and the asphalt underneath (grey).

The colour is one level further down. The overlay model's `MDAT` (`n0_lr0_c202_s_mesh_overlay202.vmdl_c`, dumped
with `resource_dump`) gives each draw call an `m_vTintColor` of its own, (0.851, 0.459, 0.046), and an `m_flAlpha`.
OFAST's model reader never read either. The material says why it is there: its searchable user data has
`VertexPaintUIPickColor = 1`, i.e. Hammer lets the mapper pick the colour of that face, and the compiler stores it
on the draw call. The material stays white so that one stripe texture serves every colour.

What the values are, from a census of every draw call placed on nine maps (`examples/drawcall_tints.rs`):

- **Linear floats of Hammer's colour bytes, gamma 2.2.** Raised to 1/2.2, every value found is a whole sRGB byte
  (road paint 237.0, 179.0, 63.0; floor arrows 118.0; skybox buildings 176, 175, 164 …). Through the exact sRGB curve
  they are not (237.5, 180.5, 60.7).
- **They reach the shader as the instance tint.** CS2's vertex shaders (decompiled in GameTracking-CS2;
  `csgo_complex`, `csgo_static_overlay`) read one packed 8-bit sRGB colour per instance, decode it to linear, apply
  `lerp(1, tint, g_flModelTintAmount) × g_vColorTint` and multiply the opacity by its alpha. A draw call's colour
  can only reach them there, so it obeys the same rules as a placement tint.
- **Common.** Overlays with a tinted draw call: de_train 578 of 707, de_overpass 148 of 384, de_nuke 145 of 237,
  de_anubis 75 of 135, de_vertigo 44 of 92, de_inferno 32 of 263, de_dust2 14 of 56, de_ancient 9 of 25, de_mirage
  8 of 301; other scene objects 2 to 72 per map. Some are opacity only (de_nuke's 35 scuff decals, alpha 0.12–0.73),
  most are colours (grey floor arrows, red stripes, pipe and silo trims, the 3D skybox's tinted buildings).
- **Never combined with a placement tint.** Every tinted draw call on the nine maps is placed with a white tint, and
  no aggregate fragment uses one (fragments carry Hammer's colour in their own `m_vTintColor`).

## Decision

- **`DrawCall` keeps `tint` (linear RGB) and `alpha`** (`crates/ofast-core/src/model/mod.rs`, white and 1 when
  absent).
- **The glTF writer folds them into the placement tint** before choosing the material
  (`DrawCall::placement_tint`, used by `GltfWriter::mesh`): the draw call's colour is encoded back to sRGB with
  gamma 2.2 and multiplies the placement's sRGB tint channel by channel; its alpha multiplies the placement's alpha.
  Everything after that is unchanged: the product is a placement tint like any other, so a `baseColorFactor` for
  most shaders, a tint-mask instance for masked prop shaders (decision 007), the environment master's `ModelTint`
  (decision 005), and nothing for `g_flModelTintAmount` 0 or `F_NOTINT` materials, as in CS2.
- **A variant with less than full alpha is named `<name>_tintRRGGBBAA`** (the alpha byte added), so that variants
  differing only in opacity do not share a name in Unreal.
- **No change in `import_map.py`**: Interchange's glTF materials, the tint-mask master and the 3D-skybox master all
  read the factor or the tint already.

Result on de_nuke, re-exported and imported with this change (`test-data/shots/drawcall_tints/`, local only;
"before" = decision 007's renders in `tint_mask_maps/`; colours measured on the most yellow 5 % of a region's
pixels):

- **The export.** 652 glTF materials (before 602). The road paint is 9 materials `<name>_tintedb33f` (sRGB 237,
  179, 63) on 66 overlays. Also new: the grey floor arrows (26 overlays, `_tint767676`), red stripes and an "X", 35
  scuff decals with their own alpha (`scuffs_overlay_001_tintffffff1e` … `bb`), caustics decals, the reactor dome's
  light grey (`nuke_silo_001b_tinte2e0e0`) and, in the 3D skybox, 11 tinted far buildings and 9 concrete walls.
  Materials with `g_flModelTintAmount` 0 (`airduct_hvac_001`, `hr_metal_railing_001_card`) got a variant name but
  no factor, as their rule says.
- **CT spawn, in the sun** (`crops_de_nuke_ct0_bays.png`, `crops_de_nuke_ct0_line.png`): the parking bays and the
  "no parking" line are yellow. Hue 36–37° (CS2 43–44°; before 57–60° at saturation 0.04, i.e. white), but paler
  than CS2's: saturation 0.23–0.29 (CS2 0.43–0.49). 7.2 % of the view's pixels changed (the paint and the reactor
  dome).
- **T spawn, in the shade** (`crops_de_nuke_t0_bays.png`): the parking bay is as strong as CS2's, paint
  (81, 71, 55) on asphalt (42, 54, 65), CS2 (89, 90, 54) on (38, 56, 68); before, white (83, 91, 97). CS2's paint
  leans lemon there because its shade is bluish, Unreal's neutral: the lighting gap decision 007 found on the same
  map. 2.0 % of the view's pixels changed, the overview 2.3 %, `tdoors` 2.5 % (`ctcrates` was framed differently
  in the earlier render and is not compared).
- **Brightness against CS2** (`compare_shots.py`): CT 0.952 (before 0.969), T 0.953 (before 0.959); yellow and the
  dimmer scuffs reflect less than white.

**Not caused by the tint, found on the way: overlay paint is faint in direct sun.** At the CT spawn the old white
lines were barely brighter than the sunlit asphalt (175 on 167), while the same kind of line in the shade at the T
spawn stood out clearly (83, 91, 97 on 43, 55, 67). The paint's texture is mostly 25–50 % opaque
(`hr_nuke_parkingstripe_single_trans`, BC7, alpha max 239, Valve's stored average 0.217, matching OFAST's decode),
and only one overlay lies on the asphalt at each paint pixel (`map_raycast` with `OFAST_LAYERS`), so the export
gives what the files say. Candidates: how Unreal lights translucent (`BLEND`) surfaces in direct sun (its
translucency lighting, not the opaque surface's), or a blend in CS2's overlay pass other than plain alpha (the
decompiled shader shows only the opaque variant; Source 2 Viewer's renderer uses plain alpha). Logged as a
follow-up.

## Alternatives considered

- **Multiply in linear space** (placement tint to linear × the draw call's float). Same result for every case found
  (the placement is always white); the sRGB product follows how the shader receives the instance tint (one sRGB
  byte colour). Revisit if a map ever combines the two.
- **Decode with the sRGB curve instead of gamma 2.2.** Within one byte either way (180.5 instead of 179); gamma 2.2
  is the one the stored values round-trip with exactly.
- **A vertex colour (`COLOR_0`) carrying the draw call's colour.** Would keep one material per CS2 material, but
  OFAST already uses `COLOR_0` for the two-layer materials' blend weights, and a placement tint is already a
  material variant: a second mechanism for the same thing would need its own path through every master. A tint
  is per draw call anyway, which is what a primitive's material is.
- **Leave the alpha out.** The scuff decals would stay at full strength (darker and more visible than in CS2),
  and the alpha costs nothing more than the colour.

## Consequences

- Every map gets a few dozen to a few hundred more material variants (de_nuke 602 → 652 glTF materials), each an
  Interchange material instance in Unreal.
- The first import after the re-export rebuilds all of the map's Interchange material instances (the glTF changed;
  decision 006).
- The order in which the engine combines a draw call's tint with a non-white placement tint is not checked: no map
  has that case.
- The environment shaders ignore a draw call's alpha (none of theirs has one below 1 on the nine maps).
- Maps exported before this change keep their white road paint until re-exported.
