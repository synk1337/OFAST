# How CS2 paints a weapon (research for M10)

Notes from CS2's own files (read with OFAST's VPK, resource and KV3 readers, 2026-10-27) and a few public sources.
What is confirmed comes from the files; guesses are marked.

## From the demo to a paint kit

A weapon's econ attributes give the paint kit, seed, wear, stickers and keychain (`docs/formats/demo.md`, "Weapon
finishes"). `scripts/items/items_game.txt` → `paint_kits` → `"<id>"` holds only: `name`, `description_tag` (its
name for people, in `resource/csgo_english.txt`), `style`, `wear_remap_min` / `_max`, `use_legacy_model`,
`composite_material_path` (332 of 1481 kits), `wear_default`, `seed` and a few leftovers from CS:GO (`vmt_path`,
`vmt_overrides`). CS:GO's look fields (pattern, colours, pattern scale, offset and rotation ranges) left
`items_game`: they are in the kit's composite material and paint material now.

A kit's composite material is its `composite_material_path`, else `weapons/paints/legacy/<name>.vcompmat` (all 1077
such kits have one; e.g. kit 180, `cu_fireserpent_ak47_bravo`). Gloves: `gloves/paints/<name>.vcompmat`. `style`
is the paint material's `F_PAINT_STYLE` + 1 (Fade 6 → 5, Fire Serpent 7 → 6, Case Hardened 8 → 7).

## Composite materials (`.vcompmat_c`)

KV3, `_class = "CCompositeMaterialEditorDoc"`, a list of assembly procedures. A legacy kit's own file only names its
paint material (`materials/models/weapons/customization/paints/vmats/<name>.vmat`, shader `csgo_customweapon.vfx`)
and includes `weapons/paints/legacy/_shared_paint_generic.vcompmat`, which does the work for every legacy kit:

1. **Match**: applies to the weapon materials that have a `composite_inputs` attribute.
2. **Inputs** (named containers): `target` (the weapon's own `csgo_weapon.vfx` material); `composite_inputs` (the
   material that attribute names, e.g. `weapons/models/ak47/materials/composite_inputs/weapon_rif_ak47_composite_inputs.vmat`,
   shader `csgo_composite_inputs.vfx`: masks, cavity, a position map, the weapon's UV scale and length); `paint`;
   `econ_instance` (`g_nRandomSeed`, `g_flWearAmount` 0 – 1); `grunge_wear` (ranges for where the wear and grunge
   textures sit: scale 1.6 – 1.8, offset 0 – 1, rotation 0 – 360°).
3. **Random roll** seeded by `g_nRandomSeed`, in this order: pattern offset (x, y), pattern rotation, wear scale,
   wear offset (x, y), wear rotation, grunge scale, grunge offset (x, y), grunge rotation (case-hardened templates
   roll two more). A kit narrows a range by adding its own variable (Fade: pattern rotation −55 to −65).
4. **Generate textures**: the paint material is rendered over the weapon's UV layout, 4096² (some 2048²), into the
   weapon material's `g_tColor`, then its roughness / metalness; the normal map and pearlescence are copied over.

Fire Serpent's paint material (style 6, "custom paint job"): `g_tPattern` (`fireserpent_ak47.psd`), the shared
`g_tWear` and `g_tGrunge` textures, paint roughness 0.38, pattern scale 1, colour brightness 1. Newer kits carry
their textures and colours (`g_vColor0` – `3`, `g_tPaintByNumberMasks`, `g_tNormal`…) in an `exposed_params`
container instead.

**Guess** (strong): the roll uses CS:GO's uniform random stream, Park–Miller's minimal standard generator with a
32-entry shuffle table, `RandomFloat(lo, hi) = lo + f (hi − lo)`. CS2's Fade range matches the CS:GO table of the
MIT-licensed fade calculator (github.com/chescos/csgo-fade-percentage-calculator, after
github.com/Step7750/UniformRandom), still kept up for CS2. To check against CS2 screenshots.

**Known** (decision 024): the pixel maths of `csgo_customweapon`, read from CS2's compiled shader and ported in
`export::paint` (its module docs give the formulas). GameTracking-CS2 tracks Source 2 Viewer's decompilation of the
default variant only (`game/csgo/shaders_vulkan_dir/shaders/vfx/csgo_customweapon.slang`: the vertex stage, the
solid style, every style's parameters and texture formats); the other styles are other static combos of the same
file, read as below. ValveResourceFormat reads `.vcompmat` as KV3 but does not composite ("Source 2 Viewer cannot
show or export a weapon with a skin applied", its CS2 notes); Skinshotter (closed source) ported the shaders to WGSL.

### Reading CS2's paint shader

`csgo_customweapon_vulkan_50_ps.vcs` (in `shaders_vulkan_dir.vpk`, 5.8 MB, format 72) is a resource whose `DATA`
block (KV3) describes the static combos and is followed by their bytecode, zstd-compressed with a dictionary.
Format 72 needs Source 2 Viewer 20.1 or newer (October 2025's 15.0 reads 59 – 70). The steps are in
`tools/shaders/vcs_combos.py` (with `examples/shader_blocks.rs`):

- **Static combo id** = Σ value × stride over `m_staticComboArray` in order, each stride the product of the ranges
  before: `S_PAINT_STYLE` (0 – 8) stride 1, `S_OVERRIDE_NORMAL` 9, `S_USE_ALL_MASKS` 18, `S_ROUGHNESS_TEXTURE` 36,
  `S_PEARLESCENCE_MASK` 72, `S_METALNESS_TEXTURE` 144, `S_SEPARATE_CHANNEL_INPUTS` 288, `S_OVERLAY_TEXTURE` 576,
  `S_CASE_HARDENING` 1152, `S_CASE_HARDENING_TRILINEAR` 2304, `S_PATINA_AGE` 4608. Only valid combos are stored
  (`m_staticComboIDs`).
- **Its code**: `m_staticComboData[i].m_nByteCodeDataIdx` → `m_byteCodeData[k]` (`m_nOffs`, `m_nSize`, `m_offs`
  per shader in the block); the block at the resource's size + `m_nOffs`, `[−5][uncompressed][compressed]` + zstd
  with dictionary `0x255df362`; in it, at `m_offs[m_byteCodeIndex[0]]`, `[version][size]` and the SPIR-V.
- **Names**: SPIR-V keeps none; Source 2 Viewer's rules restore them from the combo's `m_allVars` (each entry
  `variable index | set << 12` in the high 16 bits, a destination in the low 16): the globals' members by byte
  offset / 4, textures by binding − 30. Style 0 decompiled this way matches GameTracking-CS2's file, names included.
- **Which combos matter**: the nine styles (0 – 8) and the same with `F_SEPARATE_CHANNEL_INPUTS` (288 – 296): CS2's
  own weapon models (Glock, USP-S, AWP, MP9…) set it, the CS:GO-era ones (knives, the legacy AK) do not. It changes
  the cavity (`0.96 R^1.5`), the wear curve and threshold, and gunsmith's and patina's scratches. The paint textures
  (36 + n, 144 + n) only swap a value for a texture; `S_OVERRIDE_NORMAL` only adds the normal output. Then the
  variants CS2's kits use (`examples/paint_census.rs` counts them over every kit): overlays (576 + n, 864 + n; 70
  kits, on solid, hydrographic, spray paint, custom and gunsmith), case hardening (1152 + n, on solid, patina,
  gunsmith; 23 kits), spray paint's all-masks variant (20, 308; 37 kits); the halftone (16 kits) is a switch
  (`bSpraypaintHalftone`) inside the spray styles' own code. The overlay's UVs come from the vertex stage (one
  program for every combo): the weapon's, the pattern's (`g_bOverlayUsesPatternUVs`), or its own placement
  (`g_bOverlayUsesUniqueUVs`).
- **Colour space**: a texture variable's `m_nTypeSpecificBits` low bit is its sRGB read. The combos name different
  entries of the same texture: `g_tPattern` is linear in styles 1, 2, 4, 5 and solid's case hardening, sRGB in 6,
  7, 8; the overlay, the case-hardening ramp and the grunge are sRGB; the masks linear.

**Patina age is unused** (read 2026-10-29): `F_PATINA_AGE` (4608 + n) ages gunsmith's metal parts along a colour
ramp (`g_tPatinaAgeColorRamp`, e.g. copper → brown → verdigris, its alpha the metal's turn) by cavity, wear texture,
the kit's normal map and a start age, `g_tPatinaAgeMask` keeping parts young. The gunsmith template
(`workshop/paintkits/templates/gunsmith_extended.vcompmat`) sets it, but turns it off when `g_bLegacyPatina` is
`True`, and the four kits that carry it (Skrrrt M4A4, Monarch R8, Prototype MP9, Arctic Camo SCAR-20) all set that:
they are plain gunsmith, as CS2's previews of them show (`panorama/images/econ/default_generated/<weapon>_<kit>_light`,
`_medium`, `_heavy`: silver metal when new, the legacy patina's colours when worn). OFAST read the condition's `True`
as an unknown word until then. The same kits' combos (404, 980: a roughness texture, pearlescence, an overlay) show
that a roughness texture replaces gunsmith's own metal roughness, and that an overlay's own wear takes the separate
channels' 0.56 – 0.74 even on gunsmith.

**Trilinear case hardening** (2304 + n, read 2026-10-29: combos 3456 and 3744 against 1152): solid's case hardening
with the pattern projected from the weapon's 3D position as spray paint's (17 position taps × 2, the (y, z), (x, z)
and (y, x) planes blended by the surface normal), its four extra ramp taps moving the position's y and z by ±0.0004
instead of the UVs by ±1/2048; everything after the pattern reads is the same program. One kit: Soch Mountains on the
M4A1-S, with its own position texture (`g_tPositionOverride`, `m4a1_surface_wrap`) and a 2D ramp read at row 0.88 +
the pattern's G.

**The paints' normal maps** (read 2026-10-29): recipes generate only `g_tColor` and `g_tMetalness`. A kit's own
normal map reaches the weapon another way: the gunsmith-era templates end with `COPY_PROPERTY` steps whose target is
a plain name, the painted material's own parameter — `exposed_params.g_tNormal` → `g_tNormal` when
`g_bUseNormalMap` is true (395 kits), `g_tFinalAmbientOcclusion` → `g_tAmbientOcclusion` when
`g_bOverrideAmbientOcclusion` (105), and the pearlescence, glitter and iridescence values. The kit's normal map is
laid out on the weapon's UVs and replaces the weapon's own. The legacy kits' shared recipe
(`weapons/paints/legacy/_shared_paint_generic.vcompmat`) copies their paint material's `g_tNormal` whenever it has
one. Six of the 395 hand over a flat stand-in (three `materials/default/default_normal`, three compiled from a
material constant, `<material>_vmat_g_tnormal_…`, 1×1: Bloodsport's): OFAST keeps the weapon's own normal map there
(whether CS2 lets them flatten the weapon is not checked). Round 17's Printstream USP-S, Autotronic M9 Bayonet and
Gungnir AWP carry a real one.

**Pearlescence** (read 2026-10-29 from `csgo_weapon`'s combo 0, seen on the Gungnir AWP: CS2 indigo where OFAST's
bake keeps the pattern's sky blue, hue 0.633 against 0.570, woxic's view at tick 153201): the weapon shader turns
the colour's hue about the grey axis (Rodrigues, axis (1, 1, 1)/√3) by `g_flPearlescentScale × (1 − V·N) ×` the
pearlescence mask (the generated surface's B; its A holds the scale too), then mixes back towards the luminance by
`saturation^0.125`. Face on nothing turns; at grazing angles the Gungnir's 0.5 turns it up to 29°. The scale is one
of the values the recipe copies onto the material (1223 kits; not 0 on 64: 30 Dopplers and Gamma Dopplers, 13 custom
paints such as the Printstreams at 20, 17 gunsmith such as the Gungnir). View-dependent: it needs a material of
OFAST's own in Unreal (Interchange's glTF master has no such input) and the mask exported.

The surface's B, the share that turns (read 2026-10-29 from the composite's combos 0, 1, 2, 4, 6, 7, 8 and their
`S_PEARLESCENCE_MASK` variants 72, 74, 76, 78, 80): where the paint is, `1 − gone` (the paint's own wear;
anodized's `max(1 − masks R, gone)`; patina's `1 − no-paint mask`), × `g_tPearlescenceMask`'s R with
`F_PEARLESCENCE_MASK` — on the pattern's placement when `g_bPearlescenceMaskUsesPatternUVs` (the default), else on
the weapon's UVs and squared with the paint share; solid's on the weapon's UVs, not squared, whatever the switch;
anodized multicolour's (Dopplers) masked share × `1 − max(masks G, B)`; gunsmith's × masks R with
`g_bPearlescentOnMetallicOnly`. Every combo writes it through the sRGB-to-linear curve as it does roughness and
metalness (so the stored byte is the value). OFAST bakes it into its metallic-roughness texture's R
(`export::paint`) and draws the turn in Unreal (`tools/unreal/pearlescence.py`).

**Not read yet**: glitter and iridescence, which the weapon material draws with the values copied onto it.

### The composite's steps

Each texture a composite material generates (`COMP_MAT_PROPERTY_MUTATOR_GENERATE_TEXTURE`: `g_tColor`, then
`g_tMetalness` in roughness mode) starts from a container's parameters (the paint material) and runs its
instructions in order: `COPY_MATCHING_KEYS` (a container's parameters: the composite inputs, `econ_instance`,
`exposed_params`, `grunge_wear`), `COPY_PROPERTY` (one parameter under another name: a newer kit's
`g_tPaintByNumberMasks` over the weapon's `g_tMasks` when `g_bOverrideDefaultMasks`), `SET_VALUE` (a newer kit's
template switches a feature off when its `g_bUse*` is: overlay, metalness, roughness and pearlescence textures;
roughness per colour on), each under conditions (`INPUT_CONTAINER_VALUE_EQUALS`, written `1`, `0`, `true`;
`…_VALUE_EXISTS`; `INPUT_CONTAINER_EXISTS`; any of them negated by `m_bPassWhenTrue = false`, in 187 weapon
composites), groups of them inside `CONDITIONAL_MUTATORS`. A newer kit's colours are
`LOOSE_VARIABLE_TYPE_COLOR4` variables (`m_cValueColor4`, bytes), 750 of them over the composites; its per-colour
metalness and roughness (`g_vPaintMetalness`, `g_vPaintRoughness`) `FLOAT4` (`m_flValueFloatX` – `W`, in 152
composites; `FLOAT3` in 29). Missed, Royal Guard's slide came out as matte paint where CS2 shows half-metal blue
and fully metal gold (checked side by side, 2026-10-28).

## Gloves

**Known** (2026-10-27, read from CS2's files):

- **In the demo**: the pawn's `m_EconGloves` is a whole econ item (`m_iItemDefinitionIndex`, account, quality), its
  finish in `m_EconGloves.m_NetworkedDynamicAttributes.m_Attributes` (3 entries: attributes 6, 7, 8, as a weapon's).
- **The recipe**: `gloves/paints/<kit>.vcompmat` including `gloves/paints/_shared_paint_generic.vcompmat` (the
  `glove_*` kits name theirs in `items_game`). Its paint materials (72, `gloves/paints/*.vmat`) use
  `csgo_customglove.vfx`. The glove model's materials are the targets (`composite_target` string attribute); one of
  each pair holds the int and float attributes `left = 1` and `seed_offset = 1`. Containers new to gloves:
  `CONTAINER_SOURCE_TYPE_VARIABLE_FROM_TARGET_ATTR` (`handedness` ← the target's `left`, `material_seed` ←
  `seed_offset`). Procedure-level steps before the textures: two rolls of `instance_params` (pattern and grunge
  offset and rotation), seeded by `econ_instance.g_nRandomSeed` unless `material_seed.seed_offset` is 1, then by
  `g_nRandomSeedAlt` (set by the game, not in any file: the seed + 1 — `client.dll`, 1.41.9.3, sets
  `g_nRandomSeed` to the item's seed and `g_nRandomSeedAlt` from the same call with `lea r8d, [rax+1]`, at both of its
  references to the name); a `COPY_PROPERTY` into a container's own variable
  (`instance_params.g_fPatternTexCoordOffsetLeft` → `instance_params.g_fPatternTexCoordOffset` when left). Five
  textures generated by `F_OUTPUT_MODE`: colour (0), normal with roughness in B (1), metalness and cloth (2, G and
  B), ambient occlusion (3), anisotropic gloss (4); `g_fWearProgress` ← the wear, `g_bFlipFixup` for the left hand.
- **The shader** (`csgo_customglove_vulkan_50_ps.vcs`, 6 combos: `S_BACKWARDS_COMPATIBILITY` 1,
  `S_ANISOTROPIC_GLOSS` 2, `S_TINT_ID` 4; Source 2 Viewer's `-d` writes its reconstruction with the expressions).
  Every glove kit uses combo 0 ("Source1 texture layout") but one (`motorcycle_carbonfiber_red`, anisotropic). Four
  material layers picked by `g_tLayerMask` RGB (detail, detail normal, damage normal, grunge, and per-layer values),
  its A an index into eight tints (`g_vColorTint1` – `8`, blurred over 9 taps); a pattern (`F_PATTERN`) painted by
  `g_nPatternMode` (palette by RGB, thickness and durability; or its own colours); damage where curvature
  (`g_tSurface` R) × durability × grunge alpha passes `g_vDamageLevels` with the wear; edge highlights from
  screen-space derivatives (`dFdx`, `dFdy`: in a bake, neighbouring texels at the 2048 the composite renders);
  bleaching, grime, hue shifts by luminance and noise; the textile albedo levels. Placements (`g_vPatternTexCoordXform`,
  `g_vGrungeTexCoordXform`) are the weapon's construction without the rounding, the rotation mirrored with
  `g_bFlipFixup`.

**Done** (2026-10-27): the demo's glove finish (`Appearance::glove_finish`), the composite's attribute containers,
seeded and conditional rolls and copies into containers (`paint`), the combo-0 bake (`export::gloves`), the looks and
viewmodels keyed by the glove finish. Round 17: 7 of its 10 players' glove skins painted (Crimson Kimono, Emerald
Web, Vice, Foundation); the 3 others are 2025's kits (`gloves/paints/volatile_02/glove_*`), whose recipe renders a
`workshop/paintkits/templates/glove_compositor.vmat` (`paint_compositor`) with the height-blended program (below).

**Checked against CS2** (2026-10-28): a pattern paints over mask region `g_nPatternReplaceIndex − 1` (authored 1 –
8; the decompiled code indexes the regions with the uniform, which its declaration's expression makes `this-1`):
Crimson Kimono's 1 covers the back of the hand (region 0) and leaves its knuckle panel (region 1) its slate tint.

**2025's kits** (`gloves/paints/volatile_02/glove_*.vcompmat`, 22; read and ported 2026-10-28): the `paint`
container is loose variables holding one `LOOSE_VARIABLE_TYPE_RESOURCE_MATERIAL` (`m_strResourceMaterial`: the
kit's own `items/assets/paintkits/volatile_02/textiles/<kit>.vmat`, shader `csgo_customglove_preview.vfx`, ~260
parameters); conditions read that material's parameters through it (`paint.F_PATTERN`), and `COPY_MATCHING_KEYS
paint` copies them. Textures generate from `paint_compositor` (`workshop/paintkits/templates/glove_compositor.vmat`),
with `F_ANISOTROPIC_GLOSS` and `F_CLOTH_SHADING` set at the procedure; the kit's `F_PATTERN`, `F_PATTERN_PAINT`,
`F_PUFFY_PAINT`, `F_TINT_ID` kept only when the kit sets them. Rolls per layer (`g_vUvOffset1` – `4`, each unless
`bSyncMaterialPlacement*`), the pattern's, then damage and grime placements; the alternate seed's roll first draws
`consumable_random_numbers.xyz` and four `.xy` (13 draws that only advance the stream: a roll's component count is
its swizzle's length, up to 4). The shader's height-blended program: combo 1 (and 3), with tint ids combo 5 (and
7), ~1,800 and ~2,100 lines of GLSL; inputs per layer: substrate and surface (`g_tSubstrate*`, `g_tSurface*` colour
+ tint mask, `…Normal*`, `…Properties*` AO/metal/cloth/height), damage height, grime colour; `g_tLayerId`,
`g_tTintId`, `g_tObjectProperties` (AO, curvature, high-touch), `g_tPattern` (sRGB) and `g_tPatternProperties`.
Its colour-adjust uniforms are 4×4 matrices from the material system's expressions
(`MatrixMultiply(MatrixColorTint2(tint, 1), MatrixColorCorrect2(float3(contrast, saturation, brightness),
TextureAverageColor(texture)))`), whose definitions no file holds; Source 2 Viewer's reconstruction is what the
environment shaders already use (`environment::color_correct`, `color_tint`), the average colour the texture's
stored reflectivity.

**The height-blended program** (read 2026-10-28; ported as `export::gloves::blended`): combos 5 and 7 share one
bytecode, 1 and 3 another, and 1 is 5 with the id tint white and of no strength and the pattern over every id (23 of
the 26 materials the kits name set `F_TINT_ID`; all three of round 17's kits run combo 7). Its other findings:

- The tint ids are read unfiltered from `g_tTintId` R (`ceil(R × 7)`), at the texel and its 8 neighbours (in that
  texture's texels); an id keeps the centre's where its row or column agrees, else 1/32 per corner, 3/32 per side and
  1/2 for the centre — the compiled code counts the top-right corner in both and leaves the top side out.
- The layers blend textures, values and matrices by weight, except the burnishing normal scale, both burnishing
  roughness brightnesses and `g_bDamageBevelUseTintMask*`, which take the last present layer's value (times its
  weight): the compiled program assigns them where it adds the others.
- Expressions this program applies: `g_fWearProgress` is gamma-encoded (`SrgbLinearToGamma`: a wear of 0.16 reads
  0.44), `g_fDamageHeightBlendSoftness*` at least 0.001, `g_fDamageBevelBlendSoftness*` plus the height softness,
  `g_fPatternNormalDistortion` a tenth, `g_vId*Color` sRGB-decoded (not A); the vertex stage places the pattern
  about `g_vPatternTexCoordCenter` with a per-axis `g_vPatternTexCoordScale`, the other hand by negating u (the
  rotation is not mirrored, unlike the default combo).
- Colour comes out linear (the target is sRGB); the other outputs write their values gamma-decoded so the target
  stores them as they are.

**Not done yet**: the default layout's anisotropic combo (one kit).

## Stickers

`sticker_kits` → `sticker_material` names `stickers/<path>.vmat` (shader `csgo_weapon_sticker.vfx`: the sticker
texture, holographic / glitter / paper / metallic flags, wear bias, scratches). On the weapon they are not baked: the
weapon's own material (`F_STICKERS`) draws five slots, each with an enable flag, offset, scale, rotation and wear
(the AK-47's defaults: slot 0 at (0.148, −0.434), scale 14.1) through a per-weapon sticker mask texture;
`weapons/stickers/dev/_shared_sticker_generic.vcompmat` copies each sticker material's `*Sticker0` keys into its
slot (`COPY_KEYS_WITH_SUFFIX`, `Sticker0` → `StickerN`, when the item's `g_bEnableStickerN` loose variable is set).

**The weapon shader's sticker code** (read 2026-10-28: `csgo_weapon_vulkan_50_ps.vcs`, static combo 512 =
`S_STICKERS`, every other combo a single bit; GameTracking-CS2 tracks only combo 0; 5,125 lines of GLSL, the five
slots unrolled):

- **Placement**: in `TEXCOORD 1` (the vertex stage passes it untouched beside `TEXCOORD 0`): sticker coordinates =
  `rotate(((uv₁ − 0.5) − g_vStickerNOffset) × |g_vStickerNScale.x|, g_flStickerNRotation × 2π) + 0.5`, drawn inside
  the unit square, where `uv₁.x > −1` and the weapon's `g_tStickerWepInputs` (in `TEXCOORD 0`, R mask, G cavity) is
  above 0. Offset, scale, rotation and wear are the material's unless the instance sets the attribute
  (`Expression(exists(Sticker0Offset) ? Sticker0Offset : this)`): the client turns the item's sticker attributes
  into those.
- **Blend**: alpha = clamp(sticker A × 12.75) × mask, applied squared; colour, roughness (the sticker's
  normal-roughness B), metalness and the normal lerp by it, slot 0 first.
- **Colour**: every sticker material read sets `g_bAutomaticPBRColorFittingSticker0`: the colour (tinted and
  desaturated by the metalness when metallic) fitted to albedo levels, `g_vAlbedoLevels` (0.045, 0.737, 0.85) or,
  for metallic stickers (foil, gold: `g_bMetallicSticker0`), `g_vMetallicAlbedoLevels` / `…DarkMetallic…` by
  brightness. Holographic ones mix in `g_tHoloSpectrumSticker0` at (mask G + view·normal + view·(a camera vector),
  mask B), fitted to `g_vHoloAlbedoLevels`, and are not metallic where the mask is; glitter adds view-dependent
  sparkles and half metalness.
- **Wear** (`g_flStickerNWear` > 0): `g_tStickerScratches` (capped by `g_fWearScratchesSticker0`) and the cavity
  against a threshold from the sticker's alpha and `g_vWearBiasSticker0`; paper stickers (`g_bPaperBacking…`) show
  a grey backing, others roughen to 0.8 unless metallic.

**Where the slots are**: the model's keyvalues (`m_modelInfo.m_keyValueText`) hold a `StickerMarkup` list, per
mesh (`body_hd`, `body_legacy`) and slot: `Offset`, `Scale`, `Rotation` (turns), a `SpecialIdentifier` (`Map`,
`Autograph`, `Team1`…) and the polygons a sticker's centre may be placed in, in the same offset space. The client
sets the material's slot attributes from it: the USP-S material's slot 4 has scale 0 (never drawn), its markup 10.16,
and CS2 draws FalleN's slot-4 sticker. The material's own slot values are only defaults.

**The sticker UV set** is a planar side projection (`examples/sticker_where.rs`: the USP-S's points at one offset sit
at the same x, z on both sides of the gun, about 18.5 inches per offset unit; the mask keeps one side).

**The item's values** (attributes 113 – 116 + 4n, 278 – 289): old stickers send scale 1 and rotation 0 (or
nothing), free placements an offset and a rotation in degrees. Taken as offsets in the UV set, FalleN's land below
the gun (on no triangle); taken as a shift from the markup's slot in sticker units (÷ the slot's scale), all five
land in the markup's polygons, along the slide where CS2 shows them (tick 150500). OFAST reads them that way: offset
= markup + item ÷ scale, rotation = markup + item ÷ 360, scale = markup ÷ item. The signs (which way a shift moves)
are not checked yet.

## Keychains

`items_game.txt` → `keychain_definitions` → `pedestal_display_model` (`weapons/keychains/<set>/vmdl/<kc>.vmdl`): a
model of three bones (`joint1` the ring at the origin, `joint2`, `joint3` the charm) hanging down at rest (bounds z
−2.07 – 0.12 for `kc_missinglink_catbeach`), simulated as cloth (keyvalues `cloth_sleep_enabled`). The item's
keychain offset (attributes 300 – 302) is a point in the weapon model's space: FalleN's USP-S (9.74, 0.55, 3.96), an
inch from the model's `keychain` attachment (8.63, 0.54, 4.00); KSCERATO's (17.5, 0.74, 4.1) on the suppressor.
The weapon models' keyvalues hold a `KeychainMarkup` list of the areas a keychain may hang from: four corners in model
space and a bone (`slide`, `weapon_offset`, `silencer`, `clip`…), `LegacyModel` for the CS:GO-era meshes'. The seed
(306) is not read yet.

The swing: each charm model's physics block holds a cloth model (`m_pFeModel`, Source 2's softbody). On the blank
`kc_missinglink_default`: 20 nodes, 4 of them static (the ring: `$ha_joint1`, `$ccjoint1_0`, `$ccjoint1_1`,
`joint1`), `joint3` and `joint2` driven by the cloth's nodes (`m_nFirstPositionDrivenNode` 18), gravity 360 inches/s²
(`flGravity`) on every free node, no point damping, no air drag, hinge limits of ±45° (three) and ±67.5° (the charm's),
`m_flLocalForce` 1, `m_flLocalRotation` 0. The mesh is skinned to the three joints, one bone per vertex. `joint2` sits
0.13 inch beside the ring (the link lies flat), `joint3` 0.39 inch below it, the charm's weight (the centre of its
vertices) 0.8 inch further. OFAST does not run the cloth solver: it swings `joint2` and `joint3` as a chain of rods
from the ring under gravity, with those hinge limits and a damping of its own choosing (`export::keychains`, swing),
driven by where the weapon takes the ring in the world (the agent's place and pose; in first person, the eye camera
and the viewmodel's pose). The pendulum swings about 2.7 times a second, the pace of A–D strafes: holding an angle,
yuurih's charm swings to its hinges. Not checked against CS2's own motion (screenshots are stills).

Of the 143 definitions (2026-10-29), 82 name a model. 23 of those, the community charms (`missinglink_community_01`),
share a blank model (`workshop_blanks/kc_missinglink_default.vmdl`, a grey template material, one draw call) and add
`keychain_material`, the material CS2 draws them with: yuurih's keychain 50 (`kc_missinglink_pencil`) is an orange
pencil in CS2, grey on the bare blank. The other 61 are highlight reels (tournament souvenir charms, `highlight_reel`
names a clip): no model of their own, `base` names the definition whose model they show (`kc_cologne2026`). OFAST draws
the reel's base charm; the clip on its screen is not shown.

## Name tags

Shown on the weapon: the name tag module (`weapons/models/shared/nametag/nametag_module.vmdl`, a data plate) on the
model's `nametag` attachment (`nametag_legacy` on legacy meshes; knives have one too). Its material is a composite
(`nametag_default.vcompmat`): `g_tColor` generated at 512 from the plate's own colour with `DRAW_TEXT` of the
name at (0.04, 0.038), colour (241, 241, 241), font "Arial Narrow" (no size given). yuurih's M4A4 shows "LOVE from
Jimzor v3" on its receiver in CS2 (tick 152705). OFAST draws the text with the Windows fonts (Arial Narrow, then
Arial, Microsoft YaHei, Segoe UI Symbol per missing character) 46 texels tall.

yuurih plays left-handed: CS2 mirrors his whole viewmodel, stickers included (their text reads mirrored, as in
OFAST's mirrored viewmodel), but the plate reads "LOVE from Jimzor v3" from the stock towards the muzzle with its
"PARKER" logo at the stock's end. That is the plate mirrored along its length, turned back by the view's mirror; a
mirrored copy of the right-hand plate showed the text backwards. The plate is symmetric along its length (bounds
±1.41 inches about its origin), so the mirrored plate covers the same place. How the game does it (client code, not
the shader: `csgo_weapon` has no such switch) is not known; the result matches.

## StatTrak

The demo sends the count as attribute 80 (kill eater, an integer) and updates it live: YEKINDAR's M4A1-S goes 1471 →
1472 at tick 155067, the tick MAJ3R falls to it (round 17). The weapon carries the StatTrak module
(`weapons/models/shared/stattrak/stattrak_module.vmdl`; on knives `stattrak_module_knife.vmdl`, the display alone) on
its model's `stattrak` attachment (`stattrak_legacy` on legacy meshes). The display is `csgo_weapon.vfx` with
`F_STATTRAK` (and `F_SELF_ILLUM`): the vertex shader's static combo `S_STATTRAK` picks the digits from
`g_nStatTrakValue` (the material's 654321 is a placeholder; the game sets the count). Decompiled (VS combo 8):

- a digit's vertices have a negative v in their first UV set (−1 … −0.924) and u in [d + 1, d + 2) for digit d,
  0 the hundred-thousands … 5 the units; the fraction of u covers atlas cell d (cells 1/16 wide: [d/16, d/16 + 0.055]);
- d = ⌊u⌋ − 1, n = count / {100000, 10000, 1000, 100, 10, 1}[d] (whole), cell = 1 + n mod 10 when n ≠ 0 or d = 5,
  else 0; u − d/16 + cell/16 (the texture repeats).

The atlas's (`stattrak_digit_atlas.psd`, 512²) top row: cell 0 a dim zero (leading zeros show unlit), cells 1 – 10 the
lit digits 0 – 9, 11 – 15 empty. YEKINDAR's view shows "001471" this way, lit orange (tick 152613). OFAST sets the
digits' UVs for the count when it writes the module, one module per count a weapon shows in the round on bones that
the animation shows in turn.

## Wear tiers

Not in the game's files; the community's: factory new 0 – 0.07, minimal wear 0.07 – 0.15, field-tested 0.15 – 0.38,
well-worn 0.38 – 0.45, battle-scarred 0.45 – 1 (lower bound included).

## What it means for OFAST

Every input is readable (KV3, materials, textures). Rebuilding a skin means OFAST's own version of each paint style,
baked over the weapon's UVs (in Rust at export, or as an Unreal material), checked against CS2 screenshots of the
same seed and wear; stickers as the weapon material's decals by slot.
