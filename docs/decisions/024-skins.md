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
  roll with a uniform random stream seeded by the pattern seed, and each generated texture's steps (colour,
  roughness and metalness: copy a container's parameters, copy one, set one, each under its conditions), so a newer
  kit's own masks, colours and switches (`g_bUseOverlay`, `g_bUseMetalness`…) reach the shader as CS2 sends them.
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
- **The variants CS2's kits use**: overlays (on the five styles kits put one on: 70 kits), spray paint's halftone (16)
  and all-masks (37) variants, case hardening on solid, patina and gunsmith (23), read the same way (static combos
  576 + n, 1152 + n, 18 + 2).
- **Only what it paints right**: trilinear case hardening (1 kit) is not read yet, nor patina age, which no kit uses
  (the four that seemed to were the legacy patina's: research, "Patina age is unused"); such weapons keep their own
  finish, with a warning — a wrong skin looks worse than none.
- **Stickers baked into the weapon's textures** (`export::stickers`, 2026-10-28): CS2 draws them in the weapon's
  own shader (`csgo_weapon.vfx`, `F_STICKERS`, its static combo 512 decompiled the same way) in a second UV set;
  the bake draws the mesh's triangles into the texture's layout to find, at each texel, where the stickers are, then
  blends each slot's colour (fitted to CS2's albedo levels), roughness and metalness over the painted (or the
  weapon's own) colour and metallic-roughness. The slots come from the model's `StickerMarkup`, free placements
  shift them by the item's offset in sticker units. The view-dependent parts are fixed (holograms seen face on) or
  left out (glitter, the edges' parallax).
- **Keychains and name tags as parts of the weapon** (`export::keychains`, `export::nametags`): the charm model
  hangs from the item's point (or the model's `keychain` attachment) on the bone of the model's nearest keychain
  area, drawn with the definition's `keychain_material` when it names one (the community charms' blank model), a
  highlight reel as its `base` charm. Its joints are added to the skeleton and swing in the animations: a chain of
  rods from the ring under gravity with the cloth's hinge limits, moved by where the weapon takes the ring in the
  world (not CS2's cloth solver, whose nodes and constraints the model carries; a lying weapon's charm stays as at
  rest). The name tag plate sits on the `nametag` attachment,
  its colour baked with the name drawn as CS2's composite draws it. The text uses the Windows fonts CS2 names (Arial
  Narrow, with fallbacks for the characters it lacks) through `ab_glyph` (TrueType is a standard format: decision
  002). A left-handed viewmodel, mirrored whole as in CS2, carries a second plate mirrored along its length (CS2
  keeps the text readable there); each plate is on a bone of its own and the animation scales the other hand's to 0,
  since players switch hands during a round.
- **StatTrak modules with their digits set in the UVs** (`export::stattrak`): the module on the `stattrak`
  attachment, its digits' first UV set moved as CS2's vertex shader moves them for the count (decompiled). The
  count goes up during the round (the demo's attribute 80 on the kill's tick): a weapon carries one module per count
  it shows, each on a bone of its own, shown in turn by the animation's scales — the same switch as the name tag's
  hands, nothing new for Unreal. Weapons lying in the world carry their plate and module too.
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
- **Stickers as decals of their own** (the first plan: the triangles under each sticker copied into a mesh with the
  sticker's texture): the sticker texture's full resolution, but extra geometry on skinned weapons (weights to
  copy), the weapon's sticker mask is in the other UV set, and decals need a depth offset glTF has no word for.
  Baked, a sticker gets the weapon texture's resolution (2048 at most: on the MP9 about 200 texels across) and
  every glTF reader sees it.

## Consequences

- Seen on Aurora vs FURIA's round 17 (2026-10-27): all 28 of the round's skins painted in third and first person
  (Fire Serpents, Asiimov, Whiteout, Gungnir, Printstream; the Dopplers, Gamma Dopplers, Fades, Crimson Web, Hot
  Rod, Slaughter, Radiation Hazard; Royal Guard, an overlay), in Unreal through the players' and viewmodels' glTFs.
  The round's players export in about 100 s. Kits outside the round baked with `examples/paint_bake.rs` (Pink
  Tiger, Ghost Protocol, Djinn, a case-hardened Five-SeveN, halftone P2000, wax-rub Bizon…).
- Before the composite steps were read, newer kits got their template's grey colours and the weapon's own masks
  (Royal Guard was grey-blue, not blue and gold): their colours are `COLOR4` loose variables, their masks copied
  in by a conditional step.
- The Source 2 Viewer of October 2025 (15.0) cannot open CS2's shader files (format 72; it reads 59 – 70): 20.1
  (October 2026) can.
- Pearlescence, the paints' normal maps and AO overrides are not read yet.
- Each skin's colour and surface decode their inputs separately (the weapon's composite inputs and the shared wear
  and grunge textures again for each skin, and again for the viewmodel): fine at a round's 30 skins, a cache if it
  grows.
- Weapons lying in the world are painted too (their own glTFs, one per model and skin).
- Gloves are painted with their own shader (`csgo_customglove`, ported the same way: `export::gloves`): its default
  combo ("Source1 texture layout", the older kits) and its height-blended program (2025's kits: material layers
  with a surface over a substrate, tint ids, the pattern as tint or as paint, damage by height; `gloves::blended`),
  all 10 of round 17's glove skins. The height-blended program's colour-adjust matrices are the environment shaders'
  reconstruction (`environment::color_correct`, `color_tint`, from Source 2 Viewer's). Bakes run their rows in parallel: a glove skin
  takes about a second a hand.
- Not shown yet: stickers' glitter and view-dependent holograms; CS2's own cloth motion for the charms (OFAST's
  chain of rods stands in).
- **Compared with CS2** (2026-10-28, `-insecure`, the demo paused at the same ticks, `spec_player` in first person
  against the saved round's POV cameras: 20 views of Aurora vs FURIA's round 17): the weapon skins match (Asiimov,
  Fire Serpents, Wild Lotus, Bloodsport, Hot Rod, Radiation Hazard, Gungnir), the gloves' patterns sit on the right
  panels. Two bugs found and fixed: a glove pattern's region is `g_nPatternReplaceIndex − 1` (Crimson Kimono's was
  on the knuckle panel instead of the back of the hand), and newer kits' per-colour metalness and roughness are
  `FLOAT4` variables (Royal Guard's slide was matte). Left: stickers (FalleN's MP9), 2025's glove kits (yuurih's,
  soulfly's: ported since, below), and the metal knives' look, which follows each renderer's sky and haze (Unreal's deep blue sky turns a
  Sapphire karambit vivid where CS2's pale haze shows it near white) and the environment's grading (CS2's frames are
  less saturated throughout): a lighting question for the polish pass, not the bake.
- To check against CS2: the roll (whether a variable a kit does not define still takes its draw), which size scale
  the UV placements use (`g_flUvScale1`, or `g_flWeaponLength1 / 36`: the vertex stage declares both; OFAST takes
  the first, and the second for the projected patterns, whose own declarations name it), the look at each wear.
