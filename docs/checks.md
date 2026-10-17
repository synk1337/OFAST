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

| Map | View | After (above) | Merged | Why |
|---|---|---|---|---|
| de_inferno | ct0 / t0 | 0.92 / 1.81 | 0.81 / 1.44 | the new tone curve (the old H(2.5 x) / H(1.25 W) was fitted on these surfaces) |
| de_inferno | asite / bsite / middle / pit / balcony | 1.06 / 1.00 / 1.10 / 0.97 / 1.03 | 0.92 / 0.86 / 0.93 / 0.82 / 0.86 | same; de_inferno's base colours are 0.60× CS2's albedo (lighting.md) |
| de_inferno | apartments / apartments2 / underpass / ruins / banana | 2.18 / 1.75 / 2.87 / 2.79 / 1.66 | 1.93 / 1.30 / 2.38 / 2.08 / 1.44 | same; still item 9 |
| de_inferno | arch / library | 0.84 / 1.09 | 0.64 / 0.75 | same |
| de_nuke | bsite | 1.00 (what-if) | 0.85 | the fixes for real (bounce-only lights, start-disabled entities); still patchy: items 6 and 7 |
| de_nuke | observation / tunnels | 1.76 / 2.30 | 1.66 / 2.17 | same |
| de_mirage | underpass / shop | 1.36 / 0.79 | 1.42 / 0.82 | |
| de_anubis | ct0 / t0 | — | 1.11 / 1.08 | 1.01 / 1.01 against the uncorrected references the curve was fitted on: the fit predates the brightness correction and should be redone with it |

New views without a CS2 reference yet: de_dust2 `dumpster`, de_inferno `trike`, de_nuke `ctvan`. de_dust2's tint
views read 1.18 – 1.26 (sunlit, as the tint checks found).

Two of the import's own screenshots were wrong: de_mirage's `underpass` file held the CT-spawn view (and `ct0` was
missing), and de_dust2's `uppertunnel` came out at 0.88. `shoot_views.py` on the saved levels gave the right
pictures (1.42; 0.99 and 1.01 in two runs), which the table uses. The import's screenshot loop can therefore drop a
shot or catch one early; re-shoot with `shoot_views.py` when a number looks off.

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
   reads 1.6×). The door models (`metal_door_001_br`, de_inferno's `break_wooden_door_01`; de_mirage's ceiling fans
   too) have a rotated root bone, and CS2 poses the mesh with its skeleton / reference animation; OFAST places the
   bind-pose mesh, 90° off. The model loads fine (`ofast export-model`); the fix needs the skeleton's default pose
   (and probably the `ref` animation, which is compressed) applied at export.
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
   brighter CS2 setting (see How), and de_inferno's Hable level and colour-correction table are being re-measured in
   unmerged work (branch `claude/angry-dirac-c8379c`, the tone-fit session's uncommitted `tone_fit.py`). Re-fit the
   bounce gains on brightness-corrected references once that lands; re-check these views then.
10. **Ropes and cables are missing** (`path_particle_rope_clientside`: 69 on de_nuke — the power lines in the yard
    and their shadows).
11. **Dark rooms lit by daylight from outside stay too dark**: de_mirage's shop 0.73 (cells down to 0.22), de_dust2's
    CT spawn room 0.31 — Lumen carries less of the sunlit street's bounce than CS2's bake (the known B-apartments
    case in lighting.md).
12. **Small things**: de_nuke's observation room and garage read 1.6 – 1.8 / 0.85 – 1.6 (bright walls, but a black
    garage ceiling where CS2's is lit by bounce); de_inferno's banana 1.64 (shaded walls brighter than CS2's).
