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

**Not read yet**: patina age (4608 + n; its own ramp and remap, 4 kits) and trilinear case hardening (2304 + n, 1
kit); pearlescence and the normal-map pass, which the weapon material uses after the composite.

### The composite's steps

Each texture a composite material generates (`COMP_MAT_PROPERTY_MUTATOR_GENERATE_TEXTURE`: `g_tColor`, then
`g_tMetalness` in roughness mode) starts from a container's parameters (the paint material) and runs its
instructions in order: `COPY_MATCHING_KEYS` (a container's parameters: the composite inputs, `econ_instance`,
`exposed_params`, `grunge_wear`), `COPY_PROPERTY` (one parameter under another name: a newer kit's
`g_tPaintByNumberMasks` over the weapon's `g_tMasks` when `g_bOverrideDefaultMasks`), `SET_VALUE` (a newer kit's
template switches a feature off when its `g_bUse*` is: overlay, metalness, roughness and pearlescence textures;
roughness per colour on), each under conditions (`INPUT_CONTAINER_VALUE_EQUALS`, written `1`, `0`, `true`;
`…_VALUE_EXISTS`; `INPUT_CONTAINER_EXISTS`), groups of them inside `CONDITIONAL_MUTATORS`. A newer kit's colours are
`LOOSE_VARIABLE_TYPE_COLOR4` variables (`m_cValueColor4`, bytes), 750 of them over the composites.

## Stickers

`sticker_kits` → `sticker_material` names `stickers/<path>.vmat` (shader `csgo_weapon_sticker.vfx`: the sticker
texture, holographic / glitter / paper / metallic flags, wear bias, scratches). On the weapon they are not baked: the
weapon's own material (`F_STICKERS`) draws five slots, each with an enable flag, offset, scale, rotation and wear
(the AK-47's defaults: slot 0 at (0.148, −0.434), scale 14.1) through a per-weapon sticker mask texture;
`weapons/stickers/dev/_shared_sticker_generic.vcompmat` copies each sticker material's `*Sticker0` keys into its
slot. **Unknown**: whether the demo's offsets (attributes 278 – 289) replace the slot's default or add to it, and
their units.

## Wear tiers

Not in the game's files; the community's: factory new 0 – 0.07, minimal wear 0.07 – 0.15, field-tested 0.15 – 0.38,
well-worn 0.38 – 0.45, battle-scarred 0.45 – 1 (lower bound included).

## What it means for OFAST

Every input is readable (KV3, materials, textures). Rebuilding a skin means OFAST's own version of each paint style,
baked over the weapon's UVs (in Rust at export, or as an Unreal material), checked against CS2 screenshots of the
same seed and wear; stickers as the weapon material's decals by slot.
