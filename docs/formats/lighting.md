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
- `range` is where the light has faded out: full strength up to `range × (1 − skirt)`, then linear down to nothing
  at `range` (Source 2 Viewer's `SceneLight.ComputeDistanceFade`; `skirt` defaults to 0.1, and 0 means no fade before
  the cut — most of de_nuke's lights). `bouncescale` scales its bounce in the bake (≈ Unreal's Indirect Lighting
  Intensity, × 2 — see the bounce gains).
- `directlight`: 1 "Static" (fully baked into the lightmaps, bounce included; all of de_mirage's lights but six),
  3 "Stationary" (direct light at run time, baked shadows), 2 "Dynamic", and **0 "None": no direct light at all, the
  light only adds bounce to the bake** (de_mirage 6, de_dust2 1, de_inferno 17, de_nuke 6 — fill lights by windows
  and doors, e.g. the one 94 units from de_dust2's pit wall).

In the glTF they are `KHR_lights_punctual` approximations (rect → wide spot) for any tool. Unreal's Interchange drops
the glTF `range` (every light gets a 10 m attenuation radius), so `import_map.py` replaces them with Unreal Point,
Spot and **Rect** lights built from the manifest's `lights` list: exact shape, size, range (`attenuation_radius`,
with `skirt`), colour, bounce and `direct` (the `directlight` value), movable.

**Range.** Unreal's inverse-square lights do not keep full strength up to a cut: they fade over their whole
attenuation radius with the window (1 − (d / radius)⁴)², which is 0.35 at 80 % of the radius and 0.01 at 97 %. CS2
maps often set a range just long enough to reach the floor (de_dust2's lower-tunnel lamp: range 205, 199 units above
the floor), so a radius equal to the range left those floors almost unlit: de_dust2's lower tunnel 0.32 of CS2,
de_nuke's B site 0.50 and its decontamination room 0.24 ([checks.md](../checks.md)). `import_map.py`
(`light_radius`) sets the radius to range × (1.375 − 0.725 × skirt): the factor that fits the window to CS2's fade
best (least squares, weighted by 1 / distance²), within 0.04 for any skirt from 0 to 1 — 1.375 for skirt 0, 1.3 for
the default 0.1. Some light then reaches a little past CS2's cut, and a little less arrives just before it.

**Bounce-only lights (`directlight` 0).** Unreal has no light that only bounces, but Lumen lights its surface cache
with the light's colour × Indirect Lighting Intensity × Diffuse Scale (`LumenSceneDirectLighting.cpp` / `.ush`),
where the direct light gets colour × Diffuse Scale only. `import_map.py` gives these lights a Diffuse Scale of 0.02,
no specular, and an Indirect Lighting Intensity 50× higher: the bounce stays as for any light, 2 % of the direct
light is left. Before, they lit their surroundings directly in Unreal, where CS2 shows only their bounce.

**Many shadowed lights.** With the engine's virtual shadow map limits (a pool of 4096 pages, 16 lights per pixel in
one-pass projection) de_nuke's 174 shadowed local lights overflow both ("Virtual Shadow Map Page Pool overflow (1413
page allocations were not served), this will produce visual artifacts (missing shadow)" in the log): whole pages of
shadow go missing, which showed as blocky patches of light on de_nuke's B site that changed shape from one render to
the next. The sandbox's `DefaultEngine.ini` (`[SystemSettings]`) and `import_map.py` / `shoot_views.py`
(`RENDER_CVARS`) raise them to 16384 pages and 32 lights per pixel: the page pool no longer overflows. 32 is the
engine's maximum (`VirtualShadowMapArray.cpp` clamps it); de_nuke still logs one light-list overflow per render, which
only costs the lights beyond 32 at a pixel their full shadow filtering. CS2 itself logs "failed to allocate … shadow
from atlas" on de_nuke.

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
— display at 0.80 × radiance, and 0.80 is the volume's `minexposure`. Source 2 Viewer's reading,
`exposure × scene / white_point`, would give 0.53: the white point does not scale the picture (tool:
`tools/compare/sky_exposure.py`). Read through the colour-correction table and at CS2's default brightness
([below](#cs2s-brightness-setting-r_fullscreen_gamma)), all three channels agree: 1.02, 1.02, 0.98 × the exposure
(`tone_fit.py sky`; the red and green that first read 6 – 11 % high were the brightness setting); de_nuke's sky gives
0.97 – 1.00 at exposures 0.2 – 0.8. Auto exposure is clamped to [0.8, 1.0]; sunlit views
sit at 0.8, the palace too, the dark B apartments go up to 1.0. Its controller is Source 1's histogram tonemap
(cheat cvars `mat_tonemap_force_percent_target` "old default 45", `…_percent_bright_pixels`, `…_average_lum_min`).

For comparisons, **`mat_tonemap_force_scale <exposure>`** (cheat) locks CS2's exposure: a palace capture at
`mat_tonemap_force_scale 0.8` is identical to the automatic one (ratio 1.000), at 1.0 it is 1.23× brighter.
`mat_fullbright 1` shows the albedo × 1, linearly: it ignores the locked exposure (de_anubis at 1.0 and 0.5: identical
to 0.002 %) and the tone curve (fullbright against Unreal's base colour is flat over the whole brightness range).
Unreal's `ShowFlag.Lighting 0` render, its base colour, skips the manual exposure (exposure 1: its sky pixels fit
exposure 1.0 exactly) but keeps OFAST's tone mapper.

Fullbright is not always the albedo the lit shader uses. On de_anubis and de_vertigo it agrees with Unreal's base
colour to 0 – 8 %; on de_mirage Unreal's base colour is only 0.70× it in the CT spawn (0.78 palace, 0.88 B
apartments) and on de_inferno 0.5 – 0.6×, and there CS2's own lit ÷ fullbright on sunlit ground is less than the sun
alone gives: 1.64 – 1.73 units on de_mirage's CT-spawn cobbles, where the direct sun is 3 × 0.773 (its colour's
luminance) × sin 60° = 2.01 and Unreal's light (lit ÷ base colour) is 2.43 – 2.46. So on those maps fullbright shows
more than the lit albedo — on de_mirage 1.2 – 1.5× (B apartments … CT spawn), taking the lit albedo from the sunlit
surfaces below — something the lit shader applies and fullbright skips, not found yet; and
`tone_fit.py light` splits light from material wrongly there; [the calibration](#against-cs2s-default-picture-2026-10-18)
uses the sunlit surfaces instead.

Unreal, with Manual exposure (physical camera off) and its tone curve off (`ToneCurveAmount` 0, `ExpandGamut` 0,
`BlueCorrection` 0), shows `luminance × 2^bias` (`PostProcessEyeAdaptation.cpp`; checked the same way: Unreal's
sky pixels display at exactly the intended ratio). A CS2 scene value of 1 is a luminance of `LUX_PER_UNIT / π`, so:

```text
EV100 = log2(LUX_PER_UNIT / (π × exposure × 2^bias))        Unreal exposure bias = −EV100
```

de_mirage: EV100 0.322 at exposure 0.8 (sunlit, used for screenshots), 0 at 1.0.

**When `minexposure` > `maxexposure`** (de_nuke: 1.0 and 0.85), the minimum wins: CS2's automatic picture equals a
`mat_tonemap_force_scale 1` one to 0.1 %. OFAST uses the minimum too. Forced exposures are applied exactly: at CS2's
default brightness every halving (1.6 → 0.8 → 0.4 → 0.2, de_nuke and de_vertigo) scales the picture's linear values
by 0.495 – 0.503 once it is read back through the table (and the curve), at every brightness; a second shot 8 – 60 s
later matches the first to 0.3 % (0.8 % with drifting clouds). The 0.855 first measured for a forced 0.85 was the
brightness setting.

### CS2's brightness setting (`r_fullscreen_gamma`)

CS2's Brightness option is the cvar `r_fullscreen_gamma` (default 2.2, saved in Steam's
`userdata/<id>/730/local/cfg/cs2_machine_convars.vcfg`). It is applied last, to the picture as shown:

```text
shown = default picture ^ (r_fullscreen_gamma / 2.2)        (both sRGB-encoded, 0 – 1)
```

Measured 2026-10-17 on de_vertigo's T spawn, the same view at 2.013074 (this machine's setting) and 2.2: the first is
the second to the power 0.916 at every brightness (2.013 / 2.2 = 0.915); de_nuke's reference from the afternoon
before matches a new 2.2 shot to the power 0.917. **Every CS2 reference in `test-data/shots` before 2026-10-17 was
taken at 2.013**, brighter than CS2's default: a mid-grey shows 13 % brighter (linear), a white 4 %, a halving of the
exposure shows as 0.53, and a darkening colour table looks like a brightness loss (de_nuke, below). The calibrations
until then were made against that brighter picture.

OFAST reproduces CS2's default picture. Take references at `r_fullscreen_gamma 2.2` (and set the value back
afterwards), or pass the value they were taken with to `compare_shots.py`, `compare_views.py` and `tone_fit.py` as
`--cs2-gamma`, which
undoes it (`default = shown ^ (2.2 / gamma)`). `mat_fullbright` shots go through it too.

### Non-linear (Hable) curves

de_inferno's `.vpost` has Hable's stock parameters instead: shoulder 0.15, linear strength 0.5,
linear angle 0.1, toe strength 0.2, toe numerator 0.02, toe denominator 0.3, white point 4. So do most official maps:
cs_italy, cs_office, cs_shelter, de_ancient, de_ancient_night, de_anubis, de_cache, de_inferno, de_train and
de_vertigo. Only de_dust2, de_mirage, de_nuke and de_overpass are linear. de_boulder's `.vpost` has no tone-map block,
and de_debris, de_eldorado, de_fachwerk and de_poseidon have no `.vpost` that `map_entities` finds.

Halving the exposure does not halve the picture there: on de_anubis the darkest pixels drop to a third (×0.32) and
the brightest only to ×0.79. With `H(x) = (x(Ax + CB) + DE) / (x(Ax + B) + DF) − E/F` (the `.vpost`'s curve), W its
white point and LUT its colour-correction table, CS2's default picture is:

```text
shown sRGB = LUT( sRGB( H(k × exposure × scene) / H(w × W) ) )        k = 2.85, w = 4.2
```

Measured with `tools/compare/tone_fit.py` (2026-10-16 night and 10-17) on **de_anubis**, the map to measure on (its
visible sky, the 3D skybox's `env_sky`, is the same texture at the same brightness, 1.45, as the one that lights it;
no bloom, threshold 10; automatic exposure at `minexposure` 0.7 in both spawns), and on **de_vertigo**, the map with
the second-strongest table. Shots at the spawns at locked exposures (de_anubis 0.7 and 0.35; de_vertigo 1.6, 0.8,
0.4 and 0.2) and with `mat_fullbright 1`, views `tools/cs2/cfg/ofast_v_<map>_*.cfg`, all read as CS2's default
picture ([brightness](#cs2s-brightness-setting-r_fullscreen_gamma)). Method: invert CS2's shots through the table
(in 3D), sRGB and the curve back to exposure × scene, then compare pixel by pixel between an exposure pair, against
the exported sky's radiance on sky pixels (where Unreal's render of the same view shows nothing but its own sky), and
against Unreal's scene values (its shots inverted through its own rule) on lit cells.

- **The table** (below) is applied after the curve, to the sRGB-encoded picture. de_anubis's sends everything below
  27/255 to black: that, not Hable's toe, is why the darks drop to a third. Without it no `k` and `w` fit the
  exposure pairs; with it inverted, every pixel scales by the exposure ratio.
- **w = 4.2** (white 17): a pair halves every pixel the same, at the forced ratio (level 0.998 – 1.001), for
  w = 4.1 – 4.2 on both de_anubis spawns (fits 3.8 – 4.7); de_vertigo's six pairs give 3.7 – 5.6 and de_inferno's CT
  pair 3.2 – 3.4. A pair does not depend on `k` (it scales both shots).
- **k = 2.85**: at w = 4.2 sky pixels give the same k at every exposure: 2.84 on both de_anubis spawns (0.7 and
  0.35), 2.86 – 2.93 on de_vertigo (0.2 – 1.6). At another w, k drifts with the exposure (de_anubis at w = 5: 2.97 at
  0.7, 2.94 at 0.35). Checked again on the exports of the merged code (2026-10-18): de_anubis 2.83 – 2.85 (green;
  fitting k and w together: 2.85 and 4.4 W), de_inferno's CT spawn 2.82 – 2.83 (2.80 and 4.3 W).

| Anchor (CS2's default picture, rule k = 2.85, w = 4.2) | de_anubis | de_vertigo | de_inferno |
|---|---|---|---|
| white w from exposure pairs | 4.1 – 4.2 (CT, T) | 3.7 – 5.6 (CT, T; 1.6 / 0.8 / 0.4 / 0.2) | 3.2 – 3.4 (CT) |
| k from sky pixels at w = 4.2 | 2.84 (CT, T) | 2.90 – 2.93 (CT), 2.86 – 2.87 (T) | 2.82 – 2.83 (CT, green; CS2 draws the **skybox** `env_sky`, 0.8 × 2^1, which OFAST exports; the main world's, 0.5 × 2^1, would give 0.63 of it) |
| lit surfaces, k × CS2 scene / Unreal scene (median of 12 cells) | 2.65 (CT), 2.78 (T) | 2.45 (CT; cells 1.0 – 5.0), 1.63 (T) | 4.5 (CT; cells 3.4 – 4.9, merged code) |
| Unreal base colour / CS2 fullbright albedo | 1.08 (CT) | 1.01 (CT), 1.00 (T) | 0.5 – 0.6 (CT) |
| light, lit ÷ albedo (Unreal / CS2) | 1.00 (CT) | 1.05 (CT; 0.56 – 2.10), 1.87 (T; shaded floor 3 – 3.9) | 0.9 – 1.2 (CT) |
| automatic exposure | `minexposure` 0.7 | above `minexposure` 0.8: the auto picture is 1.38 – 1.41× the 0.8 one | |

So with this rule the sky agrees on all three maps, and where a picture still differs it is the scene: de_anubis's
surfaces are 5 – 8 % bright in Unreal (its base colours, 1.08), de_vertigo's shade is far too bright (the light, not
the materials: in its T spawn's covered hall Unreal's light is 2 – 4× CS2's, warm where CS2's is blue), and
de_inferno's surfaces are too dark (its materials, base colour 0.5 – 0.6× CS2's albedo; open). Its sunlit ground is
odd in CS2: lit ÷ fullbright gives fewer units of light than the sun alone, so for some de_inferno materials fullbright
may not show the albedo the lit shader uses.

Earlier rules: k = 3.4, w = 6 (2026-10-16 night) were measured the same way, but on shots taken at this machine's
brightness setting (2.013): fitted to the forced exposure ratio, the brighter picture's pairs pointed to w = 6 and
its sky to k = 3.4; read as the default picture, both pairs and sky point to the rule above (de_anubis
whole image against the default picture: 1.11 / 1.08 with the old rule, 1.04 / 1.02 with this one). Before that, a = 2.5, b = 1.25 and no table
were fitted on de_inferno's lit surfaces, whose materials are too dark; its fullbright anchor (3.8) assumed fullbright
goes through exposure and curve, and it does not.

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

OFAST applies the table on **every map** that has one. For a day (2026-10-18) it applied it on Hable maps only: on the
linear maps it was unconfirmed, and de_nuke's 17 levels moved its spawns from 0.98 / 0.96 to 0.92 / 0.90 of CS2's
screenshots — screenshots taken at a raised brightness setting, which is where that darkening came from.

**CS2 applies it on linear maps too** (de_nuke, whose table lowers the darks by up to 8 levels): at
CS2's default brightness, its CT spawn at exposures 1.6, 0.8, 0.4 and 0.2, read back through the table, halves every
pixel by 0.495 – 0.503 at every brightness, and 0.8 → 0.2 gives 0.246 – 0.251; without the table the same pairs
fall to 0.44 – 0.46 at the dark end (0.20 for the 4× step) and rise with brightness. Its sky pixels give exposure ×
radiance in all three channels (0.97 – 1.00) only through the table (without it, at 0.2: red 0.60, green 0.84,
blue 0.89). The darkening was the references' brightness setting
([above](#cs2s-brightness-setting-r_fullscreen_gamma)): against CS2's default picture de_nuke's spawns are 0.99 / 0.98
of CS2 with the table and 1.06 / 1.05 without it. The table was switched back on for linear maps (2026-10-18).

`tone_fit.py table` prints a map's table (grey diagonal and largest change); de_vertigo's is an S-curve (input 33 → 19,
181 → 199, 247 → 255), de_nuke's lowers the darks (16 → 8, 33 → 25).

### In Unreal

A post-process material at `BL_ReplacingTonemapper` (`M_OFAST_Tonemap_v3`), on every map: scene colour ×
eye-adaptation exposure → a straight line (shoulder 0) or `H(2.85 x) / H(4.2 W)` (`HABLE_INPUT_SCALE`,
`HABLE_WHITE_SCALE` in `import_map.py`) → sRGB → the map's table, imported as an uncompressed, non-sRGB texture
without mips and read with two bilinear lookups on neighbouring blue slices. Unreal applies nothing after a replacing
tone mapper. The curve's parameters come from the manifest's `post.tonemap`. Before this, linear maps used Unreal's
own tone mapper with its curve off, and Hable maps `M_OFAST_TonemapHable_v2` (H(2.5 x) / H(1.25 W), no table); from
2026-10-16 night to 10-17 the same material drew H(3.4 x) / H(6 W).

## Calibration against CS2 (de_mirage, 2026-10-16)

Same viewpoints in CS2 (`-insecure`, `setpos`/`setang`, HUD off, interiors at `mat_tonemap_force_scale 0.8` like
Unreal) and Unreal, compared cell by cell on a 4 × 3 grid with `tools/compare/compare_shots.py` (ratio of mean
linear luminance, Unreal / CS2). Views: `tools/compare/views/de_mirage.json` plus the two spawns. The references of
this section and the next were taken at brightness 2.013, brighter than CS2's default. The calibration was measured
again against the default picture on 2026-10-18 ([below](#against-cs2s-default-picture-2026-10-18)): the constants
stand, and what reads too bright there is Unreal's base colours, not the light.

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

### Against CS2's default picture (2026-10-18)

The constants above were fitted on references taken at brightness 2.013 (CS2's picture raised to 2.013 / 2.2,
[above](#cs2s-brightness-setting-r_fullscreen_gamma)). Measured again on de_mirage against CS2's default picture: the
same references read with `--cs2-gamma 2.013074` and, on the CS2 side, back through de_mirage's colour table; Unreal:
the import of 2026-10-18 (colour table on every map, `test-data/shots/lut_all/`), the same views re-shot on the saved
level, and a `ShowFlag.Lighting 0` render of every view for the base colour (`shoot_views.py -Try`).

Whole image, the views read brighter against the default picture, the darker ones more: CT spawn 0.99 → 1.05, T spawn
0.99 → 1.06, palace 0.95 → 1.06, B apartments 0.72 → 0.85 (same renders, uncorrected → corrected references). To tell
light from material, each view's pixels are sorted by Unreal's own light (lit ÷ base colour, in 8 × 8-pixel blocks,
sky left out): **sunlit** (1.5 units or more, mostly the direct sun), outdoor **shade** (0.15 – 0.9), and indoors
everything (**lamp**-lit). The direct sun is the same formula in both engines (on de_anubis, where fullbright can be
trusted, the light matched to 1.00), so the sunlit ratio is the material — Unreal's base colour against CS2's lit
albedo — and shade ÷ sunlit within a view is the light alone. `mat_fullbright` cannot give the albedo on de_mirage
([above](#exposure-and-tone-mapping--post_processing_volume--vpost)). A second, local check pairs shaded and sunlit
blocks of the same Unreal base colour (each channel within 8 %, at most 10 blocks apart and at least 2 from the
shadow edge, where CS2's lightmap shadows are blurrier), which takes the material out block by block.

| View | Whole image | Sunlit | Shade / lamp-lit | Light relative to the sun | Same-material pairs |
|---|---|---|---|---|---|
| CT spawn | 1.05 | 1.07 | 1.02 | 0.95 | 0.93 |
| T spawn | 1.06 | 1.07 | 1.05 | 0.99 | 0.96 |
| Palace | 1.06 | — | 1.05 | 0.98 (against the map's sunlit 1.07) | — |
| B apartments | 0.85 | — | 0.84 | 0.78 | — |

Unreal / CS2 of the scene (each shot read back through its own exposure, curve and table); the other ten views are
in [checks.md](../checks.md#light-and-material-against-the-default-picture-2026-10-18). Sunlit surfaces read 1.03 –
1.10 in all seven outdoor views.

What each constant does, from what-ifs on the saved level (`shoot_views.py -Try` multipliers, every view; change of the
scene ratio against the import's values; the base configuration shot twice agreed to 0.1 %):

| Change | Outdoor shade | Sunlit | Palace | Connector, sniper's nest | Underpass | B apartments |
|---|---|---|---|---|---|---|
| `OUTDOOR_BOUNCE_GAIN` 1.25 → 1.0 / 1.5 | −8 … −16 % / +8 … +16 % | −1 … −6 % / +1 … +4 % | −2 / +2 % | −2 … 0 / 0 … +2 % | −13 / +13 % | 0 |
| `LOCAL_BOUNCE_GAIN` 2.0 → 1.5 / 2.5 | 0 … −2 / 0 … +2 % | 0 | −6 / +6 % | −10 … −12 / +10 … +12 % | −3 / +3 % | −2 / +2 % |
| rect lights × 0.9 | 0 | 0 | −9 % | 0 | 0 | −0.5 % |

**The constants stay.** Relative to the sun, the spawns' shade is 1 – 7 % darker than CS2's (0.95 / 0.99 by class,
0.93 / 0.96 by pairs) and the palace 2 %, about the spread between the two methods. So neither the bounce gains nor
the rect-light factor is what makes de_mirage 5 – 6 % bright against the default picture: its sunlit surfaces are
7 % bright too, i.e. Unreal's base colours are about 7 % above CS2's lit albedo, as on de_anubis (1.08). Lowering the
gains to hide that would leave the shade about 10 – 15 % dark relative to the sun once the base colours are right.
What the light still gets wrong is local, and the same gain pulls it both ways:
- outdoor shade at B site, the B-apartments balcony and middle is dark (0.73 – 0.92 relative to the sun), like the
  B-apartments corridor (0.78): bounce Lumen carries only partly; raising `OUTDOOR_BOUNCE_GAIN` to 1.5 would bring
  them to 0.78 – 0.98 but the T spawn's shade to 1.06 – 1.10;
- the lamp-lit connector and sniper's nest are 1.17 / 1.18 relative to the sun: `LOCAL_BOUNCE_GAIN` 1.5 would bring
  them to 1.04 – 1.06 but the palace to 0.93 and the B apartments to 0.77; the palace, lit by `light_rect`s only, is
  the cleaner calibration;
- the underpass reads 1.21 and follows the outdoor gain (13 % for 20 %), not its hanging lamp (3 %): sky light
  reaching into an enclosed room.

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

Ten to fourteen more views per map (bomb sites, routes, interiors, lower levels) are in [checks.md](../checks.md),
with the fixes they led to (light range, bounce-only lights, shadow-map limits, start-disabled entities). Its numbers
undo CS2's brightness setting in the references, which the tables above do not.

### After the tone-curve correction (2026-10-16 night)

With the rule measured then (k = 3.4, white 6 W, the colour table on every map; since refitted,
[above](#non-linear-hable-curves)), re-imported and compared again (Unreal / CS2, whole image). The real imports reproduce
the rule's simulation on the earlier Unreal shots to 0.1 % (de_anubis, de_nuke).

| View | Before | Now | Notes |
|---|---|---|---|
| de_anubis CT / T | 1.37 / 1.32 (H(2.5 x) / H(1.25 W)) | 1.02 / 1.00 | new map; cells 0.93 – 1.15 / 0.96 – 1.07, sky 0.98 – 0.99 |
| de_inferno CT / T | 0.85 / 1.46 | 0.74 / 1.16 (simulated) | not re-imported here (the export in `test-data` had moved to a newer format); CT: sky 1.05, surfaces 0.73 — the base colours, above |
| de_mirage CT / T / palace | 1.00 / 0.97 / 0.96 | 1.00 / 0.97 / 0.95 | the table is nearly the identity |
| de_mirage B apartments | 0.61 | 0.71 | the table moves dark pixels by a few % (down); the rise is something else in the shared sandbox or run-to-run, not looked into |
| de_dust2 T / CT | 0.95 / 0.11 | 0.95 / 0.095 | |
| de_nuke CT / T | 0.98 / 0.96 | 0.92 / 0.90 | applying its table (17 levels) darkens the picture: the one map where the table, unconfirmed on linear maps, makes the match worse; see [the table](#the-colour-correction-table) |

After merging all branches (2026-10-18), with the colour table on Hable maps only and the fixes of
[checks.md](../checks.md), against the same uncorrected references: de_mirage 0.99 / 0.98, palace 0.96, B apartments
0.76; de_dust2 0.27 / 0.98 (the CT room gained from the lamp-range fix); de_inferno 0.76 / 1.22 (re-imported now);
de_nuke 0.98 / 0.95; de_anubis 1.01 / 1.01. Corrected for this machine's CS2 brightness setting (checks.md), de_anubis
reads 1.11 / 1.08: the level 3.4 was fitted on uncorrected references and should be refitted on corrected ones
(done below).

### Against CS2's default picture

Measured 2026-10-17 / 18. Every reference above was taken at this machine's brightness setting (`r_fullscreen_gamma`
2.013), which brightens CS2's picture ([above](#cs2s-brightness-setting-r_fullscreen_gamma)). Read as CS2's default picture
(`compare_shots.py --cs2-gamma 2.013074`; de_vertigo's T spawn taken again at 2.2), the Hable maps with the level
measured on that picture (k = 2.85, w = 4.2), Unreal / CS2, whole image; de_anubis and de_inferno re-imported with the
merged code (renders in `test-data/shots/refit/`), de_vertigo with the code of 2026-10-17:

| View | Brighter references, k 3.4 | Default picture, k 3.4 | Default picture, k 2.85 | Notes |
|---|---|---|---|---|
| de_anubis CT / T | 1.01 / 1.01 | 1.11 / 1.08 | 1.04 / 1.02 | lit surfaces 2.67 / 2.77 against k = 2.85: Unreal's base colours are a few % above CS2's albedo (1.08 in the CT view) |
| de_inferno CT / T | 0.76 / 1.22 | 0.81 / 1.44 | 0.75 / 1.30 | sky right (k 2.82); surfaces too dark from its materials (base colour 0.5 – 0.6× CS2's albedo); its 12 other views in [checks.md](../checks.md#after-merging-all-branches-2026-10-18) |
| de_vertigo CT / T | — | 1.42 / 2.04 | 1.27 / 1.82 | new map; the shade is too bright (light 2 – 4× CS2's in the T spawn's hall), materials right (base colour 1.00 – 1.01) — open |

The real imports reproduce `tone_fit.py simulate` to 0.1 – 0.2 % (de_vertigo CT and T, re-toned from k = 3.4, w = 6).
The linear maps do not use k and w; their numbers against the default picture are in checks.md. Their lighting
calibration (bounce gains, rect lights, fitted at 2.013 on de_mirage) was measured again against the default picture
([above](#against-cs2s-default-picture-2026-10-18)): the light holds; de_mirage's spawns and palace read 5 – 6 %
bright because its sunlit surfaces do, i.e. the base colours.

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

## Translucent surfaces and overlays

Unreal lights a translucent material apart from the opaque scene, with its **translucency lighting mode**:

| Mode | Direct light | Indirect light (Lumen) | Who used it |
|---|---|---|---|
| Volumetric NonDirectional | translucency lighting volume, no normal | translucency volume | a new Unreal material: OFAST's environment and tint-mask masters before decision 010 |
| Surface TranslucencyVolume (`TLM_SURFACE`) | translucency lighting volume (blurry, diffuse only) | translucency volume | Interchange's glTF master for `BLEND` (`/InterchangeAssets/gltf/M_Default`) |
| Surface ForwardShading (`TLM_SURFACE_PER_PIXEL_LIGHTING`) | per pixel, with the shadow maps, specular | translucency volume | OFAST's translucent surfaces now |

The translucency lighting volume and Lumen's translucency volume are coarse grids around the camera, not the
per-pixel light and virtual shadow maps of opaque surfaces. A translucent surface was therefore lit less than the
opaque ones next to it in direct sun: de_nuke's road paint came out at 0.97× the sunlit asphalt's luminance (CS2
2.13–2.35× depending on exposure), the SWAT van's lettering and de_dust2's container sign at 0.81 and 0.70 of what per-pixel lighting gives.

OFAST now handles translucent surfaces in two ways ([material.md](material.md#translucent-overlays-mesh-decals)):

- **Surfaces lying on another one are mesh decals**, blended into the GBuffer before lighting, so they are lit with
  their ground as CS2 lights them with their ground's lightmap texel: the overlays
  ([decision 009](../decisions/009-overlays-as-mesh-decals.md); road paint 2.07× now) and the `csgo_complex`
  decals with `F_DEPTH_BIAS` ([decision 010](../decisions/010-translucent-decals-and-per-pixel-lighting.md):
  de_inferno's plaster patches, de_nuke's drain covers and van lettering).
- **Every other translucent surface is lit with Surface ForwardShading** (glass, fence and foliage cards, signs):
  `import_map.py` re-parents Interchange's instances to an OFAST copy of its master with that mode
  (`M_OFAST_glTFDefault_v1`), and sets it on OFAST's own masters whose instances can be translucent. Static overlays
  in their other blend modes have their own handling (Mod2x: an unlit Modulate material,
  [decision 011](../decisions/011-static-overlay-blend-modes.md); per pixel, a Mod2x scuff got brighter in the sun
  and darker in the shade), and the unlit `csgo_unlitgeneric` cards keep Interchange's mode for now.

Per-pixel lighting fixes the direct sun only: the indirect light still comes from Lumen's translucency volume, so
in the shade a translucent surface stays darker than the screen and surface-cache lighting of the wall beside it.
That is why decals are decals: de_inferno's plaster patch on a shaded wall gets 0.62 of the wall's light factor
(luminance / base colour, `tools/compare/light_factor.py`) per pixel and 0.80 as a decal, CS2 about 0.88 (CS2
shows it brighter than the wall, as the decal does); the road paint in the shade was 1.50× per pixel and 1.86× as
a decal (CS2 2.29×).

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
