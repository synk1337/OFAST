# 010 — Translucent csgo_complex decals become mesh decals; other translucent surfaces are lit per pixel

- **Date:** 2026-10-17
- **Status:** accepted

## Context

Decision 009 found why de_nuke's road paint was faint in direct sun: every glTF `BLEND` material becomes an instance
of Interchange's `MI_Default_Blend` (`MI_Default_Blend_DS` when double-sided), whose parent
`/InterchangeAssets/gltf/M_Default` has the translucency lighting mode **Surface TranslucencyVolume** (`TLM_SURFACE`).
Unreal's own description of that mode: the light "is accumulated in a volume so the result is blurry", and only
diffuse lighting is supported (`ETranslucencyLightingMode` in `EngineTypes.h`). Overlays became mesh decals there;
every other translucent surface kept that lighting:

| Map | Translucent materials that are not overlays (world placements; 3D skybox) |
|---|---|
| de_nuke | `chainlink_fence_001_card` (9; 7), `nuke_drain_covers_color` (15), `generic_swat_cards_color_tintd4ab0b` (1) |
| de_inferno | `inferno_trim_decal_01` (67), `trike_glass` (11), `cargovan_glass` (3), `inferno_trim_decal_02` (3), `inferno_decal_01` (1) |
| de_dust2 | `dust_garbage_dumpster_signage` (6), `dust_electric_panel_cover_color` (5); `palm_treecard_01` (skybox only, 41) |
| de_mirage | `4carz1024_glass` (1; 1), `metalfence004a` (2), `urban_puddle01a` (1) |
| de_vertigo | `sign_number_0` (4), `assault_hide01` (1), `helicopter_movingblades` (1) |
| de_anubis | `water_falling02` (3); `palm_treecard_01` (skybox only, 45) |

(`csgo_complex`, `csgo_vertexlitgeneric` and non-overlay `csgo_lightmappedgeneric` with `F_TRANSLUCENT`, from the
exported glTF files. Static overlays in their other blend modes and the `csgo_unlitgeneric` cards are not counted,
see Consequences.)

`inferno_trim_decal_01` was lit worse still: it has a tint mask, so it got an instance of OFAST's
`M_OFAST_TintMask_v1` (decision 007), a material made by script whose translucency lighting mode was Unreal's
default for a new material, **Volumetric NonDirectional** (`UMaterial`'s constructor), which ignores the normal map.
Its meshes are strips of plaster damage, leaks and cracks laid over the house fronts all over the map.

Two kinds of material are in the table. Glass, fence and foliage cards and signs stand on their own. The
`csgo_complex` materials with **`F_DEPTH_BIAS`** (de_inferno's trim decals and `inferno_decal_01`, de_nuke's drain
covers and the SWAT van's lettering, de_vertigo's `sign_number_0`) are models lying on a wall or floor: their
placements are Hammer's mesh overlays (`…_mesh_overlayNN` in the world nodes), and the flag pulls them in front of
the surface they lie on. None of the glass or cards has the flag.

## Measurements

What-ifs on the imported levels with `shoot_views.py -Try` (nothing saved), in the shared Unreal project:

- **before**: the levels as imported (`TLM_SURFACE`; Volumetric NonDirectional for the trim decal);
- **per pixel**: `"materials"` set `TLM_SURFACE_PER_PIXEL_LIGHTING` (**Surface ForwardShading**) on
  `/InterchangeAssets/gltf/M_Default` and `M_OFAST_TintMask_v1`;
- **decal**: per pixel, plus a `"script"` giving every `F_DEPTH_BIAS` slot of the world a dynamic instance of the
  overlay decal master of decision 009 with the textures and factors of the material it replaces;
- **base colour**: `"console": ["ShowFlag.Lighting 0"]`, to divide the lighting out.

The **light factor** of a set of pixels is its linear luminance lit / in base colour. For a translucent surface and
the opaque surface around it, the ratio of the two factors is 1.00 when both get the same light: what CS2 does for
a decal, which shares the lightmap texel of the surface it lies on (`tools/compare/light_factor.py`, which finds
the surface's pixels as those two renders disagree on, inside a box).

| View (`tools/compare/views`), box | Measured | before | per pixel | decal | CS2 |
|---|---|---|---|---|---|
| de_nuke `ctvan` (sun), `0.25,0.3,0.6,0.75` | SWAT lettering / van, light factor ratio | 0.72 | 0.88 | 0.88 | — |
| de_inferno `banana` (shade), `0.70,0,0.98,0.88` | plaster patch / wall face, light factor ratio | 0.47 | 0.62 | 0.80 | ≈ 0.88 |
| the same | plaster patch / wall face, luminance ratio (base colour 1.38) | 0.65 | 0.85 | 1.11 | 1.22 |
| de_dust2 `dumpster` (sun) | the container's sign, luminance (per pixel = 1) | 0.70 | 1.00 | — | — |

(The van box also holds the van's shaded back, hence 0.88 rather than 1 for both: the columns compare. CS2's light
factor ratio is its luminance ratio over Unreal's base-colour ratio, 1.22 / 1.38, assuming the same albedos; the
patch's own normal map makes it less than 1 there too.) The banana
view is the view-checks session's (CS2 shot `test-data/shots/view_checks/cs2/cs2_de_inferno_banana.jpg`): CS2 shows
the patch as light plaster, brighter than the wall. Before, Unreal drew it as a dark grey streak; per pixel it is
blue-grey, still darker than the wall; as a decal it is light plaster as in CS2.

- **In the sun the two fixes are equal**: per pixel, the direct sun comes with the exact shadow, like an opaque
  surface's. The SWAT lettering went from a dull ochre to the van's bright yellow; the substation's fence cards
  and de_dust2's container sign gained as much.
- **In the shade only the decal is right**: lit per pixel, a translucent surface still takes its indirect light
  from Lumen's translucency volume, a coarse grid, darker than the screen and surface-cache lighting of the wall
  under it (decision 009 saw the same on the road paint in the shade). A decal is lit with the wall itself.
- Glass changes little in the views checked (de_inferno's trike: 1.8 % of the frame changed, 0.92 → 1.00 there):
  it is mostly see-through. Per pixel it also gets specular highlights, which Surface TranslucencyVolume cannot give.
- **Static overlays in their other blend modes** (still exported as plain `BLEND`, decision 009) are lit by
  neither mode right. de_dust2's Mod2x overlay on the T spawn's sunlit upper wall (`dust_mudbrick_transition_overlay`)
  got brighter per pixel, closer to CS2 (0.64 → 0.75 of CS2's luminance there); de_nuke's Mod2x scuff on a shaded
  pillar at the CT spawn (`scuffs_overlay_001_*`) got darker (0.86 → 0.77 of CS2), where CS2 barely changes the
  pillar: Mod2x multiplies by twice the texture and is unlit, which no lighting mode gives.

**Run-to-run noise** (repeating a run): de_nuke's views repeat within 2–4 % of their pixels changing by more than 8
sRGB steps. The first screenshot of a run can be taken before Lumen has settled (de_inferno's banana changed over 86
% of the frame between two runs as the first shot), and de_inferno's library, an interior lit only by bounce light,
varied by ±5 sRGB steps over a third of the frame between runs. The de_nuke and de_inferno rows above use a second
round of runs that start with a throwaway shot, and leave the library out; the de_dust2 sign changed only where it
is (0.16 % of the frame). The throwaway shot must not use the same camera as the next one: with the same camera the
next screenshot was taken at once (0 s) and showed the view after it.

## Decision

- **`csgo_complex` with `F_TRANSLUCENT` and `F_DEPTH_BIAS` is a decal overlay** (`is_decal_overlay` in
  `import_map.py`), drawn as a mesh decal like the overlays of decision 009, on the world's placements.
  `M_OFAST_OverlayDecal_v1` becomes **`M_OFAST_OverlayDecal_v2`**, which also applies a tint mask
  (`TintMaskTexture`, `ModelTint`, `TintMaskUV`: colour × lerp(1, tint, mask.R), the tint-mask master's rule) for
  `inferno_trim_decal_01` (tint 0.94 / 0.90 / 0.84 under its mask). `tint_mask_instances` leaves decal overlays to
  `overlay_instances`; both would otherwise write the same `MI_<name>`.
- **Every other translucent surface is lit with Surface ForwardShading** (`TRANSLUCENCY_LIGHTING`):
  - Interchange's instances: OFAST copies Interchange's master once, as **`/Game/OFAST/Materials/M_OFAST_glTFDefault_v1`**
    (a duplicate of `M_Default` with the per-pixel mode), and its two translucent instances as
    **`MI_OFAST_glTFDefault_Blend_v1`** and **`MI_OFAST_glTFDefault_Blend_DS_v1`** (same blend-mode and sidedness
    overrides, parented to the copy). After the scene import, `light_translucent_per_pixel` re-parents the
    Interchange instance of every `BLEND` material that `lit_per_pixel` names (not a decal overlay, not a static
    overlay in another blend mode, not an unlit `csgo_unlitgeneric` card, and with no OFAST instance) from
    `MI_Default_Blend(_DS)` to the copy, and gives any other back to Interchange's parent. The copies have the same
    parameters, so the instances keep their textures and factors. Idempotent: an instance with the right parent is
    left alone. The plugin's content is not edited.
  - OFAST's masters whose instances can be translucent (`M_OFAST_Environment_v1`, `M_OFAST_TintMask_v1`) get the
    mode on creation, and in place when an older OFAST made them (`ensure_translucency_lighting`): it is a property,
    not part of their graph, and opaque or masked instances look the same with any mode, so no new version.
- **Nothing changes in the exporter.** The glTF stays `BLEND`; other glTF readers keep their own lighting.

Result of the imports with this change (renders in `test-data/shots/translucent_lighting/import/`, local only):

| Map | Mesh decals: materials (tint mask) / placement slots | Lit per pixel | Left as they were | Spawns vs CS2 (`compare_shots.py`) |
|---|---|---|---|---|
| de_nuke | 19 (0) / 109; decision 009: 17 / 93 | 2 (the fence cards, world and skybox) | 30 static overlays | CT 0.990, T 0.954 (decision 009: 0.990, 0.955) |
| de_inferno | 16 (1) / 213 | 2 (trike and van glass) | 7 static overlays | CT 0.879 (decision 007's import: 0.850), T 1.51 (1.50) |
| de_dust2 | 12 (0) / 55 | 5 (container sign, panel covers, 3 skybox palm cards) | 1 static overlay, the skybox clouds | T 0.964 |

- A second import of de_nuke and de_inferno changed nothing (`0 re-parented, 2 already were`), and the imported
  views match the what-ifs (de_nuke `ctvan`: 2.7 % of the pixels differ by more than 8 sRGB steps, the noise
  level; de_dust2 `dumpster`: 0.4 %).
- **de_inferno banana** as imported: the plaster patch is light and warm as in CS2, patch / wall 1.05 (CS2 1.22;
  the what-if's 1.11 was without the trim decal's warm tint, which the decal master now applies; the light factor
  ratio reads 0.76 against the base-colour render made before, which has no tint either).
- **de_inferno CT spawn**: the first import with decisions 009 and 010; 10 % of the frame changed (the painted
  "SINI" advert and the "A" overlays, the plaster patches on the house corners), those pixels from 0.56 to 0.83 of
  CS2's luminance.
- de_nuke's CT view still differs from decision 009's render on a shaded silo pillar (4 % of the frame); the pillar
  has no translucent surface, and its luminance varied between 0.083 and 0.098 across this session's renders before
  any change was imported (bounce light settling, other sessions' imports in between). de_dust2's CT spawn (0.11 of
  CS2) is its known dark spawn room (decision 007).

## Alternatives considered

- **Surface ForwardShading for the decals too** (the first plan): right in the sun, but in the shade the coarse
  indirect light of Lumen's translucency volume leaves them darker than their wall (banana: 0.62 against the decal's
  0.80 and CS2's ≈ 0.88), the problem decision 009 solved for overlays.
- **Changing `/InterchangeAssets/gltf/M_Default` itself**: one property, but it edits the engine plugin's content
  (lost on an engine update, shared by every project) and recompiles every Interchange material, opaque ones
  included.
- **An OFAST translucent master built by script** (like the tint-mask master): would need every feature of
  Interchange's material the exporter uses and the textures imported again; the copy keeps Interchange's graph and
  the instances' parameters as they are.
- **Surface TranslucencyVolume with more light** (a brighter colour or emissive): would fit one lighting only.

## Consequences

- The copy of `M_Default` is made once: a newer engine's `M_Default` reaches OFAST only through a new version
  (`GLTF_MASTER` `_v2`).
- **Static overlays in their other blend modes** (`F_BLEND_MODE` 0, 2, 3 …, decision 009) are still exported as
  plain `BLEND` and keep Interchange's `TLM_SURFACE` until they get their own handling (another session's work):
  per pixel helped them in the sun and hurt them in the shade (Measurements). The **`csgo_unlitgeneric` cards**
  (de_mirage's `wrongway_timer`, de_dust2's skybox clouds) are exported without `KHR_materials_unlit`, so
  Interchange gives them its lit master; they keep it too (they should be unlit).
- The 3D skybox keeps translucent materials (decision 009): its palm and fence cards are lit per pixel now; a decal
  overlay in a skybox would keep Interchange's `TLM_SURFACE` (none on the six maps).
- DBuffer decals cannot write ambient occlusion: the decals drop their occlusion texture (de_nuke's drain covers
  have one of their own; the SWAT lettering and the overlays of decision 009 use `default_ao`).
- New views for these checks: de_nuke `ctvan`, de_dust2 `dumpster`, de_inferno `trike` (`tools/compare/views`, CS2
  cfgs in `tools/cs2/cfg`). No CS2 shot of them yet; the CS2 comparison above uses the view-checks session's banana.
