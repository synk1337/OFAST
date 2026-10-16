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
| `bouncescale` | 1.3 | bounce strength in the bake | Indirect Lighting Intensity |
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
  `size_params` are **half**-sizes (Hammer's defaults 16 × 16 with 80 cd and 120 nits only agree for 32 × 32 units;
  likewise the omni2 default 9816 nits ↔ a sphere of radius 2); `light_spot` uses `innerconeangle` / `outerconeangle`.
- `range` is where the light has faded out (linearly over the last `skirt` fraction); `bouncescale` scales its bounce
  in the bake (≈ Unreal's Indirect Lighting Intensity).
- de_mirage's lights are `directlight` 1 ("Static": fully baked into the lightmaps, bounce included).

In the glTF they are `KHR_lights_punctual` approximations (rect → wide spot) for any tool. Unreal's Interchange drops
the glTF `range` (every light gets a 10 m attenuation radius), so `import_map.py` replaces them with Unreal Point,
Spot and **Rect** lights built from the manifest's `lights` list: exact shape, size, range, colour and bounce, movable.

**Unreal rect lights shine twice their candela value** along their axis: `RectLightSceneProxy.cpp` sets the luminance
to intensity / (0.5 × width × height), while point and spot lights use the intensity as is. `import_map.py` halves
rect lights' candela; without it de_mirage's palace came out 1.48× too bright, with it 0.81× (like the other views).

## Sky — `env_sky`

`skyname` is a `sky.vfx` material; its `g_tSkyTexture` is an HDR **cube map** (BC6H, 6 faces of 512², flag
`CUBE_TEXTURE`). Face layout and orientation: see [vtex.md](vtex.md#cube-maps). The sky shows
`cube(R⁻¹ · view direction) × tint_color/255 × brightnessscale × 2^(g_flBrightnessExposureBias +
g_flRenderOnlyExposureBias)`, with `R` the entity's rotation. When the map has a 3D skybox, the `env_sky` **inside the
skybox world** is the one drawn, so it also turns with the `skybox_reference` angles (de_mirage's has yaw 146.25° and
a reference at 0°; the sky is symmetric around the vertical, so it shows little).

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

See [map.md](map.md#3d-skybox). It is ordinary geometry once placed; it is lit by the main map's sun in Unreal.

## Exposure and tone mapping — `post_processing_volume` + `.vpost`

| Source | Key | de_mirage |
|---|---|---|
| `post_processing_volume` (the `master` one) | `enableexposure`, `minexposure`, `maxexposure` | on, 0.8, 1.0 (linear scale) |
| | `postprocessing` | `lighting/postprocessing/de_mirage_prefab/de_mirage.vpost` |
| `.vpost` DATA `m_toneMapParams` | `m_flWhitePoint`, `m_flExposureBias` | 1.5, 0 |
| | shoulder / linear strength / linear angle / toe strength, num, denom | 0 / 0.001 / 0.001 / 1, 1, 1 |
| `.vpost` DATA `m_bloomParams` | `m_flScreenBloomStrength`, `m_flBloomThreshold` | 0.126, 1.055 |
| `.vpost` DATA | `m_colorCorrectionVolumeData` | 32³ colour LUT (near identity, slightly more saturated) |

The tone curve is Hable's (Uncharted 2) filmic curve (Source 2 Viewer's `post_processing.frag.slang`); with the
stock parameters above it is a straight line. **Measured** on CS2 screenshots, the picture is

```text
display = clamp(exposure × scene, 0, 1)   then sRGB (then a light screen-blended bloom and the colour LUT)
```

The measurement: in a sunlit de_mirage view, pure-sky pixels — whose radiance is known exactly from the exported sky
— display at 0.80 × radiance (blue channel; red and green a little higher from the bloom), and 0.80 is the volume's
`minexposure`. Source 2 Viewer's reading, `exposure × scene / white_point`, would give 0.53: the white point does
not scale the picture (tool: `tools/compare/sky_exposure.py`). Auto exposure is clamped to [0.8, 1.0]; sunlit views
sit at 0.8, the palace too, the dark B apartments go up to 1.0. Its controller is Source 1's histogram tonemap
(cheat cvars `mat_tonemap_force_percent_target` "old default 45", `…_percent_bright_pixels`, `…_average_lum_min`).

For comparisons, **`mat_tonemap_force_scale <exposure>`** (cheat) locks CS2's exposure: a palace capture at
`mat_tonemap_force_scale 0.8` is identical to the automatic one (ratio 1.000), at 1.0 it is 1.23× brighter.
`mat_fullbright 1` shows the albedo only.

Unreal, with Manual exposure (physical camera off) and its tone curve off (`ToneCurveAmount` 0, `ExpandGamut` 0,
`BlueCorrection` 0), shows `luminance × 2^bias` (`PostProcessEyeAdaptation.cpp`; checked the same way: Unreal's
sky pixels display at exactly the intended ratio). A CS2 scene value of 1 is a luminance of `LUX_PER_UNIT / π`, so:

```text
EV100 = log2(LUX_PER_UNIT / (π × exposure × 2^bias))        Unreal exposure bias = −EV100
```

de_mirage: EV100 0.322 at exposure 0.8 (sunlit, used for screenshots), 0 at 1.0.

## Calibration against CS2 (de_mirage, 2026-10-16)

Same viewpoints in CS2 (`-insecure`, `setpos`/`setang`, HUD off, interiors at `mat_tonemap_force_scale 0.8` like
Unreal) and Unreal, compared cell by cell on a 4 × 3 grid with `tools/compare/compare_shots.py` (ratio of mean
linear luminance, Unreal / CS2). Views: `test-data/views/de_mirage.json` plus the two spawns.

| View | Whole image | Cells | Notes |
|---|---|---|---|
| CT spawn (sunlit street, sky) | 0.95 | 0.86 – 1.00 | sunlit ground 0.97 – 1.00, sky 0.96; shade 0.86 – 0.90 |
| T spawn (sunlit + large shaded walls) | 0.86 | 0.76 – 1.00 | sunlit ground 0.98 – 1.00; shaded walls 0.76 – 0.83 |
| Palace interior (`light_rect` wall lights) | 0.81 | 0.51 – 0.88 | wall facing the lights 0.84 |
| B apartments (`light_omni2` ceiling lights) | 0.74 | 0.52 – 0.92 | lit walls 0.74 – 0.92, floor 0.55 – 0.63 |

Before calibration (Unreal template lighting) the CT view was 0.91 overall but 0.38 in shade and 0.61 in the sky.
Before the rect-light fix the palace was 1.48.

How the interior was pinned down (palace): exposure ruled out (`mat_tonemap_force_scale`), sky and sun ruled out
(`OFAST_DISABLE=sun,sky`: still 1.41), bounce ruled out (`OFAST_DISABLE=…,bounce`: direct light alone already 1.3× CS2's
total). CS2's actual light on the wall, from lit ÷ albedo against the sunlit CT ground, is 1.7× the formula's direct
light (its bounce), so the formula was right and Unreal over-delivered: the rect-light factor above.

What remains is **shade and indirect light 10 – 25 % darker** than CS2, everywhere: CS2's baked bounce (lightmaps,
per-light `bouncescale`) is stronger than Lumen's, and its screen-blended bloom (strength 0.126) lifts dark tones.
No colour LUT (CS2's is close to identity, slightly more saturated) and no fog yet.

## Fog — `env_cubemap_fog`

Distance fog coloured by the sky cube map: opacity = `max_opacity × ((distance − start) / (end − start))^falloff`,
faded with height between `heightstart` and `heightend`. de_mirage: start 384, end 8000, falloff 1.6, height
−500 … 5000. Written to the manifest; not reproduced in Unreal yet (Unreal's height fog falls off exponentially).

## Not used yet

- Light probes (`env_combined_light_probe_volume`), lightmaps, `env_cubemap` reflections: baked data Lumen replaces.
- The colour-correction LUT, bloom and fog (in the manifest, not applied).

## Sources

- ValveResourceFormat (MIT): `Renderer/Renderer/World/WorldLightingInfo.cs`, `SceneEnvironment/SceneLight.cs`,
  `ScenePostProcessVolume.cs`, `SceneSkybox2D.cs`, `Entities/EnvSky.cs`, `EnvCubemapFog.cs`, shaders
  `common/pbr.slang`, `post_processing.frag.slang`, `sky.frag.slang`, `common/fog.slang`
- Hammer's FGDs in the CS2 install: `game/core/lights_base.fgd`, `lights2.fgd`, `postprocessing.fgd`
- Unreal 5.8 source: `PostProcessEyeAdaptation.cpp` (manual exposure), `Scene.h` (tone curve settings),
  `InterchangeGltfTranslator.cpp` (light units), `InterchangeGenericTexturePipeline.h` (`.hdr` → cube)
