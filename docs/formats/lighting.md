# Lighting, sky, exposure (`light_*`, `env_sky`, `post_processing_volume`, `.vpost`)

How CS2 lights and exposes a map, and how OFAST rebuilds the same picture in Unreal. OFAST's code:
`crates/ofast-core/src/export/map.rs` (lights, manifest), `src/export/sky.rs` (sky), `tools/unreal/import_map.py`
(Unreal setup).

CS2 bakes almost all of its lighting (lightmaps, light probes). OFAST does not reuse the bake: Unreal relights the map
with Lumen from the same light sources, so the goal is to give Unreal the same sources, in matching units, and the
same camera exposure. Numbers below are de_mirage's (2026-10-16).

## One unit system: `LUX_PER_UNIT`

CS2 light values are unitless. Its shaders compute diffuse light as `albedo × light` (no 1/π; Source 2 Viewer's
`pbr.slang`), where Unreal computes `albedo × lux / π`. OFAST multiplies every CS2 light value by one constant,
`LUX_PER_UNIT = π`, so a lit surface has the **same scene value** in both engines. Everything that emits light in
Unreal is scaled by it (sun, local lights, Sky Light and visible sky by `LUX_PER_UNIT / π`) and the exposure is
derived from it, so any other constant would give the same picture; π keeps Unreal's numbers equal to CS2's.

## Sun — `light_environment`

| Key | de_mirage | Meaning | Unreal (Directional Light) |
|---|---|---|---|
| `angles` | 60 318 0 | direction | rotation (−pitch, −yaw, roll) |
| `color` | 255 223 179 | sRGB colour | light colour |
| `brightness` × `brightnessscale` | 3 × 1 | **linear** multiplier | intensity = 3 × π = 9.42 lux |
| `angulardiameter` | 1 | "sun spread angle", shadow softness (degrees) | Source Angle |
| `bouncescale` | 1.3 | bounce strength in the bake | Indirect Lighting Intensity (× 1.25, see the bounce gains) |
| `skytexture` | `mirage_sky` | ambient comes from that `env_sky`'s HDR | Sky Light cube map |
| `skycolor` × `skyintensity` | 130 205 255 × 1 | flat ambient, **only used when `skytexture` is empty** (Hammer's `lights_base.fgd`) | — |
| `skyambientbounce` | 0 0 0 | albedo of a virtual ground below the horizon | Sky Light lower hemisphere colour |

Ambient light: CS2 light values are irradiance / π, so the Sky Light intensity is `skytexturescale × LUX_PER_UNIT / π`
(1.0 with π). Maps without a `skytexture` are lit by the flat `skycolor × skyintensity`: the exporter then writes
`<map>_ambient.hdr`, a uniform cube of that colour, for the Sky Light (the visible sky stays the `env_sky`).

Below the horizon CS2 lights the scene from a virtual ground of albedo `skyambientbounce`. Unreal's Sky Light takes a
radiance there instead (`lower_hemisphere_color`), approximated as albedo × (sun on flat ground + the sky's upper
hemisphere average); black on de_mirage (albedo 0).

## Local lights — `light_omni2`, `light_rect`, `light_barn` (and legacy `light_omni`, `light_spot`)

- `brightness` is in **stops** (EV) for the `*2`/rect/barn classes: linear = 2^brightness × `brightnessscale`
  (the `brightness_units` key only says how Hammer displayed it; the engine reads the EV value). The legacy classes
  give a linear `brightness`.
- The linear value is the illuminance reached **100 units (2.54 m)** in front of the light (Source 2 Viewer's
  `SceneLight.cs`). In candela: linear × `LUX_PER_UNIT` × 2.54² — glTF `KHR_lights_punctual` intensity, which
  Unreal's Interchange imports as candela.
- Checked on de_mirage's own lights: Hammer's stored `brightness_candelas` ≈ 79.6 × 2^EV, i.e. 1000 lm / 4π at EV 0.
- `colormode` 1 means the colour comes from `colortemperature` (kelvin) instead of `color`.
- Shapes: `light_omni2` lights a cone of half-angle `outer_angle` (180 = all around) from a sphere of radius
  `size_params[0]`; `light_rect` / `light_barn` are rectangles lighting the hemisphere in front of them, and their
  `light_rect` `size_params` are **half**-sizes (Hammer's defaults 16 × 16 with 80 cd and 120 nits only agree for
  32 × 32 units; likewise the omni2 default 9816 nits ↔ a sphere of radius 2). A `light_barn`'s `size_params` is its
  barn-door aperture instead; its emitter is `luminaire_size` (default 4) — OFAST uses that as a square and ignores
  the barn doors for now. `light_spot` uses `innerconeangle` / `outerconeangle`.
- `range` is where the light has faded out (linearly over the last `skirt` fraction); `bouncescale` scales its bounce
  in the bake (≈ Unreal's Indirect Lighting Intensity, × 2 — see the bounce gains).
- de_mirage's lights are `directlight` 1 ("Static": fully baked into the lightmaps, bounce included).

In the glTF they are `KHR_lights_punctual` approximations (rect → wide spot) for any tool. Unreal's Interchange drops
the glTF `range` (every light gets a 10 m attenuation radius), so `import_map.py` replaces them with Unreal Point,
Spot and **Rect** lights built from the manifest's `lights` list: exact shape, size, range, colour and bounce, movable.

**Unreal rect lights shine twice their candela value** along their axis: `RectLightSceneProxy.cpp` sets the luminance
to intensity / (0.5 × width × height), while point and spot lights use the intensity as is. `import_map.py` halves
rect lights' candela; without it de_mirage's palace came out 1.48× too bright, with it 0.81× (then 0.93 with the
bounce gains).

## Sky — `env_sky`

`skyname` is a `sky.vfx` material; its `g_tSkyTexture` is an HDR **cube map** (BC6H, 6 faces of 512², flag
`CUBE_TEXTURE`). Face layout and orientation: see [vtex.md](vtex.md#cube-maps). The sky shows
`cube(R⁻¹ · view direction) × tint_color/255 × brightnessscale × 2^(g_flBrightnessExposureBias +
g_flRenderOnlyExposureBias)`, with `R` the entity's rotation. When the map has a 3D skybox, the `env_sky` **inside the
skybox world** is the one drawn, so it also turns with the `skybox_reference` angles (de_mirage's has yaw 146.25° and
a reference at 0°; the sky is symmetric around the vertical, so it shows little).

A map can have several `env_sky`s. The **visible** one is the first without `startdisabled` (the 3D skybox world's
first). The one that **lights** the map is the one whose `targetname` the `light_environment`'s `skytexture` names,
and it can be a disabled one: de_inferno shows `MAT_sky` and lights with the disabled `LIGHT_sky`. When they differ the
lighting sky is written as `<map>_ambient.hdr` for the Sky Light, and its averages set the ambient values; a
`skytexture` that names no `env_sky` is reported as a warning and the visible sky lights the map.

OFAST writes it as `<map>_sky.hdr`: an equirectangular Radiance image (2:1) in the layout Unreal imports as a
`TextureCube` — `u = 0.5 + atan2(D.x, −D.y) / 2π`, `v = acos(D.z) / π` for the Unreal direction `D`. Rotation, tint
and brightness are baked in; the render-only bias goes to the manifest. Check: the image's average radiance
(0.754, 0.792, 0.836) matches the texture's own stored "reflectivity" (0.742, 0.788, 0.844).

In Unreal the same cube map lights the Sky Light (Specified Cubemap) and is drawn on a large unlit sphere whose
material looks it up along the view direction, times the manifest's `sky_intensity` (`LUX_PER_UNIT / π` × the
render-only bias).

Without a sky or sun in the manifest, `import_map.py` falls back to Unreal's physical sky, a default sun and auto
exposure, and says so in its log: the picture is then not calibrated.

## 3D skybox — `skybox_reference` + `sky_camera`

See [map.md](map.md#3d-skybox). CS2 draws the 3D skybox first and the world over it, so skybox geometry never hides
the world, whatever its position, and is lit apart. Placed at 16× around the map in one Unreal scene it can cut
through the playable space: de_dust2's skybox sand lies 1.1 m above the floor of a CT-spawn room and covered it.
`import_map.py` (`setup_skybox`) therefore gives skybox actors a material whose pixel depth offset pushes them 1 km
back (the world always wins the depth test), and takes them out of shadows and Lumen; translucent skybox materials
(clouds) keep theirs. Skybox light blockers are hidden without casting anything.

## Exposure and tone mapping — `post_processing_volume` + `.vpost`

| Source | Key | de_mirage |
|---|---|---|
| `post_processing_volume` (the `master` one) | `enableexposure`, `minexposure`, `maxexposure` | on, 0.8, 1.0 (linear scale) |
| | `postprocessing` | `lighting/postprocessing/de_mirage_prefab/de_mirage.vpost` |
| `.vpost` DATA `m_toneMapParams` | `m_flWhitePoint`, `m_flExposureBias` | 1.5, 0 |
| | shoulder / linear strength / linear angle / toe strength, num, denom | 0 / 0.001 / 0.001 / 1, 1, 1 |
| `.vpost` DATA `m_bloomParams` | `m_flScreenBloomStrength`, `m_flBloomThreshold` | 0.126, 1.055 |
| `.vpost` DATA | `m_bHasColorCorrection`, `m_nColorCorrectionVolumeDim`, `m_colorCorrectionVolumeData` | true, 32, 32³ RGBA8 colour table (near identity, slightly more saturated; see [the table](#the-colour-correction-table)) |

The tone curve is Hable's (Uncharted 2) filmic curve (Source 2 Viewer's `post_processing.frag.slang`); with the
stock parameters above it is a straight line. **Measured** on CS2 screenshots, the picture is

```text
display = clamp(exposure × scene, 0, 1)   then sRGB
```

The measurement: in a sunlit de_mirage view, pure-sky pixels — whose radiance is known exactly from the exported sky
— display at 0.80 × radiance (blue channel; red and green read a few percent higher), and 0.80 is the volume's
`minexposure`. Source 2 Viewer's reading, `exposure × scene / white_point`, would give 0.53: the white point does
not scale the picture (tool: `tools/compare/sky_exposure.py`). Auto exposure is clamped to [0.8, 1.0]; sunlit views
sit at 0.8, the palace too, the dark B apartments go up to 1.0. Its controller is Source 1's histogram tonemap
(cheat cvars `mat_tonemap_force_percent_target` "old default 45", `…_percent_bright_pixels`, `…_average_lum_min`).

For comparisons, **`mat_tonemap_force_scale <exposure>`** (cheat) locks CS2's exposure: a palace capture at
`mat_tonemap_force_scale 0.8` is identical to the automatic one (ratio 1.000), at 1.0 it is 1.23× brighter.
`mat_fullbright 1` shows the albedo × 1, linearly: it ignores the locked exposure (de_anubis at 1.0 and 0.5: identical
to 0.002 %) and the tone curve (fullbright against Unreal's base colour is flat over the whole brightness range).
Unreal's `ShowFlag.Lighting 0` render, its base colour, skips the manual exposure (exposure 1: its sky pixels fit
exposure 1.0 exactly) but keeps OFAST's tone mapper.

Unreal, with Manual exposure (physical camera off) and its tone curve off (`ToneCurveAmount` 0, `ExpandGamut` 0,
`BlueCorrection` 0), shows `luminance × 2^bias` (`PostProcessEyeAdaptation.cpp`; checked the same way: Unreal's
sky pixels display at exactly the intended ratio). A CS2 scene value of 1 is a luminance of `LUX_PER_UNIT / π`, so:

```text
EV100 = log2(LUX_PER_UNIT / (π × exposure × 2^bias))        Unreal exposure bias = −EV100
```

de_mirage: EV100 0.322 at exposure 0.8 (sunlit, used for screenshots), 0 at 1.0.

**When `minexposure` > `maxexposure`** (de_nuke: 1.0 and 0.85), the minimum wins: CS2's automatic picture equals a
`mat_tonemap_force_scale 1` one to 0.1 %, and a forced 0.85 one is 0.855× as bright. OFAST uses the minimum too.

**Non-linear curves.** de_inferno's `.vpost` has Hable's stock parameters instead: shoulder 0.15, linear strength 0.5,
linear angle 0.1, toe strength 0.2, toe numerator 0.02, toe denominator 0.3, white point 4. So do most official maps:
cs_italy, cs_office, cs_shelter, de_ancient, de_ancient_night, de_anubis, de_cache, de_inferno, de_train and
de_vertigo. Only de_dust2, de_mirage, de_nuke and de_overpass are linear. de_boulder's `.vpost` has no tone-map block,
and de_debris, de_eldorado, de_fachwerk and de_poseidon have no `.vpost` that `map_entities` finds.

Halving the exposure does not halve the picture there: on de_anubis the darkest pixels drop to a third (×0.32) and
the brightest only to ×0.79. With `H(x) = (x(Ax + CB) + DE) / (x(Ax + B) + DF) − E/F` (the `.vpost`'s curve), W its
white point and LUT its colour-correction table, CS2 shows:

```text
shown sRGB = LUT( sRGB( H(k × exposure × scene) / H(w × W) ) )        k = 3.4, w = 6
```

Measured on **de_anubis** (2026-10-16 night), the map to measure on: its visible sky (the 3D skybox's `env_sky`) is
the same texture at the same brightness (1.45) as the one that lights it, there is no bloom (threshold 10), and its
automatic exposure sits at `minexposure` (0.7) in both spawns. Shots at the spawns at exposure 0.7, 0.35 and with
`mat_fullbright 1` (`tools/cs2/cfg/ofast_v_anubis_*.cfg`). Method: invert CS2's shots through the table (its grey
diagonal per channel) and sRGB, then compare with the curve: pixel by pixel between an exposure pair, against the
exported sky's radiance on sky pixels (picked where Unreal's render shows nothing but sky), and against Unreal's
scene values (its shots inverted through its own curve) on lit cells.

- **The table** (below) is applied after the curve, to the sRGB-encoded picture. de_anubis's sends everything below
  27/255 to black: that, not Hable's toe, is why the darks drop to a third. Without it no `k` and `w` fit the
  exposure pairs; with it inverted, every pixel halves at half exposure (0.50 – 0.51 over the whole brightness range).
- **w = 6** (white 22 – 24): the halving is the same at every brightness for w × W = 22 – 24, on de_anubis's two
  spawns (0.7 / 0.35) and on de_inferno's CT spawn (0.8 / 0.4). A pair does not depend on `k` (it scales both shots).
- **k = 3.4**: sky pixels, whose radiance is known from the exported sky, follow the stock Hable shape at both
  exposures and in all three channels (3 – 5 % per pixel), with k = 3.37 – 3.44 on both de_anubis spawns.

| Anchor (CS2's shots read through the new rule) | de_anubis | de_inferno |
|---|---|---|
| sky pixels | k = 3.37 (CT), 3.44 (T) | 3.15 – 3.25 with OFAST's sky: CS2 draws the **skybox** `env_sky` (0.8 × 2^1), at 0.93 – 0.95 of OFAST's value; the main world's (0.5 × 2^1) would be 0.63 |
| lit surfaces, k × CS2 scene / Unreal scene (median of 12 cells) | 3.27 (CT), 3.41 (T): OFAST's light × albedo is right | 5.4 (CT): Unreal's surfaces are too dark |
| Unreal base colour / CS2 fullbright albedo | 1.00 (CT), 0.72 – 1.00 (T surfaces) | 0.60 (CT) |
| light, lit ÷ albedo in each engine (Unreal / CS2) | 1.02 (CT), 1.0 – 1.4 (T surfaces, where the base colour is lowest) | 1.01 (CT; 0.58 – 1.51) |
| whole shot, Unreal / CS2, old rule → new rule (simulated) | 1.37 → 1.02 (CT), 1.32 → 1.00 (T); sky 0.98 | 0.85 → 0.74 (CT: sky 1.05, surfaces 0.73), 1.46 → 1.16 (T) |

So the level differs, not the sky or the light: with k = 3.4 the sky and the lit surfaces of de_anubis agree, and
de_inferno's sky agrees too. **The old rule (a = 2.5, b = 1.25, no table) was wrong**: it was fitted on de_inferno's lit
surfaces, which are darker in Unreal than in CS2 for another reason, and it made de_anubis 1.3× too bright, sky
included. That reason is de_inferno's **materials**: their base colour is 0.6× CS2's albedo in the CT view, while the
light itself matches (lit ÷ albedo: 1.01). That is open. Its sunlit ground is odd in CS2: lit ÷ fullbright gives 2.08
units of light, less than the sun alone (≈ 2.5), so for some de_inferno materials fullbright may not show the albedo
the lit shader uses. The old rule's other two anchors were artifacts: its fullbright anchor (3.8) assumed fullbright
goes through exposure and curve, and it does not; its sky anchor (2.0) used the same wrong curve.

Not reproduced, because toggling them in CS2 changes nothing: `r_csgo_enable_tonemapping 0` (identical to 0.1 %),
`r_csgo_render_post_colorcorrection 1` (0.02 sRGB level), `r_csgo_render_post_bloom 0` on de_inferno, which has an
additive bloom (strength 0.12, threshold 0.6): 0.7 %.

### The colour-correction table

`m_colorCorrectionVolumeData`: `m_nColorCorrectionVolumeDim`³ (32³) RGBA8 entries, red fastest, then green, then
blue; input and output are the sRGB-encoded picture. The exporter writes it as `<map>_lut.png`, the 32 blue slices
side by side (1024 × 32, pixel (b × 32 + r, g) = output for input (r, g, b), Unreal's colour-grading layout), and the
manifest's `post.color_correction` names it. Every official map checked but de_overpass has one:

| Largest change (sRGB levels of 255) | Maps | Grey input 32 → |
|---|---|---|
| ≤ 10 | de_mirage 7, de_train 7, de_dust2 10, de_overpass (none) | 30 – 32 |
| 15 – 24 | de_ancient 15, de_nuke 17, cs_italy 17, cs_office 22, de_cache 24, de_inferno 24 | 14 – 25 |
| 27 – 31 | de_vertigo 27, de_anubis 31 | 5 – 19 |

CS2 draws every map through the same post-processing, so OFAST applies the table on linear maps too, although there
it could not be confirmed separately: its effect is small (de_mirage ±1 %, de_dust2 +1 %), and de_nuke's 17 levels
move its spawns from 0.98 / 0.96 to 0.92 / 0.90 of CS2 (an exposure pair, 1.0 / 0.85, halves the same with or
without it; its sky's three channels agree better with it in one spawn and worse in the other).

### In Unreal

A post-process material at `BL_ReplacingTonemapper` (`M_OFAST_Tonemap_v3`), on every map: scene colour ×
eye-adaptation exposure → a straight line (shoulder 0) or `H(3.4 x) / H(6 W)` (`HABLE_INPUT_SCALE`,
`HABLE_WHITE_SCALE` in `import_map.py`) → sRGB → the map's table, imported as an uncompressed, non-sRGB texture
without mips and read with two bilinear lookups on neighbouring blue slices. Unreal applies nothing after a replacing
tone mapper. The curve's parameters come from the manifest's `post.tonemap`. Before this, linear maps used Unreal's
own tone mapper with its curve off, and Hable maps `M_OFAST_TonemapHable_v2` (H(2.5 x) / H(1.25 W), no table).

## Calibration against CS2 (de_mirage, 2026-10-16)

Same viewpoints in CS2 (`-insecure`, `setpos`/`setang`, HUD off, interiors at `mat_tonemap_force_scale 0.8` like
Unreal) and Unreal, compared cell by cell on a 4 × 3 grid with `tools/compare/compare_shots.py` (ratio of mean
linear luminance, Unreal / CS2). Views: `tools/compare/views/de_mirage.json` plus the two spawns.

| View | Whole image | Cells | Notes |
|---|---|---|---|
| CT spawn (sunlit street, sky) | 1.00 | 0.93 – 1.06 | sunlit ground 1.00 – 1.06, sky 0.96 – 0.99 |
| T spawn (sunlit + large shaded walls) | 0.97 | 0.89 – 1.04 | shaded walls 0.93 – 0.97 |
| Palace interior (`light_rect` wall lights) | 0.965 | 0.77 – 1.13 | wall facing the lights ≈ 0.97 |
| B apartments (`light_omni2` ceiling lights) | 0.75 | 0.58 – 0.92 | see below; 0.6 since the shadow proxies really cast ([below](#shadows-from-the-back-of-walls)) |

Unreal screenshots are taken 6 s after switching to each camera (`SHOT_DELAY_S`), so Lumen's lighting has settled.
At 2 s (the first measurements) interiors came out too dark by a varying amount: the palace measured 0.79 – 0.93
depending on which view was shot before it; at 6 s and 10 s it is 0.965 every time.

History: Unreal's template lighting gave the CT view 0.91 overall but 0.38 in shade and 0.61 in the sky; before the
rect-light fix the palace was 1.48; before the bounce gains (below) the four views were 0.95 / 0.86 / 0.81 / 0.74
(those, and the bounce gains, were measured at 2 s).

How the interior was pinned down (palace): exposure ruled out (`mat_tonemap_force_scale`), sky and sun ruled out
(`-Disable sun,sky`: still 1.41), bounce ruled out (`-Disable sun,sky,bounce`, which turns off the local lights'
Lumen surface-cache bounce — screen traces still add a little: the wall facing the lights stays at 1.3× CS2's total).
CS2's actual light on the wall, from lit ÷ albedo against the sunlit CT ground, is 1.7× the formula's direct light
(its bounce), so the formula was right and Unreal over-delivered: the rect-light factor above.

**Bounce gains.** With CS2's own bounce scales applied (sun `bouncescale`, sky `skybouncescale`, each light's
`bouncescale` → Unreal's Indirect Lighting Intensity), shade and interiors were still 10 – 25 % darker: CS2's
path-traced lightmaps carry more bounce light than Lumen. Measured with `shoot_views.py -Try` (what-if settings on the
saved level): Lumen's quality settings change nothing (+0.5 %); the sun and sky bounce move the outdoor shade (×2 →
CT 1.09, T 1.21, too far) but not interiors; the local lights' bounce moves the palace. In the palace CS2 bounces
0.7× the direct light, Lumen 0.43×. `import_map.py` therefore multiplies the bounce scales by **1.25 for sun and
sky** and **2.0 for local lights** (`OUTDOOR_BOUNCE_GAIN`, `LOCAL_BOUNCE_GAIN`), which gives the table above.

The B apartments stayed at 0.75: CS2's light on their walls (lit ÷ albedo, 0.46) is several times what the ceiling
lights send directly (≈ 0.08) — it is sunlight bouncing down the corridors from the sunlit rooms, which Lumen carries
only partly whatever its settings or the sun's bounce gain (×2 moved it by 0.01). That 0.75 still included sunlight
leaking through walls (the proxies of the time never cast); without it they are at 0.6.

**Bloom and fog: no measurable effect.** Toggled in CS2 at the CT spawn (exposure locked):
`r_csgo_render_post_bloom 0` (default 1), and switching `r_csgo_render_post_colorcorrection` between 0 (default) and
1, change the picture by less than 0.1 sRGB level on average; `fog_enable 0; fog_enableskybox 0` changes nothing
either (the only differing pixels are palms moving in the wind; the 3D-skybox hotel is identical). So the Unreal side
reproduces neither bloom nor fog: doing so would add differences, not remove them. Their settings stay in the
manifest for reference. The colour-correction cvar switches nothing, but the table itself is applied (measured on
de_anubis, where it is strong; [above](#the-colour-correction-table)); de_mirage's is nearly the identity.

## Checked on three more maps (2026-10-16 evening)

The spawns of de_dust2, de_inferno and de_nuke, in CS2 (`-insecure`, exposure locked at the value OFAST uses:
0.925, 0.8, 1.0) and in Unreal with no per-map setting, compared with `compare_shots.py` (Unreal / CS2):

| View | Whole image | What it shows |
|---|---|---|
| de_dust2 T spawn (sunlit street) | 0.95 | calibration holds; the shade under a balcony 0.64 – 0.70 |
| de_dust2 CT spawn (covered room) | 0.11 | the room is lit warmly in CS2 although no light entity is near: bounce of the sunlit courtyard in its bake (or emissive surfaces); Lumen brings almost none of it — open |
| de_inferno CT spawn | 1.08 | colours right with the environment materials (white "Pasini" wall, pink house with stone showing); one ground shadow missing (1.62 there) |
| de_inferno T spawn (shaded street) | 2.7 | sunlight reaches the street in Unreal, not in CS2: shadows missing although the occluding wall has its shadow proxy: those proxies never cast (fixed, see the table below); also foliage / cloth colours |
| de_nuke CT spawn | 1.05 | |
| de_nuke T spawn | 0.97 | |

How the references were taken: with cfg files in the game's `cfg` folder (`exec ofast_setup`, `exec
ofast_v_<map>_<team>`, see [development.md](../development.md#comparing-with-cs2)). The round restarts a few seconds after `mp_warmup_end` and puts the player back on a random
spawn, so the view is re-applied (a key bound to it) right before each screenshot and checked with `getpos` — the
first de_dust2 CT reference was taken from the wrong spawn this way.

The demo export was checked the same evening: de_nuke round 2 at tick 8135, CS2's first-person view of the killer
(`demo_gototick`, `spec_player`) and the Unreal sequence's camera overlap; the victim's proxy covers the player model,
with a vertical difference of about 3° (CS2's demo playback interpolates; "TrueView: disabled - old demo").

### After the shadow and tone-curve fixes (2026-10-16 night)

All ten views again, after shadow proxies that cast ([below](#shadows-from-the-back-of-walls)) and the Hable tone
curve on de_inferno (Unreal / CS2, whole image):

| View | Before | Now | Notes |
|---|---|---|---|
| de_mirage CT / T / palace | 1.00 / 0.97 / 0.965 | 1.00 / 0.97 / 0.96 | unchanged |
| de_mirage B apartments | 0.75 | 0.61 | no more sunlight through the walls (strips on the stairs gone); bounce down the corridors still short |
| de_dust2 T / CT | 0.95 / 0.11 | 0.95 / 0.11 | unchanged; the CT room's bounce is still open |
| de_inferno CT | 1.08 (linear picture) | 0.85 | ground 0.93 – 1.01; façades and loggia in the shade 0.75 – 0.85 |
| de_inferno T | 2.7 | 1.46 | the street is shaded as in CS2 (0.98 – 1.18); a pillar and the floor by the arch get sunlight that CS2 shades (1.4 – 2.3) |
| de_nuke CT / T | 1.05 / 0.97 | 0.98 / 0.96 | |

### After the tone-curve correction (2026-10-16 night)

With the measured rule ([above](#exposure-and-tone-mapping--post_processing_volume--vpost): k = 3.4, white 6 W, the
colour table on every map), re-imported and compared again (Unreal / CS2, whole image). The real imports reproduce
the rule's simulation on the earlier Unreal shots to 0.1 % (de_anubis, de_nuke).

| View | Before | Now | Notes |
|---|---|---|---|
| de_anubis CT / T | 1.37 / 1.32 (H(2.5 x) / H(1.25 W)) | 1.02 / 1.00 | new map; cells 0.93 – 1.15 / 0.96 – 1.07, sky 0.98 – 0.99 |
| de_inferno CT / T | 0.85 / 1.46 | 0.74 / 1.16 (simulated) | not re-imported here (the export in `test-data` had moved to a newer format); CT: sky 1.05, surfaces 0.73 — the base colours, above |
| de_mirage CT / T / palace | 1.00 / 0.97 / 0.96 | 1.00 / 0.97 / 0.95 | the table is nearly the identity |
| de_mirage B apartments | 0.61 | 0.71 | the table moves dark pixels by a few % (down); the rise is something else in the shared sandbox or run-to-run, not looked into |
| de_dust2 T / CT | 0.95 / 0.11 | 0.95 / 0.095 | |
| de_nuke CT / T | 0.98 / 0.96 | 0.92 / 0.90 | applying its table (17 levels) darkens the picture: the one map where the table, unconfirmed on linear maps, makes the match worse; see [the table](#the-colour-correction-table) |

## Shadows from the back of walls

CS2's walls are single-sided: an interior wall usually has no outer face. CS2 stops sunlight at such a wall from
either side, but Unreal's Nanite meshes only cast shadows from front faces. Seen from the sun, an interior wall shows
its back and lets the light through:
- de_mirage: bright strips of sunlight on the B apartments' stairs. A ray from a lit stair towards the sun
  (`map_raycast`) crosses two plaster walls, both from behind.
- de_inferno: the T-spawn street sunlit (2.7× CS2's brightness). The caster is a plaster wall seen from behind.

**Which faces CS2 shades with.** Nearly all world-node geometry carries `OBJECT_TYPE_NO_SHADOWS`; its sun shadows
are in the lightmaps. The casters that CS2 needs from behind were all world geometry built in Hammer (no
`OBJECT_TYPE_MODEL`). Hammer geometry is often a lone wall, while models are usually closed. So OFAST gives
two-sided shadows to world geometry only: the exporter names it, and brush entities (`func_*`), `world_…`
([map.md](map.md#world-node-vwnod_c-data)).
On de_inferno's spawns this measured the same as proxying every mesh (0.73 / 1.21 against 0.72 / 1.21), with 57 – 86 %
fewer proxies. Whether CS2 really lets light through the back of models is not shown: the one case looked at (a
roof kit's wooden ceiling over a façade) also has the roof's sun-facing tiles above it.

What does not work:
- `bCastShadowAsTwoSided` on the component: Nanite does not use it.
- Two-sided materials on the visible meshes: they fix the shadows, but `DistanceFieldAtlas.cpp` then builds
  thin-shell distance fields, and Lumen's sky light and bounce leak instead (the calibration views moved by up to
  +80 %).
- A hidden copy, either `bVisible` off + `bCastHiddenShadow` or out of the main pass: a Nanite mesh hidden like
  this casts nothing at all. Earlier imports had such proxies and they never cast. de_mirage's calibration was
  measured with them, so its B apartments then included light leaking through walls (0.75, now 0.6).
- A visible Nanite copy whose masked material shows only in shadow passes (Shadow Pass Switch) casts nothing either.
  This holds although the material has `bUsedWithNanite` and Nanite's shadow raster takes the switch's shadow branch
  (`NaniteRasterizer.usf`: `SHADOW_DEPTH_SHADER` is `DEPTH_ONLY`). The cause was not found.
- Keeping only the faces the light sees from behind (`TwoSidedSign` in the shadow branch), so a copy would not cast
  again what its visible mesh casts: sunlight reached the B-apartment stairs again.

What `import_map.py` does (`add_shadow_proxies`, `make_shadow_only`): every opaque, single-sided `world_…` mesh at
least 1 m wide in two directions gets a copy with these settings:
- rendered **without Nanite** (`bDisallowNanite`: the mesh's fallback LOD);
- a two-sided masked material visible only in shadow passes (`M_OFAST_ShadowProxy_v2`);
- **lighting channel 1**, which only the sun shares (`SUN_CHANNELS`). With all lights, the copies on walls and
  ceilings shaded the very surfaces their wall lights light: de_mirage's palace fell from 0.97 to 0.62 of CS2;
- no collision, and out of Lumen (`affect_distance_field_lighting`, `affect_dynamic_indirect_lighting` off).

Masked, translucent and already two-sided meshes (decals, foliage, fences, glass) get none, since they would cast
solid shadows. Thin or small meshes (pipes, wires, bottles, signs) get none either: they cannot let a visible patch
of sunlight through. Proxies per map: de_mirage 110, de_dust2 89, de_inferno 279, de_nuke 226. CS2's own light blockers (`OBJECT_TYPE_BLOCK_LIGHT`) get the
same treatment. Unlike proxies, they stay in Lumen's distance fields, where they should stop bounce light as black
occluders, since they seal gaps for CS2's bounce light too. That has not been re-checked since their material became
masked: Lumen's surface cache sees nothing of them now.

## Import styles

`import_map.py` builds one of two lighting setups on the same imported geometry and materials (`run_ue.ps1 -Style`):

| | **standard** (default) | **realistic** |
|---|---|---|
| Goal | the picture CS2 shows | a physically lit version of the map |
| Level / screenshots | `/Game/OFAST/Maps/<map>`, `<map>_<view>.png` | `<map>_realistic`, `<map>_realistic_<view>.png` |
| Sun | CS2's `light_environment` × `LUX_PER_UNIT` (9.42 lux on de_mirage), CS2's colour | 100 000 lux at 5800 K, tinted by the atmosphere, CS2's direction |
| Sky | CS2's `env_sky` HDR on a sphere + Sky Light from it | Unreal's Sky Atmosphere + real-time Sky Light, light height fog |
| Local lights | CS2-matched candela (`intensity_cd`), CS2 bounce × gain | Hammer's real candela (`intensity_cd_photometric` = 79.6 × linear), plain bounce |
| Exposure | fixed, measured on CS2 (EV100 0.322 on de_mirage) | auto (histogram), EV100 2 – 16 |
| Tone curve | CS2's (a straight line or Hable's), then CS2's colour-correction table | Unreal's filmic curve, local exposure, light bloom and vignette |

The realistic style does **not** keep CS2's light ratios: it pairs Hammer's photometric candela with a real-world
sun (100 000 lux before the atmosphere, about 80 000 on the ground), so a 121 cd wall lamp is far fainter next to the
sun than CS2 shows it (CS2's sun is 9.42 lux in its own units). Interiors are lit mostly by daylight coming in, and
the auto exposure brightens them like a camera would. `-Disable` works in this style too. It needs
the project setting `r.DefaultFeature.AutoExposure.ExtendDefaultLuminanceRange` (set in the sandbox's
`DefaultEngine.ini`) for its EV100 limits.

## Fog — `env_cubemap_fog`

Distance fog coloured by the sky cube map: opacity = `max_opacity × ((distance − start) / (end − start))^falloff`,
faded with height between `heightstart` and `heightend`. de_mirage: start 384, end 8000, falloff 1.6, height
−500 … 5000. Written to the manifest; not reproduced in Unreal because it has no visible effect on de_mirage (see the
calibration notes above). Maps with heavy fog would need it (Unreal's height fog falls off exponentially, CS2's with
a power law).

## Not used yet

- Light probes (`env_combined_light_probe_volume`), lightmaps, `env_cubemap` reflections: baked data Lumen replaces.
- Bloom and fog: no measurable effect in CS2 on de_mirage (bloom: 0.7 % on de_inferno), so not applied (their
  settings are in the manifest).

## Sources

- ValveResourceFormat (MIT): `Renderer/Renderer/World/WorldLightingInfo.cs`, `SceneEnvironment/SceneLight.cs`,
  `ScenePostProcessVolume.cs`, `SceneSkybox2D.cs`, `Entities/EnvSky.cs`, `EnvCubemapFog.cs`, shaders
  `common/pbr.slang`, `post_processing.frag.slang`, `sky.frag.slang`, `common/fog.slang`
- Hammer's FGDs in the CS2 install: `game/core/lights_base.fgd`, `lights2.fgd`, `postprocessing.fgd`
- Unreal 5.8 source: `PostProcessEyeAdaptation.cpp` (manual exposure), `Scene.h` (tone curve settings),
  `InterchangeGltfTranslator.cpp` (light units), `InterchangeGenericTexturePipeline.h` (`.hdr` → cube)
- Measurements, not sources: the tone curve and the colour-correction table were read off CS2 screenshots
  (de_anubis, de_inferno), as described above
