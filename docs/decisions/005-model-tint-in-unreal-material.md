# 005 — Environment model tints applied in Unreal's material, not baked

- **Date:** 2026-10-16
- **Status:** accepted

## Context

Decision 004 stopped baking what changes no pixels, which left de_inferno with 555 colour bakes (3.3 GB) and a
5.4 GB export. Most of what remained were real model-tint variants: placements give `csgo_environment(_blend)`
materials a tint colour (`rendercolor`), the shader recolours the tint-masked area with it, and one plaster texture
carries about 100 tints — each a 2048² copy of the same colour texture.

The tint is cheap to compute where CS2 computes it, per pixel: towards the tint's hue at the same luminance, by the
layer's strength (`g_flModelTintAmount` × `g_bModelTintN`) × the tint mask (the height texture's G after a contrast
and brightness) × (1 − the tint's darkest channel). Unreal can do the same in a material.

## Decision

- **The exporter bakes colour without the model tint**, once per (colour texture, colour settings). Every tint
  still gets its own glTF material (`<name>_tintRRGGBB`), all sharing that colour texture.
- **The tint travels in `extras.ofast.environment`**: the tint (linear RGB), whether it is baked, and per layer the
  files (colour, normal, surface, height) with the tint strength and the mask's contrast and brightness. The height
  texture is written for the layers Unreal tints (blend materials already had it); it holds the tint mask in G.
- **Unreal applies it with one material function**, `MF_OFAST_ModelTint_v1` (the formula of `model_tint` in
  `environment.rs`, in HLSL), used by the height-blend master (`M_OFAST_HeightBlend_v2`, per layer before the
  blend), a new single-layer master (`M_OFAST_Environment_v1`) and the 3D-skybox master (`M_OFAST_Skybox_v3`).
- **Every single-layer environment material gets the new master**, tinted or not. Found on the way: Interchange's
  re-import keeps existing material instances as they are, so de_inferno's project still drew these materials as
  they were converted before the environment bakes (9e55752): the plain colour × the tint as a factor, no colour
  adjustments. The terrace tablecloths were solid dark red in every Unreal render; CS2 shows red and white check,
  and so does the new master.
- **Other glTF readers see the untinted colour** by default. `ofast export-map --bake-model-tints` bakes the tints
  into the colour as before (one texture per tint) and marks them as baked, for Blender and viewers.

Result on de_inferno: 126 colour bakes (0.76 GB) instead of 555 (3.3 GB); the environment layers use 269 colour
textures, baked or plain. 39 height textures are new (10 MB, the tinted props'). The `textures` folder: 1099 files,
2.6 GB, instead of 1467 files, 5.1 GB; the whole export 3.3 GB instead of 5.4 GB, written in 33 s instead of 47 s.
`--bake-model-tints` gives the previous export (1467 files, 555 colour bakes).

In Unreal (`test-data/shots/model_tint/`): base-colour renders (lighting off) of the three views match the
`--bake-model-tints` export imported through the same masters to 0.3–0.6 sRGB levels on average, so the material
reproduces the bake. Lit, the CT-spawn view measures 0.849 of CS2's luminance (0.851 before; two renders of the
same import differ by more than that) with the pink and blue plaster unchanged, and the T-spawn view 1.48 (1.46).

## Alternatives considered

- **The tint mask in the colour bake's alpha** (where alpha is unused). Needs a bake even for layers with nothing
  but a tint, makes those PNGs RGBA (and Unreal's textures BC3 instead of BC1), and alpha-tested materials need
  their alpha: a second mechanism for them. The height texture is there already for blend materials, shared by all
  settings, and the props' ones are small.
- **A tint mask texture of its own** (G only, grey PNG). Smaller than the height texture, but a new texture kind
  for 10 MB of savings.
- **A `baseColorFactor` for other glTF readers.** A factor multiplies the whole surface; the tint keeps luminance
  and only touches the masked area, so tinted walls would come out darker and wrongly coloured. Untinted, with the
  option to bake, is more honest.
- **Keep baking by default.** Twice the colour textures for a result Unreal can compute exactly.

## Consequences

- Unreal's environment materials depend on `import_map.py`: imported without it (plain Interchange), tinted
  environment surfaces are untinted.
- `extras.ofast.height_blend` now holds only the blend settings; the layer files moved to
  `extras.ofast.environment`. An `import_map.py` from before this change cannot read new exports (and the reverse).
- The tint is computed after texture filtering, as in CS2, instead of before it in the bake: edges of the mask
  can differ by a level.
- Single-layer environment materials that are alpha-tested or translucent get their blend mode as an instance
  override of an opaque master (de_inferno has 68 alpha-tested ones and no translucent ones; translucent is
  untested).
- The other materials still come from Interchange's glTF import, whose re-import does not update existing material
  instances: whenever the exporter changes how it converts a material, a project that imported the map before keeps
  the old look until those assets are deleted or rebuilt. Not fixed here.
