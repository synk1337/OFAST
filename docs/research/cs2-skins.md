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

**Unknown**: the pixel maths of `csgo_customweapon`'s nine styles (solid, hydrographic, spray, three anodized,
custom paint job, patina, gunsmith): how wear, grunge, cavity, masks and colours combine. It is only in the compiled
shaders (`shaders_pc_dir.vpk`: `csgo_customweapon`, `csgo_composite_inputs`, `csgo_weapon`, `csgo_weapon_sticker`).
ValveResourceFormat reads `.vcompmat` as KV3 but does not composite ("Source 2 Viewer cannot show or export a weapon
with a skin applied", its CS2 notes); Skinshotter (closed source) ported the shaders to WGSL.

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
