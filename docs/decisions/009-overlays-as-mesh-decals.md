# 009 — Translucent overlays become mesh decals, lit with the surface under them

- **Date:** 2026-10-17
- **Status:** accepted

## Context

Decision 008 made de_nuke's road paint yellow, and found on the way that translucent overlays are faint in direct sun
in Unreal while they match CS2 in the shade: at the CT spawn the old white lines were barely brighter than the sunlit
asphalt (sRGB 175 on 167), and the yellow paint came out pale and brownish. The export was ruled out there: the
paint's colour texture is translucency only (RGB white, the stripe's alpha a flat 0.49, a few spots up to 0.94;
Valve's stored average 0.217 = OFAST's decode), only one overlay lies on the asphalt at each paint pixel, and the glTF
material is `BLEND` with the draw call's tint as `baseColorFactor`.

**How CS2 lights an overlay.** An overlay is a surface lying on another one (road paint, signs, stains):
`csgo_lightmappedgeneric` with `F_OVERLAY` (always with `F_TRANSLUCENT` on the six exported maps) and
`csgo_static_overlay`. It is lit like the surface under it: same lightmap texel, same sun shadow. Blending
α·lit(paint) + (1 − α)·lit(ground) is then the same as lighting α·paint + (1 − α)·ground once.

**How Unreal lit it.** Every glTF `BLEND` material is an instance of Interchange's `MI_Default_Blend`, whose parent
`/InterchangeAssets/gltf/M_Default` has the translucency lighting mode **Surface TranslucencyVolume**
(`TLM_SURFACE`, read with a probe script). A translucent surface in that mode takes its direct light from the
translucency lighting volume and its indirect light from Lumen's translucency volume: coarse 3D grids around the
camera, not the per-pixel light and shadow the opaque asphalt under it gets. The paint got less sun than the ground
it lies on, so the blend darkened it: the yellow became orange-brown and the white lines grey.

**Measured** on de_nuke's spawns with `shoot_views.py -Try` (what-ifs on the imported level, nothing saved) and
`tools/compare/overlay_contrast.py` (boxes `0.24,0.62,0.46,1.0` for the CT bays, left of the yellow curb,
`0.58,0.60,0.92,1.0` for the CT "no parking" line and text, `0.57,0.74,0.76,1.0` for the T bays). Paint = the most
saturated 5 % of yellow pixels in a box, asphalt = the median of its least saturated half; the ratio is their linear
luminance (paint / asphalt), so the light cancels out when both get the same:

| | CT spawn, sun: bays / line | T spawn, shade: bays |
|---|---|---|
| CS2 (reference shots `cs2_de_nuke_ct0/t0.jpg`) | 2.35 / 2.07, paint (249, 213, 125) | 2.29, paint (88, 89, 53), hue 62° |
| translucent, `TLM_SURFACE` (before, decision 008's import) | 0.97 / 0.95, paint (178, 157, 126) | 1.62, hue 38° |
| translucent, Surface ForwardShading (what-if) | 1.95 / 1.78 | 1.50, hue 40° |
| mesh decal (what-if on the paint only) | 2.09 / 1.77 | 1.87, hue 58° |
| mesh decal (this decision, imported) | 2.07 / 1.86, paint (254, 221, 144) | 1.86, hue 57° |

Per-pixel forward shading (`TLM_SURFACE_PER_PIXEL_LIGHTING` on `M_Default`) fixes the direct sun but not the
indirect light, which still comes from Lumen's translucency volume: the shade got slightly worse (exact shadows, no
more sunlight leaking in from the volume's coarse cells) and stays orange-brown. The mesh decal takes the light of
the asphalt itself: the shade's hue now matches CS2's lemon (CS2's shade is bluish, Unreal's lighting gap of
decision 007), and both spawns gained. The imported line also covers the "NO PARKING" text, a decal now as well.

## Decision

- **Overlays blended with plain alpha become mesh decals** (`import_map.py`: `is_decal_overlay`,
  `overlay_instances`, `put_overlay_decals`): `csgo_lightmappedgeneric` with `F_OVERLAY` + `F_TRANSLUCENT`, and lit
  `csgo_static_overlay` in its Translucent blend mode (`F_BLEND_MODE` 1). Each gets an instance of a new master,
  `M_OFAST_OverlayDecal_v1`: Deferred Decal domain, translucent blend, glTF's metallic-roughness inputs as
  Interchange's material uses them (colour × `BaseColorFactor`, opacity = colour alpha × the factor's alpha,
  normal, roughness and metalness with their factors). A material in the decal domain on a static mesh is drawn as
  a mesh decal: it blends colour, normal and roughness into the GBuffer (DBuffer decals) over whatever lies under it,
  before lighting, which is CS2's arrangement.
- **On the placements, not the meshes**: component overrides on the world's actors. The 3D skybox's copies keep the
  translucent material, since a decal there would be depth-tested at its true position, not behind the world like
  the rest of the skybox (decision of `setup_skybox`).
- **Nothing changes in the exporter.** The glTF stays as glTF describes it (`BLEND`), and other glTF readers keep
  the translucent material.

Result on de_nuke, re-imported with this change (renders in `test-data/shots/overlay_decals/`, local only; "before"
= decision 008's renders in `drawcall_tints/`):

- **17 overlay materials** became decal instances, on **93 placement slots** (no overlay in de_nuke's 3D skybox).
- **CT spawn, sun:** the paint as in the table, and the "NO PARKING" text on the road is readable as in CS2 (faint
  before). Whole view against CS2 (`compare_shots.py`) 0.990 (before 0.952); 4.0 % of the pixels changed, all paint.
- **T spawn, shade:** 1.86 at hue 57°; whole view 0.955 (before 0.953); 0.4 % changed. `tdoors` 2.0 %, `top` 0.3 %;
  `ctcrates` changed in its paint line and "NO-IDLE ZONE" text (the rest of its difference is the camera offset
  decision 008 noted).
- **Base colour** (`ShowFlag.Lighting 0`, `overlay_contrast.py --albedo 237,179,63`): the paint is a plain alpha
  blend of the draw tint over the asphalt, opacity 0.43–0.45 on the bays (the texture's 0.49 through Unreal's mip
  filtering) and 0.34 on the farther line, with a fitted base-colour scale of 1.05–1.14 (the scene's exposure) and a
  residual under 0.5 sRGB steps.

**Checked in CS2 afterwards** (same day, Steam F12 captures at both spawns, normal and `mat_fullbright 1`; in
`test-data/shots/overlay_decals/cs2/`, local only):

- **Against fresh captures the lit contrast is within 3–7 %:** CS2 2.13 at the CT bays and 2.01 at the T bays
  (default auto exposure), Unreal 2.07 and 1.86. The older reference shots gave 2.35 and 2.29 for the same scene:
  the ratio of a near-white paint moves with the exposure, through the tone curve's shoulder, so lit ratios compare
  only at the same exposure. Hue: CS2 42° / 60°, Unreal 42° / 57°.
- **CS2 draws the paint more opaque than its texture says.** In the base-colour shots (linear, exposure-free; the
  fitted scale of CS2's fullbright is 0.94–0.97, i.e. albedo × 1), the per-pixel opacity of the stripes, solved from
  R − B against the draw tint, peaks at 0.57–0.61 in CS2 against 0.46–0.49 in Unreal (the texture's 0.486). The most
  opaque 0.5 % of the pixels follow about the same ratio (Unreal 0.51–0.69, CS2 0.63–0.86): **1.2–1.35×, about
  1.27×**, not a curve such as a square root (0.70), gamma 2.2 (0.72) or two blended layers (0.74), which all
  overshoot at 0.49. The three channels of
  CS2's paint also disagree under a plain blend (R 0.56–0.62, G 0.66–0.77, B 0.55–0.60): CS2's paint is a little
  yellower than the draw tint (extrapolated to full opacity about sRGB 237, 190, 64 against 237, 179, 63), while
  Unreal's channels agree (R 0.48–0.49, G 0.48–0.51). Where that comes from is not found: the material's
  `g_flOpacityScale` is 1, the overlay has no vertex alpha, and the decompiled shaders available show no translucent
  variant of `csgo_lightmappedgeneric`. Not compensated: a factor nobody can explain would be a guess, and with the
  lighting fixed the lit result is already within 3–7 %.
- **CS2's asphalt is bluish in base colour** (sRGB 100–106, 107–114, 114–121) where Unreal's is neutral grey
  (103, 103, 103): a separate question about the ground's material, not the overlay.

## Alternatives considered

- **Surface ForwardShading** for translucent materials (a copy of Interchange's `M_Default` with
  `TLM_SURFACE_PER_PIXEL_LIGHTING`, Interchange's instances re-parented to it). Fixes the direct sun only (table
  above), and costs a second copy of Interchange's master. Still the better mode for translucent surfaces that do
  not lie on another one (glass, fence cards, foliage cards); not done here, see Consequences.
- **A brighter or more opaque paint** to make up for the light. Would fit one lighting only: the sun's and the
  shade's under-lighting differ.
- **Copying the paint into the ground's material** (blending the overlay's textures into the asphalt's). Exact, but
  needs a material per ground-overlay pair and breaks where an overlay spans several surfaces.

## Consequences

- de_nuke 17, de_mirage 73, de_dust2 12, de_inferno 13, de_anubis 6, de_vertigo 15 overlay materials become decals.
- **Static overlays in other blend modes are still exported as plain alpha blend**, which is wrong for them:
  `csgo_static_overlay`'s `F_BLEND_MODE` is 0 Opaque, 1 Translucent, 2 Alpha Test, 3 Mod2x, 4 Additive, 5 Multiply,
  6 ModThenAdd (its feature list in GameTracking-CS2; Mod2x has `DstBlend One`). de_nuke has 30 Mod2x overlays
  (unlit grime), 2 opaque and 1 alpha-tested; de_inferno 6 and de_anubis 7 alpha-tested, 1–4 Mod2x per map. They
  keep the translucent material until they get their own handling.
- **Other translucent materials keep Interchange's `TLM_SURFACE` lighting** and so are still under-lit in direct
  sun: de_nuke's drain covers, chain-link fence cards and SWAT cards, de_mirage's car glass and puddle, de_dust2's
  palm cards, etc. (2–6 per map). Surface ForwardShading is the likely fix (see the table); not checked against CS2.
- DBuffer decals are needed (`r.DBuffer`, on by default; now stated in the sandbox's `DefaultEngine.ini`). DBuffer
  decals cannot write ambient occlusion; the overlays checked use `default_ao` anyway.
- Overlapping overlays are blended in Unreal's decal order, not CS2's (none found overlapping on de_nuke's paint).
- CS2's paint stays about 1.27× more opaque and a little yellower than the export says (see "Checked in CS2"); if
  other overlays show the same, the opacity rule of `csgo_lightmappedgeneric` overlays needs finding.
