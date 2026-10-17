# 012 — Unlit CS2 materials become glTF `KHR_materials_unlit`; additive cards blend additively

- **Date:** 2026-10-18
- **Status:** accepted

## Context

Decision 010 found that `csgo_unlitgeneric` cards were exported as ordinary PBR materials, so Interchange gave them
its lit glTF master (`/InterchangeAssets/gltf/M_Default`, through `MI_Default_Blend` for the translucent ones) and
Unreal shaded them with the sun. It left them out of its per-pixel lighting and noted this.

Which exported materials CS2 draws unlit (the `extras.ofast.shader` of every material in the six exported maps,
against Valve's compiled shaders as decompiled in GameTracking-CS2):

| Shader | Pixel shader (decompiled) | Exported |
|---|---|---|
| `csgo_unlitgeneric` | `g_tColor × tint`, opacity = alpha × the tint's alpha, then fog; no light, lightmap or probe read | de_mirage `window_illum_backing` (3, opaque); de_dust2 `nuke_clouds_002` (14, Additive, `F_TWOTEXTURE`, 3D skybox) |
| `csgo_black_unlit` | black, then fog | `black_simple` on all six maps (10–41 placements) |
| `csgo_effects` | additive dust and light-shaft cards | not exported (`SKIPPED_SHADERS`) |
| the 2D sky | | exported as an HDR, drawn by OFAST's unlit sky material (`sky_intensity`) |

`csgo_unlitgeneric`'s tint is built in its vertex shader exactly like the prop shaders' (`lerp(1, rendercolor,
g_flModelTintAmount) × g_vColorTint`, decision 007); OFAST multiplied its placement tint only. Its `F_BLEND_MODE`
(0 Opaque, 1 Translucent, 2 Alpha Test, 3 Mod2x, 4 Additive, 5 Multiply, 6 ModThenAdd) cannot be said in glTF beyond
`BLEND`. `F_TWOTEXTURE` ("TwoTexture") adds `g_tColor2` with its own UV transform; the decompiled default variant
does not use it. Anubis's `black` (`generic.vfx`, with a normal map) is not an unlit shader and stays lit.

`black_simple` had no texture and no colour factor, so every glTF reader took white: Unreal drew it lit white.

Two more things decided what the cards look like:

- **de_mirage's `wrongway_timer`**, the card named in decision 010, is an atlas of eight stopwatch bands on
  `func_brush`es named `[PR#]retake.asite` / `retake.bsite` that **start disabled**: CS2 shows them only in Retakes.
  Since the view checks (docs/checks.md) the exporter leaves start-disabled entities out, so it is no longer exported.
- **de_dust2's clouds** (`nuke_clouds_002`: two white cloud textures, the shapes in alpha, up to 0.50 and 0.81) were
  exported black (`_tint000000`): the old skybox prefabs store the scene objects' `m_vTintColor` as (0, 0, 0, 0),
  which OFAST read as black (57 of the skybox's 84 objects; fixed in commit 5272b68, see map.md). Drawn alpha-blended,
  the black clouds darkened the sky in streaks, and the skybox bushes on the hill were black, where CS2 shows a light
  haze and green bushes (`test-data/shots/compare_de_dust2_t0.png`).

## Decision

- The exporter gives `csgo_unlitgeneric` and `csgo_black_unlit` materials **`KHR_materials_unlit`** (listed in
  `extensionsUsed`; not required, readers without it fall back to the PBR values), keeps `extras.ofast`, and gives
  `csgo_black_unlit` a black `baseColorFactor`. Interchange imports them with its unlit master
  (`MI_Unlit_Opaque` / `_Mask` / `_Blend`, `_DS`), which reads only the base colour texture and factor.
- `csgo_unlitgeneric` takes CS2's tint formula (`tint.rs`), like the prop shaders.
- **Two textures** (`F_TWOTEXTURE`, second texture at its default transform): the exporter bakes their product into
  the colour texture (`<texture>_two_<hash>.png`; colours multiplied in linear, alphas multiplied). That is Source 1's
  `UnlitTwoTexture`, which the feature ports, and it is what the CS2 picture shows (Checks).
- `import_map.py` gives the Interchange instance of an unlit card in **Additive** mode (`F_BLEND_MODE` 4) Unreal's
  Additive blend mode (`set_unlit_blend_modes`; Unreal's base pass writes colour × opacity, `BasePassPixelShader.usf`).
  Unlit materials stay out of the per-pixel lighting of decision 010.
- No scale: CS2 writes the unlit colour into the scene before exposure and tone mapping, like the sky, and one unit of
  CS2 radiance is one unit of Unreal luminance with OFAST's light units (`LUX_PER_UNIT / π` = 1, as `sky_intensity`).

## Checks

de_mirage and de_dust2 re-exported on top of `main` 62696e1 (every earlier decision merged) and re-imported with all
their views; pictures in `test-data/shots/unlit/` (local only), compared with `tools/compare/compare_views.py
--cs2-gamma 2.013074` against the CS2 references of `docs/checks.md`.

**Where the unlit surfaces show.** A ray through every 40th pixel of each view of the two maps (`map_raycast`, 1296
rays per view, 14 views of de_mirage and 19 of de_dust2) hits no `black_simple` and no `window_illum_backing`: the
black surfaces fill spots out of sight, and de_mirage's window glow sits 1.7 units behind opaque arched windows
(`window_6x10_arch`; a ray through every 12th pixel of the window finds no gap). On these two maps the change shows
in de_dust2's sky.

**de_dust2's clouds** (T spawn `t0`, the sky left of the wall: linear luminance, and the cloud structure as the
standard deviation of the luminance between a 4- and a 40-pixel blur, in % of the sky's):

| | Sky | Cloud structure | |
|---|---|---|---|
| CS2 | 0.702 | 0.55 % | JPEG block noise: no cloud shape shows above it |
| `main` (lit, alpha blend, white tint) | 0.767 | 0.31 % | faint warm streaks |
| unlit, additive, `g_tColor` alone | 0.798 | 0.97 % | white streaks CS2 does not have |
| unlit, additive, the two textures' product | 0.778 | 0.27 % | no streaks |

With one texture the clouds' alpha averages 0.043; the product, 0.010. The sky stays 1.11× CS2's here, against
1.09× without the clouds: the rest of the difference is the sky itself, not this change. Pictures:
`compare_de_dust2_t0_sky.png` and `compare_de_dust2_t0_cloud_residual.png` (each row's mean taken out, × 12).

**Nothing else moved.** Every view of both maps reads within ±0.02 of `main`'s import (whole-picture ratio to CS2),
except two: de_mirage's `connector`, whose file held the sniper view (the screenshot-loop slip `docs/checks.md`
describes; `sniper` was not written), and `underpass` (1.42 → 1.38; `main`'s figure came from a separate
`shoot_views.py` run). The skybox bushes stay green, as on `main`.

## Alternatives considered

- **An OFAST unlit master instead of Interchange's** — not needed: Interchange's handles the extension, and the
  only thing it cannot say (the additive mode) is a base-property override on its instance.
- **Leaving the cards lit and darkening them** — the lit result depends on where the sun falls; CS2's does not.
- **The second texture in Unreal's material instead of a bake** — needs a material of OFAST's own for one material on
  the maps checked; the bake works for every glTF reader. A second texture with its own scale, offset or rotation
  cannot be baked onto the first one's UVs and keeps the first texture alone.
- **Exporting Mod2x / Multiply unlit cards as Unreal's Modulate** — none on the six maps; Modulate is dst × src, Mod2x
  dst × src × 2, so they would need their own check. They stay alpha-blended.

## Consequences

- Other glTF readers (Blender, viewers) also draw these materials unlit, and `black_simple` black.
- Opaque unlit materials in the 3D skybox still get the lit skybox master (`setup_skybox`), which carries the depth
  offset that keeps the skybox behind the world; `black_simple` lit is still black.
- In the realistic style (physical sun, auto exposure) unlit colours are too dark: they would need the style's
  exposure scale.
- The two-texture product rests on Source 1's rule and on the CS2 picture showing no clouds; the second texture's
  scroll (`g_vTex2CoordScrollSpeed`, absent here) is not reproduced.
