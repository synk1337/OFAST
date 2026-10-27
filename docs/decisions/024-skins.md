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
  version once the decompiled shader turned out to be public): Valve's compiled `csgo_customweapon` as decompiled by
  Source 2 Viewer and tracked in SteamDatabase's GameTracking-CS2 (`csgo_customweapon.slang`), rewritten as
  OFAST's own Rust, as the tint and environment shaders were (decisions 007, 008, 011, 012).
- **Read like CS2** (`paint`): the paint kit from `items_game.txt`, its composite material and what it includes, the
  roll with a uniform random stream seeded by the pattern seed, and the parameters in the order the generator copies
  them (paint material, the weapon's composite inputs, the loose variables, the roll, seed and wear).
- **Baked in the export** (`export::paint`), per weapon material that names its `composite_inputs` and per skin:
  a colour texture and a metallic-roughness one in the weapon's UV layout, at the pattern's size (2048 at most), in
  the glTF (`TextureJob::PaintColor`, `PaintSurface`), so Unreal (and any glTF reader) shows the skin with no
  material of its own. Third person and first person both (the weapon looks carry the finish:
  `loadout::WeaponLook::finish`).
- **What the tracked file holds** is the vertex stage and the pixel stage's default variant (every static combo 0):
  the solid style with no overlay, case hardening or paint textures. From it, exact: the placements of pattern,
  wear and grunge (rounded to hundredths, the shader's own translation, the weapon's size scale on all three), where
  the paint is gone (wear texture × cavity, plus the no-paint mask, against `0.58 – 0.68` as the wear grows, per-mask
  durability and softness), the grunge (colour and roughness), the surface (roughness and metalness per mask), the
  solid colours and their albedo levels. The module docs of `export::paint` give the formulas.
- **Custom paint jobs and gunsmith** (styles 6, 8) are not in that variant: their pattern's colour takes the solid
  colours' place under the same shared maths, OFAST's reading until their own variants are read.
- **Only what it paints right**: solid, custom paint jobs and gunsmith, without overlays, case hardening or patina
  age. The others (hydrographic, spray paint, the three anodized: Dopplers, Fades; patina) colour masks their own
  way: their weapons keep their own finish, with a warning — a wrong skin looks worse than none.
- **Legacy kits** use the CS:GO-era model (`use_legacy_model`), which the demo's mesh groups already show; its
  material names its own composite inputs, in the layout the kit's pattern was painted for. Their composite
  materials roll no pattern offset or rotation (Fire Serpent's pattern sits the same at every seed; its wear and
  grunge move).

## Alternatives considered

- **OFAST's own version, checked against CS2 screenshots** — the user's first choice, the earlier bake (pattern
  rotated about the centre, paint worn where the wear passed the wear texture's resistance, grunge by wear alone).
  The decompiled shader showed each of these differently: placements turn about the layout's corner, the wear
  texture and the cavity together decide, the grunge also sits in recesses.
- **Painting in Unreal** (a material per style reading the same textures): the same work in a graph, and every glTF
  reader but Unreal would see bare weapons.
- **Stickers baked into the colour**: CS2 draws them in the weapon's own material (five slots), over the paint;
  they wait for a decal of their own.

## Consequences

- Seen on Aurora vs FURIA's round 17 (2026-10-27): of the round's 28 skins, 13 painted in third and first person
  (MAJ3R's and KSCERATO's Fire Serpents, YEKINDAR's Asiimov, KSCERATO's Whiteout, Wild Lotus, Bloodsport, Gungnir,
  Printstream, Kill Confirmed, Water Elemental, Autotronic…), in Unreal through the players' and viewmodels' glTFs;
  the 15 others (Dopplers, Fades, Crimson Web, Hot Rod, Royal Guard's overlay…) keep their weapon's own finish.
  The round's players export in about 75 s.
- **Dead end, for now**: the other styles' variants are in CS2's own shader files (`shaders_vulkan_dir.vpk`,
  `csgo_customweapon_vulkan_50_ps.vcs`), which the Source 2 Viewer on hand (15.0, October 2025) cannot open: CS2's
  files are format 72, it reads 59 – 70. The current release (the one GameTracking-CS2 runs) reads them, and
  decompiles any variant.
- Pearlescence, the paints' normal maps and AO overrides are not read yet.
- Each skin's colour and surface decode their inputs separately (the weapon's composite inputs and the shared wear
  and grunge textures again for each skin, and again for the viewmodel): fine at a round's 30 skins, a cache if it
  grows.
- Not painted yet: weapons lying in the world (their own glTFs), gloves, stickers, keychains, name tags, StatTrak
  counters.
- To check against CS2: the roll (whether a variable a kit does not define still takes its draw), which size scale
  the placements use (`g_flUvScale1`, or `g_flWeaponLength1 / 36`: the file declares both), the look at each wear.
