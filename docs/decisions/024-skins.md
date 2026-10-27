# 024 — Skins: read from the demo, painted by CS2's own shader maths

- **Date:** 2026-10-27
- **Status:** accepted (first pass)

## Context

M10: the weapons in a round as CS2 shows them, with their skins. The demo carries each weapon's finish (paint kit,
seed, wear, stickers, keychain, name tag: `docs/formats/demo.md`, "Weapon finishes"). CS2 paints a skin at run time
with composite materials: the paint kit's material (`csgo_customweapon.vfx`) rendered over the weapon's UV layout
from the weapon's composite inputs (cavity, masks, position, surface), the seed rolling where the pattern, wear and
grunge textures sit, into the weapon material's own colour and roughness (`docs/research/cs2-skins.md`). Every input
is readable; the pixel maths of the paint styles is in Valve's compiled shaders.

## Decision

- **CS2's shader as the reference** (the user's choice, 2026-10-27, replacing their first choice of OFAST's own
  version once the decompiled shader turned out to be public): Valve's compiled `csgo_customweapon`, rewritten as
  OFAST's own Rust, as the tint and environment shaders were (decisions 007, 008, 011, 012). GameTracking-CS2
  tracks its default variant (`csgo_customweapon.slang`); the other styles are read from CS2's own shader file, each
  static combo's SPIR-V decompiled with SPIRV-Cross and named as Source 2 Viewer names it (the user approved
  downloading Source 2 Viewer 20.1, which reads CS2's current format; `docs/research/cs2-skins.md`, "Reading CS2's
  paint shader").
- **Read like CS2** (`paint`): the paint kit from `items_game.txt`, its composite material and what it includes, the
  roll with a uniform random stream seeded by the pattern seed, and the parameters in the order the generator copies
  them (paint material, the weapon's composite inputs, the loose variables, the roll, seed and wear).
- **Baked in the export** (`export::paint`), per weapon material that names its `composite_inputs` and per skin:
  a colour texture and a metallic-roughness one in the weapon's UV layout, at the pattern's size (2048 at most), in
  the glTF (`TextureJob::PaintColor`, `PaintSurface`), so Unreal (and any glTF reader) shows the skin with no
  material of its own. Third person and first person both (the weapon looks carry the finish:
  `loadout::WeaponLook::finish`).
- **The nine styles, both input layouts**: static combos 0 – 8 (solid, hydrographic, spray paint, anodized,
  anodized multicolour, anodized airbrushed, custom paint job, patina, gunsmith) and 288 – 296, the same with
  `F_SEPARATE_CHANNEL_INPUTS` (CS2's own weapon models; the CS:GO-era ones and the knives use the first). Shared:
  the placements (rounded to hundredths, the shader's own translation, the weapon's size scale), where the paint is
  gone (wear texture against the cavity, the no-paint mask, durability and softness, `0.58 – 0.68`; `0.56 – 0.74`
  and a wear-dependent cavity curve with separate channels), the grunge, the albedo levels. Then each style's own
  colour, roughness and metalness; spray paint and Fades project their pattern from the weapon's 3D position. The
  module docs of `export::paint` give the formulas.
- **Only what it paints right**: overlays, case hardening, patina age and spray paint's halftone and all-masks
  variants are not read yet; those weapons keep their own finish, with a warning — a wrong skin looks worse than
  none.
- **Legacy kits** use the CS:GO-era model (`use_legacy_model`), which the demo's mesh groups already show; its
  material names its own composite inputs, in the layout the kit's pattern was painted for. Their composite
  materials roll no pattern offset or rotation (Fire Serpent's pattern sits the same at every seed; its wear and
  grunge move).

## Alternatives considered

- **OFAST's own version, checked against CS2 screenshots** — the user's first choice, the earlier bake (pattern
  rotated about the centre, paint worn where the wear passed the wear texture's resistance, grunge by wear alone,
  the pattern's colour as is). The decompiled shader showed each of these differently: placements turn about the
  layout's corner, the wear texture and the cavity together decide, the grunge also sits in recesses, a custom
  paint job's alpha is its durability, gunsmith paints metal parts their own way.
- **Driving Source 2 Viewer's window** to read each combo: not allowed for this (the user declined control of the
  app); its command line does not decompile shaders, so OFAST reads the combos itself and calls SPIRV-Cross.
- **Painting in Unreal** (a material per style reading the same textures): the same work in a graph, and every glTF
  reader but Unreal would see bare weapons.
- **Stickers baked into the colour**: CS2 draws them in the weapon's own material (five slots), over the paint;
  they wait for a decal of their own.

## Consequences

- Seen on Aurora vs FURIA's round 17 (2026-10-27): of the round's 28 skins, 27 painted in third and first person
  (Fire Serpents, Asiimov, Whiteout, Gungnir, Printstream; the Dopplers, Gamma Dopplers, Fades, Crimson Web, Hot
  Rod, Slaughter, Radiation Hazard…), in Unreal through the players' and viewmodels' glTFs; Royal Guard (an overlay)
  keeps its weapon's own finish. The round's players export in about 105 s.
- The Source 2 Viewer of October 2025 (15.0) cannot open CS2's shader files (format 72; it reads 59 – 70): 20.1
  (October 2026) can.
- Pearlescence, the paints' normal maps and AO overrides are not read yet.
- Each skin's colour and surface decode their inputs separately (the weapon's composite inputs and the shared wear
  and grunge textures again for each skin, and again for the viewmodel): fine at a round's 30 skins, a cache if it
  grows.
- Not painted yet: weapons lying in the world (their own glTFs), gloves, stickers, keychains, name tags, StatTrak
  counters.
- To check against CS2: the roll (whether a variable a kit does not define still takes its draw), which size scale
  the UV placements use (`g_flUvScale1`, or `g_flWeaponLength1 / 36`: the vertex stage declares both; OFAST takes
  the first, and the second for the projected patterns, whose own declarations name it), the look at each wear.
