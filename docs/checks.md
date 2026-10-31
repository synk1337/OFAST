# Map checks against CS2

The lighting was calibrated on a few views ([lighting.md](formats/lighting.md)): both spawns of each map and
de_mirage's palace and B apartments. This page checks many more spots of de_mirage, de_dust2, de_inferno and de_nuke
— bomb sites, main routes, interiors, dark corners and lower levels — against CS2, lists what was wrong and what was
done about it. First run 2026-10-17.

## How

- **Views**: `tools/compare/views/<map>.json`, 10 – 14 per map (name, CS2 `setpos` / `setang`, note). Picked from
  the maps' own place names (`env_cs_place`) and, on de_nuke, Valve's screenshot cameras (`point_devshot_camera`:
  `interiord` is the B-site view, `interiorc` A site, `interiorb` the lobby, `warehouse` the yard). Floors and lines of
  sight were checked with `map_raycast` before rendering ([development.md](development.md#debug-tools-for-maps)).
- **Unreal**: `shoot_views.py -Views tools/compare/views/<map>.json` on the imported level, or `import_map.py -Views`.
- **CS2**: tours written by `tools/cs2/make_view_cfgs.py` (`ofast_tour_<map>.cfg`: F8 next view, F9 re-apply),
  exposure locked at the value OFAST uses (the manifest's `post.min_exposure`: de_mirage 0.8, de_dust2 0.925,
  de_inferno 0.8, de_nuke 1.0), Steam F12 captures (2560 × 1440). de_nuke's views were also captured with
  `mat_fullbright 1` (albedo only) to tell light from material.
- **Comparison**: `tools/compare/compare_views.py --cs2-gamma 2.013074`: whole-image ratio of linear luminance,
  Unreal / CS2 (1.00 = the same brightness), and the darkest / brightest of the 4 × 3 cells. The references were taken
  at this machine's CS2 brightness setting (`r_fullscreen_gamma` 2.013074, default 2.2), which shows every pixel at
  the power 2.013 / 2.2 of CS2's default picture (measured by the tone-fit session on de_vertigo, 2026-10-17);
  `--cs2-gamma` turns them back into the default picture first. The older tables in lighting.md were measured
  against the uncorrected, brighter references: corrected, de_mirage's palace and B apartments read 1.07 and 0.90,
  uncorrected 0.96 and 0.76.

Pictures (local only, `test-data/shots/view_checks/`): `cs2/` the references, `before/` and `after/` the Unreal
renders, `compare_*` the side-by-side images.

## Results

Unreal / CS2, whole image (1.00 = as bright as CS2's default picture), before and after the fixes below; the cell
range is the darkest and brightest of the 12 cells after. **Before** is the Unreal levels as other sessions had last
imported them (de_mirage on 2026-10-16, the others on the 17th). **After**: de_mirage, de_dust2 and de_inferno
re-imported with this branch (`test-data/exports_vc`); de_nuke rendered on its existing level with the new
shadow-map limits and the radius change as a what-if (`attenuation_radius × 1.375`, the factor for skirt 0, which
most of its lights have), because other sessions were working on de_nuke — its bounce-only lights and start-disabled
entities are not in these numbers.

**de_mirage** (exposure 0.8)

| View | Where | Before | After | Cells after | Notes |
|---|---|---|---|---|---|
| ct0 / t0 | spawns (references of 2026-10-16) | 1.06 / 1.04 | 1.05 / 1.04 | 0.92 – 1.17 | |
| palace | palace interior, wall lights | 1.07 | 1.07 | 0.90 – 1.27 | |
| apps | B apartments corridor | 0.90 | 0.90 | 0.73 – 1.05 | bounce from the sunlit rooms still short |
| asite | A site, sunlit | 1.03 | 1.03 | 0.91 – 1.23 | |
| stairs | A-site stairs over the site | 1.03 | 1.04 | 0.98 – 1.09 | |
| bsite | B site, sunlit | 0.93 | 0.93 | 0.77 – 1.04 | |
| bapps | B apartments balcony over B | 0.88 | 0.88 | 0.72 – 1.13 | |
| middle | middle | 0.99 | 0.99 | 0.94 – 1.03 | |
| connector | connector, wall lamp | 1.31 | 1.31 | 1.04 – 1.54 | 9 |
| sniper | sniper's nest room | 1.30 | 1.30 | 0.93 – 1.59 | 9 |
| shop | shop interior, dark | 0.73 | 0.79 | 0.44 – 0.98 | 11 |
| tramp | T ramp stairs | 1.21 | 1.15 | 0.99 – 1.32 | 9 |
| underpass | underpass | 1.37 | 1.36 | 1.10 – 1.74 | 9 |

**de_dust2** (exposure 0.925)

| View | Where | Before | After | Cells after | Notes |
|---|---|---|---|---|---|
| ct0 / t0 | spawns (references of 2026-10-16) | 0.11 / 1.02 | 0.31 / 1.03 | 0.10 – 1.16 | CT room: 11 |
| asite | A site | 0.96 | 0.96 | 0.79 – 1.06 | |
| longa | long A | 1.02 | 1.02 | 0.96 – 1.23 | |
| pit | pit | 1.31 | 1.01 | 0.65 – 1.36 | bounce-only light by the wall (2) |
| outsidelong | outside long | 1.03 | 1.03 | 0.92 – 1.09 | |
| bsite | B site | 1.21 | 1.14 | 0.96 – 1.46 | |
| bwindow | B window over the site | 1.15 | 1.12 | 0.92 – 1.31 | |
| uppertunnel | upper tunnels, inside | 0.88 | 1.00 | 0.88 – 1.09 | |
| uppertunnel2 | upper tunnels' exit to B | 1.23 | 1.18 | 1.04 – 1.37 | |
| lowertunnel | lower tunnels | 0.32 | 0.68 | 0.31 – 1.35 | lamp now reaches the floor (1); its bulb does not glow (6) |
| middle | middle | 1.09 | 1.09 | 0.99 – 1.28 | |
| middoors | past mid doors | 1.21 | 1.18 | 1.06 – 1.32 | |
| undera | CT side street to A | 0.97 | 0.98 | 0.94 – 1.02 | |

**de_inferno** (exposure 0.8, Hable curve)

| View | Where | Before | After | Cells after | Notes |
|---|---|---|---|---|---|
| ct0 / t0 | spawns (references of 2026-10-16) | 0.90 / 1.73 | 0.92 / 1.81 | 0.72 – 2.67 | T: pillar and floor by the arch sunlit (lighting.md) |
| asite | A site | 1.09 | 1.06 | 0.90 – 1.60 | |
| bsite | B site | 0.99 | 1.00 | 0.87 – 1.18 | |
| banana | banana | 1.64 | 1.66 | 1.05 – 2.90 | shaded walls too bright (12) |
| arch | arch courtyard | 0.84 | 0.84 | 0.36 – 1.09 | |
| library | library interior | 0.72 | 1.09 | 0.64 – 2.10 | (1) |
| apartments | apartments room, wall lamp | 1.23 | 2.18 | 1.19 – 4.38 | 9; window glass renders opaque white (12) |
| apartments2 | T-side apartments hall | 1.75 | 1.75 | 1.18 – 2.32 | 9 |
| middle | middle | 1.10 | 1.10 | 1.04 – 1.16 | |
| underpass | sewer underpass | 2.86 | 2.87 | 1.98 – 4.96 | 9 |
| pit | pit | 0.98 | 0.97 | 0.74 – 1.15 | |
| ruins | church by B | 2.60 | 2.79 | 1.09 – 4.76 | 9; stained glass grey (6) |
| balcony | balcony over second mid | 1.03 | 1.03 | 0.81 – 1.14 | |

**de_nuke** (exposure 1.0; after = what-if, see above)

| View | Where | Before | After | Cells after | Notes |
|---|---|---|---|---|---|
| ct0 / t0 | spawns (references of 2026-10-16) | 1.00 / 0.98 | 1.08 / 1.03 | 0.81 – 1.27 | |
| bsite | B site (lower level) | 0.50 | 1.00 | 0.46 – 1.88 | (1, 3); reactor pit still dark, blue patch from below (7) |
| bsite2 | B site, far corner | 0.42 | 0.58 | 0.03 – 1.84 | same (7) |
| observation | observation room by B | 1.56 | 1.76 | 1.01 – 3.80 | 12 |
| decon | decontamination room (lowest) | 0.24 | 0.83 | 0.31 – 2.80 | (1) |
| decondoor | lower tunnels at decon | 0.63 | 0.90 | 0.47 – 1.54 | (1) |
| tunnels | lower tunnels | 1.64 | 2.30 | 1.27 – 6.35 | door stands open (5) |
| secret | secret (lower) | 1.08 | 1.25 | 0.87 – 1.59 | |
| ramproom | ramp room above B | 1.29 | 1.42 | 0.93 – 2.54 | |
| asite | A site | 0.24 | 0.27 | 0.10 – 0.55 | 8 |
| lobby | lobby | 0.51 | 0.69 | 0.38 – 1.03 | |
| outside | yard, sunlit | 1.02 | 1.02 | 0.85 – 1.50 | power cables missing (10) |
| garage | garage | 0.85 | 1.38 | 0.81 – 2.67 | black ceiling (12) |

Most sunlit outdoor views are within 0.93 – 1.10; de_dust2's B site and mid-doors area read 1.12 – 1.18 (shade
brighter than CS2's), de_mirage's B balcony 0.88 and de_inferno's banana 1.66. What remains is mostly interiors: the
views that were far too dark from short-ranged lamps came up (de_nuke's lower level, de_dust2's tunnels, de_inferno's
library), and interiors lit mostly by bounce or by generous lamps are now more often too bright than too dark — see 9.

## After merging all branches (2026-10-18)

Every branch merged into `main` (overlays as mesh decals, static overlay blend modes, per-pixel translucency, tint
masks, the tone curve measured on de_anubis — H(3.4 x) / H(6 W), the colour table on Hable maps only — and this page's
fixes), the five maps re-exported and imported with all their views, compared the same way (`--cs2-gamma 2.013074`;
renders in `test-data/shots/merged/`). Nearly every view reads within 0.05 of the "After" columns above. What moved:

| Map | View | After (above) | Merged | Refit level | Why |
|---|---|---|---|---|---|
| de_inferno | ct0 / t0 | 0.92 / 1.81 | 0.81 / 1.44 | 0.75 / 1.30 | the new tone curve (the old H(2.5 x) / H(1.25 W) was fitted on these surfaces) |
| de_inferno | asite / bsite / middle / pit / balcony | 1.06 / 1.00 / 1.10 / 0.97 / 1.03 | 0.92 / 0.86 / 0.93 / 0.82 / 0.86 | 0.86 / 0.79 / 0.85 / 0.76 / 0.79 | same; de_inferno's base colours are 0.60× CS2's albedo (lighting.md) |
| de_inferno | apartments / apartments2 / underpass / ruins / banana | 2.18 / 1.75 / 2.87 / 2.79 / 1.66 | 1.93 / 1.30 / 2.38 / 2.08 / 1.44 | 1.49 / 1.18 / 2.15 / 1.85 / 1.32 | same; still item 9 |
| de_inferno | arch / library | 0.84 / 1.09 | 0.64 / 0.75 | 0.57 / 0.65 | same |
| de_nuke | bsite | 1.00 (what-if) | 0.85 | — | the fixes for real (bounce-only lights, start-disabled entities); still patchy: items 6 and 7 |
| de_nuke | observation / tunnels | 1.76 / 2.30 | 1.66 / 2.17 | — | same |
| de_mirage | underpass / shop | 1.36 / 0.79 | 1.42 / 0.82 | — | |
| de_anubis | ct0 / t0 | — | 1.11 / 1.08 | 1.04 / 1.02 | the merged curve was fitted on references at the brighter setting (1.01 / 1.01 against them) |

**Refit level** (2026-10-18): the Hable level measured again on CS2's default picture with `tools/compare/tone_fit.py`
— H(2.85 x) / H(4.2 W) instead of H(3.4 x) / H(6 W) ([lighting.md](formats/lighting.md#non-linear-hable-curves)) —
and de_anubis and de_inferno re-imported with it (renders in `test-data/shots/refit/`). The linear maps (de_mirage,
de_dust2, de_nuke) do not use it. With the level right, de_inferno's sunlit views read 0.76 – 0.86: what remains is
its materials, and its bright interiors (item 9) come down by 10 – 25 %.

New views without a CS2 reference yet: de_dust2 `dumpster`, de_inferno `trike`, de_nuke `ctvan`. de_dust2's tint
views read 1.18 – 1.26 (sunlit, as the tint checks found).

Two of the import's own screenshots were wrong: de_mirage's `underpass` file held the CT-spawn view (and `ct0` was
missing), and de_dust2's `uppertunnel` came out at 0.88. `shoot_views.py` on the saved levels gave the right
pictures (1.42; 0.99 and 1.01 in two runs), which the table uses. The import's screenshot loop can therefore drop a
shot or catch one early; re-shoot with `shoot_views.py` when a number looks off.

### Colour table on every map (2026-10-18)

The colour-correction table applies on the linear maps too again (CS2 applies it there, measured on de_nuke at the
default brightness: [lighting.md](formats/lighting.md#the-colour-correction-table)). de_nuke, de_dust2 and de_mirage
re-imported with all their views (`test-data/shots/lut_all/`; all 53 screenshots right this time):

| Map | Spawns (ct0 / t0) | Views, without → with the table |
|---|---|---|
| de_nuke | 1.07 / 1.03 → 1.00 / 0.97 | sunlit closer to 1 (ctcrates 1.12 → 1.04, tdoors 1.11 → 1.01, outside 1.00 → 0.95); the table lowers the darks, so dim interiors drop (bsite 0.85 → 0.76, lobby 0.70 → 0.61, decon 0.82 → 0.76) and bright ones too (tunnels 2.17 → 2.05, garage 1.38 → 1.30) |
| de_dust2 | 0.31 / 1.05 → 0.28 / 1.06 | within 0.05 (lower tunnel 0.68 → 0.63) |
| de_mirage | 1.05 / 1.05 → 1.05 / 1.06 | within 0.05 (B apartments 0.90 → 0.85, underpass 1.42 → 1.29) |

de_nuke's dim interiors (B site, lobby, decontamination) now sit lower; what keeps them dark is still items 6 and 7
(unlit lamps and screens, the lights below the floor).

### Light and material against the default picture (2026-10-18)

de_mirage's lighting calibration measured again against CS2's default picture, with the material taken out
([lighting.md](formats/lighting.md#against-cs2s-default-picture-2026-10-18), method and what-ifs there). Unreal /
CS2 of the scene on the `lut_all` import, re-shot on the saved level, plus a `ShowFlag.Lighting 0` render of every view:
**sunlit** blocks (Unreal light 1.5 units or more) measure the material, since the direct sun is the same formula in
both engines; **shade** (0.15 – 0.9) and indoor (**lamp**) blocks the light × material, so their ratio to the sunlit
one is the light alone (indoors: against the map's median sunlit 1.07). **Pairs**: neighbouring shaded and sunlit
blocks of the same Unreal base colour, away from shadow edges.

| View | Whole image | Sunlit | Shade / lamp | Light relative to the sun | Pairs |
|---|---|---|---|---|---|
| ct0 / t0 | 1.05 / 1.06 | 1.07 / 1.07 | 1.02 / 1.05 | 0.95 / 0.99 | 0.93 / 0.96 |
| asite | 1.07 | 1.10 | 1.03 | 0.94 | too few |
| stairs | 1.04 | 1.09 | 1.01 | 0.92 | 0.90 |
| middle | 0.99 | 1.03 | 0.91 | 0.88 | 0.92 |
| bsite | 0.93 | 1.07 | 0.78 | 0.73 | 0.82 |
| bapps | 0.93 | 1.03 | 0.78 | 0.75 | 0.89 |
| palace | 1.06 | — | 1.05 | 0.98 | — |
| apps | 0.85 | — | 0.84 | 0.78 | — |
| connector / sniper | 1.29 / 1.28 | — | 1.25 / 1.26 | 1.17 / 1.18 | — |
| shop | 0.79 | — | 0.74 | 0.69 | — |
| tramp | 1.10 | — | 0.84 – 1.09 | 0.79 – 1.02 | — |
| underpass | 1.29 | — | 1.29 | 1.21 | — |

Every outdoor view's sunlit surfaces read 1.03 – 1.10, which looked like Unreal's base colours being about 7 % above
CS2's lit albedo (de_anubis: 1.08). It was Unreal's highlights of baked lights, which CS2 does not draw ("Highlights
of baked lights" below), and that, not the bounce light, is most of what reads bright in the sunlit views. The calibration views'
light is within 1 – 7 % of CS2's relative to the sun, so `OUTDOOR_BOUNCE_GAIN` 1.25, `LOCAL_BOUNCE_GAIN` 2.0 and the
rect-light halving stay. `mat_fullbright` could not be used for this on de_mirage: it shows 1.3 – 1.55× the albedo
the lit shader uses (Unreal's base colour 0.65× it in the CT spawn, CS2's lit ÷ fullbright on sunlit cobbles below
the direct sun alone; lighting.md).

The T ramp (`tramp`) varies from one editor session to the next: 1.10 (the import's shot), 1.15 – 1.17 in two later
runs on the saved level, 0.84 in a third that shot every view seven times (the same in all seven). In that session
the other views agreed with the import's shots within 1 % (the dark shop 3 %); the cause was not found.

### Highlights of baked lights (2026-10-18)

Why the sunlit surfaces read 7 % bright, stage by stage ([lighting.md](formats/lighting.md#highlights-cs2-does-not-draw-2026-10-18)
has the method and the evidence):
- **Colour textures** (exporter): OFAST's decoded colours average what CS2 stored as each texture's average (de_mirage's
  348 colour textures: median 1.0065). Not the cause.
- **Base colours in Unreal**: de_anubis's base colours are right (0.99 / 0.97 of CS2's `mat_fullbright`). Its 1.08
  was the base-colour render: `ShowFlag.Lighting 0` adds 0.45 × the specular colour; base-colour renders now also
  set `ShowFlag.Specular 0`.
- **The sun's highlight**: CS2 bakes de_mirage's sun (and nearly every official map's) into its lightmaps, which
  draw no highlight; Unreal's sun drew one. With the sun's Specular Scale at 0 the sunlit blocks read 0.97 – 1.04
  (1.03 – 1.10 before); across 5800 sunlit blocks Unreal / CS2 grew with the highlight's share exactly as if CS2 had
  none (slope 0.92; −0.08 without it). Baked local lights draw none either, unless Hammer bakes their highlight into
  the cubemaps. **Fixed**: the manifest says how CS2 draws each light (`direct` on the sun; `bake_specular`,
  `bake_specular_scale` on local lights) and `import_map.py` sets each light's Specular Scale to match.
- Interchange's glTF master reflected with F0 0.08 where CS2 (and OFAST's own masters) use 0.04. **Fixed** the same
  day (`set_gltf_specular`: `SpecularFactor` 0.5; F0 0.039 in every view after the re-import): sunlit pooled
  1.014 → 1.001, CT / T / palace 1.02 / 1.03 / 1.01 → 1.00 / 1.01 / 0.97, connector / sniper 1.20 / 1.23 → 1.14 /
  1.16, underpass 1.28 → 1.22, the dark B site / B apartments / shop 2 – 4 % darker; relative to the sun CT shade
  0.94, T shade 1.02, palace 0.97, bounce gains unchanged
  ([lighting.md](formats/lighting.md#reflections-of-interchanges-gltf-master-2026-10-18)).

de_mirage re-exported (the manifest only: glTF, binary and textures came out byte-identical) and re-imported with the
fix; the import's own shots, all 15 right this time, against the `lut_all` import's (Unreal / CS2 of the scene, same
method as the table above; "all" is every non-sky block):

| View | All, before → after | Sunlit | Shade / lamp | Light relative to the sun (sunlit 1.01) |
|---|---|---|---|---|
| ct0 / t0 | 1.05 / 1.05 → 1.02 / 1.04 | 1.07 / 1.05 → 1.02 / 1.00 | 1.00 / 1.06 → 0.99 / 1.05 | 0.97 / 1.05 |
| asite / stairs | 1.06 / 1.05 → 1.03 / 1.01 | 1.10 / 1.10 → 1.04 / 1.03 | 1.02 / 1.01 → 1.01 / 1.00 | 0.97 / 0.97 |
| bsite / bapps | 0.86 / 0.85 → 0.83 / 0.83 | 1.06 / 1.03 → 0.99 / 0.96 | 0.78 / 0.77 → 0.77 / 0.76 | 0.77 / 0.79 |
| middle | 0.96 → 0.94 | 1.04 → 0.98 | 0.90 → 0.90 | 0.91 |
| palace / apps | 1.05 / 0.84 → 1.01 / 0.75 | — | same | 1.00 / 0.74 |
| connector / sniper | 1.26 / 1.27 → 1.20 / 1.22 | — | same | 1.19 / 1.21 |
| shop / tramp / underpass | 0.71 / 1.09 / 1.28 → 0.70 / 1.08 / 1.28 | — | same | 0.69 / 1.07 / 1.26 |
| all seven outdoor views, pooled | | 1.07 → **1.01** | | |

### The highlight rule on the other maps (2026-10-18)

de_anubis, de_dust2, de_inferno, de_nuke and de_vertigo re-exported for the new manifest keys and re-imported with the
rule; the importer also has the barn-light work (de_vertigo's export and level already had it), so de_vertigo keeps
its cookie spots. de_dust2's and de_vertigo's exports came out byte-identical but for the manifest, which only gained
keys (replaced in `test-data/exports`). de_anubis's, de_inferno's and de_nuke's glTF differed as well — their exports
predated the unlit `black_simple` materials (`KHR_materials_unlit`, 1 – 3 materials each) — so their new exports are
in `test-data/exports_hl/` and the shared ones were left as they were. Every map's views were shot on the level as
other sessions had left it (**before**), the map re-imported and shot again (**after**), and in the same editor
session once more with Unreal's highlights put back on the sun and every light that is not bounce-only (**rule
alone** = after ÷ that, whole image). Unreal / CS2 against the default picture (`--cs2-gamma 2.013074`): the whole
image as in the tables above, and the scene (each picture read back through its tone rule: k 2.85 / w 4.2 on the
Hable maps, the colour table on every map) over sunlit and outdoor-shade blocks as in the de_mirage table (views with
fewer than 30 sunlit blocks have neither). Renders, numbers and the scratch scripts that made them (local only):
`test-data/shots/highlights_maps/`.

| | de_anubis | de_dust2 | de_inferno | de_mirage | de_nuke | de_vertigo |
|---|---|---|---|---|---|---|
| sun | baked | baked | baked | baked | baked | baked |
| local lights: own highlight (stationary / dynamic) / baked into cubemaps / baked, none / bounce only | 1 / 14 / 5 / 13 | 2 / 1 / 27 / 1 | 6 / 22 / 162 / 17 | 0 / 0 / 66 / 6 | 15 / 97 / 56 / 6 | 3 / 24 / 62 / 0 |
| spawns CT / T, whole image | 1.04 / 1.02 → 1.01 / 1.01 | 0.28 / 1.06 → 0.28 / 1.03 | 0.79 / 1.41 → 0.77 / 1.37 | 1.05 / 1.05 → 1.02 / 1.04 (scene) | 1.00 / 0.97 → 0.93 / 0.92 | 0.82 / 1.61 → 0.82 / 1.48 |
| sunlit, all views pooled | 1.06 → **1.01** | 1.18 → **1.13** | 0.97 → **0.91** | 1.07 → **1.01** | 1.24 → **1.11** | 1.16 → **1.07** |
| rule alone on the sunlit blocks | −5 % | −4 % | −6 % | −6 % | −11 % | −7 % |
| outdoor shade, per view | 0 % | 0 – −3 % | −2 – +1 %; A site −7 % (the rule −1 %, the rest between editor sessions) | 0 – −1 % | 0 – −4 % | −1 / −6 % |
| lamp-lit interiors, per view | — | 0 – −3 % | 0 – −8 % (church −8 %) | 0 – −11 % (palace −4 %, B apartments −11 %) | 0 – −1 %; B site's far corner −6 %, ramp room −16 % | — |

- **Sunlit surfaces** lose the sun's highlight everywhere: 4 – 7 %, de_nuke 11 % (more of its sunlit light was
  Unreal's highlight). de_anubis now reads 1.01 like de_mirage. de_dust2 (1.13) and de_nuke
  (1.11) stay above CS2 and de_inferno below (its materials, base colour 0.6× CS2's albedo, lighting.md): not the
  highlight, open.
- **Shade** hardly moves (Lumen's reflections stay; the bounce gains were not touched), except where baked lamps
  without a cubemap highlight light it: de_vertigo's T-spawn hall (its 36 static barn lamps) 1.73 → 1.63.
- **Interiors** follow their lamps: de_nuke bakes 97 of its 153 baked lamps' highlights into the cubemaps, and its
  rooms keep them (decontamination, lobby, observation, tunnels within 1 %); the ramp room, where 15 of the 37 lamps
  within 15 m bake none, drops 16 %. de_inferno bakes few (22 of 184): its church, T-side apartments hall and
  library lose 5 – 8 %, the sewer underpass, lit through its openings, nothing. Its apartment room with the wall
  lamp read 1.51 – 1.79 in three shots of the same re-imported level (1.53 every time with the highlights back,
  2.05 before): too unsettled to measure the rule on.
- **Repeatability**: de_inferno's after and highlights-back shots were taken three times each (one editor session,
  a throw-away shot after each switch); the other views agreed to 0.5 % (the T-side apartments hall, banana and
  A site to 2 %), and its table gives their means.
- **de_nuke's B site** moved for another reason: the two stationary barn lights with a cookie are now cookie spots
  (the barn-light work), one 7 m from the B-site camera and one near `secret` and `garage`. B site 0.78 → 0.63,
  B site far corner 0.49 → 0.41 and secret 1.14 → 1.18 of which the rule is −1 %, −6 % and −1 %.

Per view, before → after (Unreal / CS2; the rule alone from the whole image):

**de_anubis** (exposure 0.7, Hable)

| View | Whole image | Sunlit | Shade | Rule alone |
|---|---|---|---|---|
| ct0 | 1.04 → 1.01 | 1.07 → 0.99 | 0.97 → 0.96 | −2 % |
| t0 | 1.02 → 1.01 | 1.06 → 1.03 | 0.99 → 0.98 | −1 % |

**de_dust2** (exposure 0.925)

| View | Whole image | Sunlit | Shade | Rule alone |
|---|---|---|---|---|
| ct0 | 0.28 → 0.28 | — | — | −1 % |
| t0 | 1.06 → 1.03 | 1.11 → 1.05 | 0.83 → 0.83 | −3 % |
| asite | 1.00 → 0.97 | 1.10 → 1.05 | 0.80 → 0.78 | −2 % |
| bsite | 1.16 → 1.13 | 1.20 → 1.12 | 1.17 → 1.16 | −3 % |
| bwindow | 1.14 → 1.10 | 1.19 → 1.13 | 1.04 → 1.03 | −3 % |
| longa | 1.05 → 1.02 | 1.14 → 1.08 | 0.78 → 0.77 | −3 % |
| lowertunnel | 0.63 → 0.63 | — | — | 0 % |
| middle | 1.10 → 1.07 | 1.16 → 1.09 | 1.06 → 1.06 | −3 % |
| middoors | 1.18 → 1.15 | 1.21 → 1.16 | 1.15 → 1.15 | −3 % |
| outsidelong | 1.05 → 1.02 | 1.11 → 1.07 | 0.78 → 0.77 | −3 % |
| pit | 1.02 → 1.01 | 1.15 → 1.08 | 0.97 → 0.97 | −1 % |
| tbluedoor | 1.27 → 1.24 | 1.30 → 1.25 | 1.17 → 1.16 | −2 % |
| tcar | 1.28 → 1.25 | 1.35 → 1.30 | 0.84 → 0.83 | −2 % |
| tdoor | 1.25 → 1.22 | 1.36 → 1.29 | 1.05 → 1.05 | −2 % |
| tsalmon | 1.19 → 1.17 | 1.24 → 1.18 | 1.12 → 1.11 | −2 % |
| undera | 0.98 → 0.97 | 1.08 → 1.04 | 0.92 → 0.92 | −1 % |
| uppertunnel | 0.93 → 0.92 | — | — | −3 % |
| uppertunnel2 | 1.14 → 1.13 | 1.23 → 1.18 | 1.13 → 1.13 | −1 % |

**de_inferno** (exposure 0.8, Hable; after and rule alone: means of three shots)

| View | Whole image | Sunlit | Shade | Rule alone |
|---|---|---|---|---|
| ct0 | 0.79 → 0.77 | 0.76 → 0.72 | 0.45 → 0.45 | −3 % |
| t0 | 1.41 → 1.37 | 2.64 → 2.46 | 1.19 → 1.17 | −3 % |
| asite | 0.86 → 0.80 | 0.88 → 0.78 | 0.64 → 0.59 | −5 % |
| bsite | 0.82 → 0.80 | 0.92 → 0.87 | 0.60 → 0.60 | −3 % |
| middle | 0.85 → 0.82 | 0.94 → 0.85 | 0.75 → 0.75 | −4 % |
| pit | 0.76 → 0.73 | 0.93 → 0.83 | 0.53 → 0.53 | −5 % |
| balcony | 0.78 → 0.77 | 0.81 → 0.77 | 0.59 → 0.60 | −1 % |
| arch | 0.57 → 0.56 | 1.10 → 0.99 | 0.49 → 0.48 | −3 % |
| banana | 1.31 → 1.28 | 1.96 → 1.78 | 1.08 → 1.06 | −2 % |
| ruins (church) | 1.86 → 1.72 | 3.80 → 3.39 | 1.79 → 1.67 | −8 % |
| apartments2 | 1.16 → 1.11 | 1.13 → 0.98 | 1.14 → 1.12 | −6 % |
| library | 0.49 → 0.47 | 1.31 → 1.08 | 0.54 → 0.52 | −5 % |
| underpass | 2.16 → 2.16 | 2.60 → 2.60 | 1.48 → 1.48 | 0 % |
| apartments | 2.05 → 1.51 – 1.79 | — | — | unsettled (see above) |

**de_nuke** (exposure 1.0)

| View | Whole image | Sunlit | Shade | Rule alone |
|---|---|---|---|---|
| ct0 | 1.00 → 0.93 | 1.17 → 0.97 | 0.87 → 0.85 | −8 % |
| t0 | 0.97 → 0.92 | 1.15 → 1.06 | 0.88 → 0.87 | −5 % |
| ctcrates | 1.04 → 1.01 | 1.17 → 1.06 | 0.91 → 0.89 | −3 % |
| tdoors | 1.01 → 0.96 | 1.17 → 0.98 | 0.94 → 0.93 | −5 % |
| outside | 0.96 → 0.92 | 1.11 → 1.01 | 0.69 → 0.68 | −4 % |
| garage | 1.30 → 1.26 | 1.90 → 1.80 | 0.90 → 0.87 | −3 % |
| asite | 0.23 → 0.23 | — | — | −1 % |
| bsite | 0.78 → 0.63 | — | — | −1 % (cookie spot) |
| bsite2 | 0.49 → 0.41 | — | — | −6 % (cookie spot) |
| secret | 1.14 → 1.18 | — | — | −1 % (cookie spot) |
| ramproom | 1.35 → 1.13 | — | — | −16 % |
| observation | 1.65 → 1.65 | — | — | −1 % |
| decon | 0.77 → 0.77 | — | — | 0 % |
| decondoor | 0.82 → 0.82 | — | — | 0 % |
| lobby | 0.60 → 0.60 | — | — | 0 % |
| tunnels | 2.05 → 2.03 | 2.22 → 2.21 | 1.65 → 1.63 | 0 % |

**de_vertigo** (exposure 0.8, Hable)

| View | Whole image | Sunlit | Shade | Rule alone |
|---|---|---|---|---|
| ct0 | 0.82 → 0.82 | 1.04 → 1.00 | 0.77 → 0.76 | −1 % |
| t0 | 1.61 → 1.48 | 1.19 → 1.09 | 1.73 → 1.63 | −8 % |

## Colour against CS2 (2026-10-30)

The question: is Unreal's picture more saturated than CS2's, as round 17's first-person views suggested (FalleN's MP9
at tick 152488, "red paint 0.80 against 0.67")? Measured with `tools/compare/colour_balance.py` on the reference
views of the three linear maps, shot again with the current code (`shoot_views.py`), CS2's references brought back
to the default picture (`--cs2-gamma 2.013074`). Per 16 × 9 cell, Unreal over CS2: colour balance (R/G, B/G) and
brightness in the sun (CS2 luminance > 0.25) and the shade (< 0.12), and saturation (chroma over luminance):

| Map | Sun R/G, B/G, brightness | Shade R/G, B/G, brightness | Saturation |
|---|---|---|---|
| de_dust2 (12 views) | 1.01, 0.99, 1.06 | 1.02, 0.97, 1.00 | 1.05 |
| de_mirage (15) | 1.02, 1.00, 0.99 | 0.98, 1.06, 0.82 | 1.00 |
| de_nuke (18) | 1.01, 0.96, 0.87 | 0.99, 1.01, 0.82 | 0.97 |

No: the sunlit surfaces of all three match CS2 within 1 – 2 % in colour, and the saturation within 5 %. What
differs is the shade, and not the same way on each map: de_dust2's is 2 % warmer (woxic's passage at tick 153201:
6 % warmer, 12 % less blue, the ceiling brown where CS2's is bluish grey), de_mirage's 6 % bluer and 18 % dark,
de_nuke's interiors dark (its open items above: A site, the lamps' self-illumination). The cool colours read a little
pale on de_dust2 (saturation 0.92 on blue cells, warm ones 1.06).

A fit of the shade's light was tried: renders with the sun's bounce off and with the sky light off split it into
direct light, sun bounce and sky light, and gains on the two were fitted to CS2's cells (all three channels). On
de_dust2 alone it asks for a fifth less sun bounce (0.87 ×, sky 0.98 ×); de_mirage wants the opposite in colour; over
the three maps 0.88 × and 1.07 × move every map by 1 – 2 % only. `OUTDOOR_BOUNCE_GAIN` stays: the shade is a per-map
lighting question (the open items), not the picture's colour.

## Blood against CS2 (2026-10-30)

Round 17 of Aurora vs FURIA on de_dust2, through MAJ3R's eyes: his two body hits on yuurih (ticks 153346 and 153353,
medium blood: 27 and 28 damage), his headshot on him (153359: the helmet took it, low blood and the helmet's sparks)
and his two hits on molodoy (154511, 154517, medium), 12 frames 1 – 7 ticks after the hits, CS2 (`tools/cs2/pov_shots.py`
at the default brightness, the tick read off the demo bar) against Unreal (`shoot_sequence.py -PovShots` at that tick).

- **Which blood**: right on all of them; the demo's blood dispatch carries the damage CS2 picks by (`export::shots`).
- **Helmet sparks**: CS2's white rays fan up from the head with a white point for 5 – 7 ticks; Unreal's the same in
  number, spread, length and time, a little thinner.
- **Direction**: CS2's bright red stays on the player hit, and its dark streaks go to the left of them on the
  shooter's screen, a little up, for both shooting directions (north at yuurih, east at molodoy); Unreal's now the
  same. Two wrong tries on the way: everything along the bullet, where the streaks hid in depth; everything to the
  left, which sat the splashes on the crate beside yuurih.
- **Size and reach**: one tick after a hit CS2's dark streaks reach 25 – 35 units from the wound, 35 – 45 two or three
  ticks later; Unreal's about half that. CS2's red glow round the head is denser.
- **In the shade** (molodoy, 154514 – 154520) CS2's near-black streaks show against a lit ground; Unreal's ground there
  is dark (the per-map shade, above) and they hardly show.
- Not drawn: the streaks' decals on the ground (`…_ground_decal`), CS2's bloom round the sparks.
- **Colour** (2026-10-31): Unreal's splashes were a pink-red (143, 49, 39 of 255) beside near-black streaks, unlit and
  drawn twice as bright as their tint; CS2's blood is lit (its renderers keep their default diffuse lighting, its
  streaks and mists are tinted by the light), one red from the splash to the streaks, (186, 66, 34) in the sun. Lit
  now in CS2's own gradients: (177, 26, 19) on yuurih at 153356 – 153362, the streaks the same red darker. CS2's is a
  little more orange, its red cloud wider (the sunlit wall shows through its edges).

## HE blast, tick by tick (2026-10-30)

Three HE grenades thrown in an offline game on de_dust2 (CS2 `-insecure`, recorded with GOTV: `tv_record`), each
filmed one tick at a time from the thrower's eyes (`tools/cs2/tick_shots.py`, 90 frames from the tick before the
blast): mid (about 510 units away), A long in the sun (about 250, close up), the lower B tunnel indoors (about 190).
The same in all three:

| Ticks after | CS2 |
|---|---|
| +0 | a soft white-peach flash about a hundred units across, a lens streak across the screen |
| +1 – +3 | orange puffs bursting out, rays of sparks (white and dense indoors), the walls lit orange |
| +2 – +7 | a puffy fireball about 180 units across, orange with a yellow-white core, darkening at its edges |
| +8 – +10 | dark red inside black smoke |
| +11 – +18 | brown-black smoke thinning |
| from about +22 | gone (indoors a faint haze to about +28); the scorch stays |

Unreal, round 17's blast at the mid doors (tick 151907, 228 units from where the mid one went off) through a camera
placed the same way relative to it, before: a small white flare for 15 ticks and a fireball burning orange past +60.
After rebuilding the blast (`grenade_effects.py`: CS2's heat sheet in orange over the fireball's own dark smoke, its
jets of fire, a 4-tick flash, lifetimes following the table rather than the files' 1 – 6 s): flash +0 – +3, orange
fire +4 – +8, dark smoke +9 – +15, gone by +17. Its fire is a little yellower than CS2's; the blast at the doorway
hides some of it behind the door frame. Not drawn: the lens streak, the heat trails and embers.

**Correction (2026-10-31): the table's ticks are about 4.2 times too short.** Each `demo_step_tick` moves CS2's
particles about 3 ticks further than the demo, so a stepped "+N" is about +4.2 N in the demo's own time. Round 17's
blast at the mid doors, played at a quarter speed and grabbed as it played (`tools/cs2/slow_shots.py`, a free camera
350 units south of it):

| Ticks after | CS2, played |
|---|---|
| +3 – +11 | a white-peach flash about 150 units across (a lens streak at first), the walls lit orange; rays of sparks to about +25 |
| +13 – +30 | a puffy orange fireball with a yellow-white core, about 150 units across |
| +28 – +45 | dark red inside black smoke |
| +47 – +75 | brown-black smoke thinning |
| to about +110 | a faint haze |

The same stages as the stepped table at 4.2 times its ticks. The blast's layers were first stretched to match (their
lifetimes times 4.2), but the user preferred the short blast of the first rebuild (2026-10-31): it is back, 1.5
times bigger, without the 7 tongues of fire flung out (their ropes of smoke crossed walls). Its light keeps the file's
0.225 s, which the played frames match (the walls orange to about +11, gone by +15). Frames taken with
`pov_shots.py` reach their tick in one 64-tick step, so their particles are about 3 ticks older than the tick:
a check of something that lasts a few ticks (a blood splash one tick after a hit) is worth taking again played.

## Molotov against CS2 (2026-10-31)

Round 17's molotov at the top of mid (fire 251, lit at tick 153065, out at +450), from a free camera 290 units east
of it and 195 up (`183.18 1439.73 195.64`, pitch 33.6, yaw 180), CS2 played at a quarter speed for its first two
seconds (`tools/cs2/slow_shots.py`) and stepped 8 ticks at a time after (the look only: each step runs the
particles about 3 ticks ahead):

| Ticks after | CS2 |
|---|---|
| +5 | a ball of fire 40 units across where the bottle broke |
| +7 – +22 | it swells into a yellow-orange fireball 130 units across, thrown the bottle's way (into the wall), flames licking up to the wall's top, dark at its edges from +13 |
| +24 – +47 | deep red and black with embers; the carpet of flames lights under it from +26 |
| +40 – +66 | black smoke rising off it and thinning, gone by +70 |
| +45 – about +200 | a white-hot column about 60 units across and 100 high where it lit |
| to +450 | a dense carpet over about 250 × 250 units, cream and peach where its flames overlap, orange-brown where thin; magenta specks all along its near edge |

Unreal before (`grenade_effects.py` of 2026-10-30): no burst; 8 flames and 3 tongues of gas on every flame, orange,
no edge. Rebuilt from `molotov_groundfire` and `molotov_explosion` (`grenade_effects.py`, the fire layers'
comments): the burst from the demo's own dispatch (the replay's fire `burst`), the carpet coloured by CS2's
gradients, a heart where it lit, the edge's flames on the side the camera sees. Compared at the same 17 ticks:
the burst grows, rises and turns black at CS2's ticks (a little smaller and its smoke paler); the carpet is as
wide and as dense, a little yellower than CS2's cream; the magenta edge shows where
CS2's does. Five passes on the way: at CS2's per-point numbers on 16 flames the carpet was a few faint flames (CS2
fills it from hundreds of points of its own); the fireball's smoke went black at +11 until its sheet played over a
longer life (its first 40 %), and read dark brown until its unlit colours were 6 times brighter (a lit map's
exposure); the edge burned red-orange until its lookup went down the gradient as it faded.

Not drawn: CS2's flames climbing the edge of walls are drawn from the export's wall points, not CS2's (its own
traces from the outline); the burst's ground splashes (`explosion_molotov_ground_splash07a`) and its pale steam
(`molotov_explosion_child_fireball4`). The incendiary's burst (sparks, tongues, splashes) is built from its files
but checked only against stepped frames.

## Flashbang against CS2 (2026-10-31)

Round 17's flashbang at tick 154316 in the upper B tunnel, from a free camera at MAJ3R's eyes 304 units away
(`-1683.56 1014.23 88.96`, pitch 8.2, yaw 35.5), CS2 played at a quarter speed: a soft white-peach glow about 200
units across, low by the floor, brightest 4 – 6 ticks in with a streak across it, fading out by +12, gone by +15; the
room round it hardly lit. CS2's particle system is a single flare dispatched by the server
(`explosion_flashbang`: 0.25 s, no depth test, faded by how much of it is in sight); its size is bloom. Its sheet,
`aircraft_blue`, is not in CS2's archives.

Unreal before: the whole room blown out white at +1 (a 600 cd light), then a hard white disc 80 units above the flash
(the sprites' lift). Now (`grenade_effects.py`, `FLASH_LAYERS`): a broad soft glow (`particle_flare_001`) under a
white core (`aircraft_white`), not lifted, faded in over the first quarter and out over the rest, drawn 120 units
nearer the camera so the floor does not cut it (`camera_bias`), and a faint light with the same timing. Compared at
+4, +6 and +9: the same timing and place; Unreal's glow a little more compact and brighter in the middle, without
CS2's streak.

## Smokes against CS2 (2026-10-31)

Round 17's smokes at de_dust2's mid doors from a free camera 700 units south (`-397.18 951.68 62.80`, pitch 8,
yaw 90), CS2 every 3 s through their life: puffy, cauliflower-like clouds, grey with a beige cast in the sun, shaded
in every crease and dark grey on their side in the wall's shadow. Unreal before: the same cloud (its simulated
volume matches CS2's size and its shrinking to the door at +1414 and gone by +1990 to the tick), but a flat orange-tan
fill. Shot with the indirect light off, the sun alone lit the cloud's sunny side and left the rest black: the
indirect light (Lumen's translucency volume) filled it evenly. Now (`smoke_volumes.py`): the volume's ambient
occlusion on (`r.HeterogeneousVolumes.AmbientOcclusion`, also in the project's `DefaultEngine.ini`), a cool grey
albedo with 5 % of the team colour (half before), billows twice as strong and a little bigger, bulging out of the
surface (the noise's crests had carved pits: a sponge), a crisper edge. Compared at four ticks (`probe_smoke.py`):
grey cauliflower billows, shaded creases and side, as CS2's; Unreal's still a little softer and warmer. Then, from the
user's look in the editor: the billows rise through the cloud (the noise drifts up 20 units a second: a fixed noise
left the smokes standing still), the noise moves where the cloud is looked up by up to 15 units (a cloud cut by
CS2's solid voxels, a clip over mid's low wall, showed a flat plane there), and a fading cloud turns translucent
instead of its threshold rising to 1 (it was eaten into holes as it went). The noise's input pin is "World Position":
the material connected it as "Position" without checking, so it had always taken its default (the same place).

## What was wrong

### Fixed

1. **Local lights died out before their range** (de_nuke's B site, decontamination room, lobby; de_dust2's tunnels
   and CT-spawn room). CS2 keeps a light at full strength up to range × (1 − skirt) and cuts it there; Unreal's
   inverse-square window is already at 0.35 at 80 % of the radius and 0.01 at 97 %, and CS2 maps set ranges just long
   enough to reach the floor (most of de_nuke's lights have skirt 0). The importer now sets the attenuation radius to
   range × (1.375 − 0.725 × skirt), the factor that fits the two fades best
   ([lighting.md](formats/lighting.md#local-lights--light_omni2-light_rect-light_barn-and-legacy-light_omni-light_spot)).
   de_nuke's B site 0.50 → 1.00, decontamination room 0.24 → 0.83, lobby 0.51 → 0.69; de_dust2's lower tunnel
   0.32 → 0.68, upper tunnel 0.88 → 1.00, CT spawn 0.11 → 0.31; de_inferno's library 0.72 → 1.09. It also brightens
   rooms that were already bright enough (de_inferno's lamp-lit apartment 1.23 → 2.18, de_nuke's garage
   0.85 → 1.38): their excess is item 9.
2. **Bounce-only lights lit directly** (`directlight` 0: de_mirage 6, de_dust2 1, de_inferno 17, de_nuke 6). One sits
   94 units from de_dust2's pit wall: the wall showed bright where CS2 has it in shade (a cell at 3.35, the view
   1.31 → 1.01). The exporter writes `direct` to the manifest and the importer keeps only their bounce (Diffuse Scale
   0.02 with 50× the Indirect Lighting Intensity, see lighting.md).
3. **Missing shadows from overflowing virtual shadow maps** (de_nuke). With 174 shadowed lights the page pool and
   the per-pixel light list overflowed ("Virtual Shadow Map Page Pool overflow (1413 page allocations were not
   served) … missing shadow" in the log); whole pages of shadow went missing, seen as blocky patches of light on the
   B site that changed from one render to the next. Raised to 16384 pages and 32 lights per pixel (sandbox
   `DefaultEngine.ini`, and set by `import_map.py` / `shoot_views.py`). The page-pool overflow is gone; the light
   list still overflows once per de_nuke render (32 is the engine's maximum; a pixel's lights beyond 32 lose full
   shadow filtering, they keep their shadows).
4. **Other game modes' props and blockers were exported**: entities with `startdisabled` — de_inferno's 31 wingman
   props `props.2v2` (barricades, bins, cones, a door) and 24 blocker brushes, de_mirage's 29 retake brushes and 2
   shutter pieces, de_nuke's 8 blocker brushes, 4 retake brushes and 3 wingman doors. CS2 does not draw them in a
   normal match; the exporter now skips them.

### Open (follow-ups)

5. **Doors stand open** (de_nuke's tunnel: CS2 shows a closed orange double door, Unreal an empty frame — the view
   reads 1.6×). Fixed 2026-10-23: CS2 skins such meshes into the model's reference pose (its embedded `ref`
   animation), which the map export now applies ([model.md](formats/model.md#reference-pose-modelrefpose)): de_nuke's
   `metal_door_001_br` turns 90° about z into its frame. de_inferno's `break_wooden_door_01` was not off (its mesh
   has no bones of its own: drawn as stored); de_mirage's ceiling fans have bones moving differently and stay as
   stored.
6. **No self-illumination.** Materials with `F_SELF_ILLUM` (`g_tSelfIllumMask`, `g_flSelfIllumBrightness`) are
   exported as plain lit materials: de_inferno's stained-glass church windows render grey (CS2: glowing colours),
   de_nuke's fluorescent tubes and bell lamps, lit signs, vending machine and control-room screens look switched off
   (de_mirage 3, de_dust2 1, de_inferno 30, de_nuke 23 materials).
7. **Local lights shine through the back of single-sided geometry.** Shadow proxies are sun-only (lighting channel
   1), so a local light behind a floor or wall lights what is on the other side. de_nuke has two 19 × 29 m blue
   `light_rect`s 400 units below its lowest floor (EV 8.6, one pointing up), baked with full occlusion in CS2; in
   Unreal they light B site's ceiling and upper walls from below (the blue patch). Proxies that also cast for local
   lights, set slightly behind their surface so wall-mounted lights do not shade their own wall (the reason proxies
   are sun-only), would be the next thing to try.
8. **de_nuke's A site stays dark** (0.24, 0.29 with the fixes). CS2 shows bright sun shafts on the floor and the
   east wall; with OFAST's geometry and the `light_environment` direction (60°, yaw 26) every one of those points
   has the roof (`hr_metal_corrugated_005`, opaque, ~480 units up) between it and the sun. Best guess: CS2's bake
   lets the sun through roof pieces that do not cast (a per-object no-shadow flag OFAST does not read yet); or the
   shafts come from a light OFAST does not model.
9. **Interiors too bright on de_inferno** (church 2.6, sewer underpass 2.9, T-side apartments 1.75 before the
   fixes, more after) **and somewhat on de_mirage** (connector, sniper's nest, T ramp, underpass 1.2 – 1.4). The
   bounce gains (`LOCAL_BOUNCE_GAIN` 2, `OUTDOOR_BOUNCE_GAIN` 1.25) were fitted against references taken at the
   brighter CS2 setting (see How). Re-measured on de_mirage against the default picture (2026-10-18,
   [above](#light-and-material-against-the-default-picture-2026-10-18)): the gains hold for the calibration views,
   but relative to the sun the connector and sniper's nest get 1.17 / 1.18 of CS2's light (`LOCAL_BOUNCE_GAIN` 1.5
   would fit them and leave the palace at 0.93) and the underpass 1.21, which follows the outdoor gain, not its lamp:
   sky light reaching into an enclosed room. No one gain fits these and the palace; open. Without the highlights of
   the baked lamps and sun (2026-10-18, [above](#highlights-of-baked-lights-2026-10-18)) the palace is at 1.00
   relative to the sun, the connector and sniper's nest 1.19 / 1.21, the underpass 1.26: the same picture.
   de_inferno's interiors were measured with the refitted Hable level since (refit column above): 1.2 – 2.2.
10. **Ropes and cables are missing** (`path_particle_rope_clientside`: 69 on de_nuke — the power lines in the yard
    and their shadows).
11. **Dark rooms lit by daylight from outside stay too dark**: de_mirage's shop 0.73 (cells down to 0.22), de_dust2's
    CT spawn room 0.31 — Lumen carries less of the sunlit street's bounce than CS2's bake (the known B-apartments
    case in lighting.md).
12. **Small things**: de_nuke's observation room and garage read 1.6 – 1.8 / 0.85 – 1.6 (bright walls, but a black
    garage ceiling where CS2's is lit by bounce); de_inferno's banana 1.64 (shaded walls brighter than CS2's).
13. **Barn lights** (2026-10-18, de_vertigo): OFAST lit them as hemisphere-wide rects, where CS2 lights a frustum
    times a cookie texture. Stationary and dynamic barn lights with a cookie are now spot lights fitted to it (the
    CT spawn's far wall 2.4× → 1.05× CS2's light). Static ones with a cookie keep the rect: de_vertigo's 36 T-spawn
    lamps are next to absent in CS2's bake (the T spawn reads 1.09 without them, 1.81 with), de_nuke's garage lamps
    are not; their `skirt_near` (0 against 0.1) is the only difference found
    ([lighting.md](formats/lighting.md#barn-lights-and-their-cookies)). de_vertigo's CT floor is now short of sky
    light in Unreal (a third of the sky's blue).
