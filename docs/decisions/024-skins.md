# 024 — Skins: read from the demo, painted by OFAST's own bake

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

- **OFAST's own version of the paint styles first** (the user's choice, 2026-10-27), checked against CS2's own
  weapons; the exact maths of CS2's shaders may come in a later release.
- **Read like CS2** (`paint`): the paint kit from `items_game.txt`, its composite material and what it includes, the
  roll with a uniform random stream seeded by the pattern seed, and the parameters in the order the generator copies
  them (paint material, the weapon's composite inputs, the loose variables, the roll, seed and wear).
- **Baked in the export** (`export::paint`), per weapon material that names its `composite_inputs` and per skin:
  a colour texture and a metallic-roughness one in the weapon's UV layout, at the pattern's size (2048 at most), in
  the glTF (`TextureJob::PaintColor`, `PaintSurface`), so Unreal (and any glTF reader) shows the skin with no
  material of its own. Third person and first person both (the weapon looks carry the finish:
  `loadout::WeaponLook::finish`).
- **The bake's first reading** (module docs of `export::paint`): pattern at its placement; paint everywhere but the
  no-paint mask; the wear texture as the paint's resistance against the wear amount, edges (cavity) first, the
  weapon's own colour and metal where it is gone; grunge darkening with wear.
- **Only the styles it gets right**: custom paint jobs and gunsmith, which paint their pattern's colours. The
  others (solid, hydrographic, spray paint, the three anodized ones, patina) colour masks through colour maps
  (`g_vColor0` – `3`; Fade's pattern is a red–green–blue gradient): their weapons keep their own finish, with a
  warning, until the colour maps are read — a wrong skin looks worse than none.
- **Legacy kits** use the CS:GO-era model (`use_legacy_model`), which the demo's mesh groups already show; its
  material names its own composite inputs, in the layout the kit's pattern was painted for.

## Alternatives considered

- **CS2's shader maths first** — Valve's compiled shaders, as decompiled by Source 2 Viewer and tracked publicly in
  SteamDatabase's GameTracking-CS2 (`csgo_customweapon.slang` holds the paint styles' code). Kept for later; OFAST's
  tint and environment shaders were rebuilt from that source before (decisions 007, 008, 011, 012).
- **Painting in Unreal** (a material per style reading the same textures): the same work in a graph, and every glTF
  reader but Unreal would see bare weapons.
- **Stickers baked into the colour**: CS2 draws them in the weapon's own material (five slots), over the paint;
  they wait for a decal of their own.

## Consequences

- Seen on Aurora vs FURIA's round 17 (2026-10-27): of the round's 28 skins, 12 painted in third and first person
  (MAJ3R's and KSCERATO's Fire Serpents at minimal and field-tested wear, YEKINDAR's Asiimov, Wild Lotus,
  Bloodsport, Gungnir, Printstream, Kill Confirmed, Water Elemental, Autotronic…), in Unreal through the players'
  and viewmodels' glTFs; the 16 others (Dopplers, Fades, Crimson Web, Hot Rod…) keep their weapon's own finish.
  The round's players export in 70 s.
- Colour maps, case hardening, pearlescence, the paints' normal maps and AO overrides are not read yet.
- Each skin's colour and surface decode their inputs separately (the weapon's composite inputs and the shared wear
  and grunge textures again for each skin, and again for the viewmodel): fine at a round's 30 skins, a cache if it
  grows.
- Not painted yet: weapons lying in the world (their own glTFs), gloves, stickers, keychains, name tags, StatTrak
  counters.
- To check against CS2: the roll (whether a variable a kit does not define still takes its draw), the wear's look at
  each wear amount, the placements' rotation centre.
