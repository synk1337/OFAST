"""Smoke grenades' clouds in Unreal: one Heterogeneous Volume per smoke, drawn from the atlas `ofast export-demo` wrote
next to the replay (crates/ofast-core/src/export/smoke.rs; docs/formats/replay.md, "Smoke volumes";
docs/decisions/017-grenades-and-effects.md). Used by import_demo.py, and by probe_smoke.py to try the look.

- SMOKE_MATERIAL: one Volume-domain material for every smoke. A Custom node (SMOKE_HLSL) samples the smoke's atlas at
  the world position and the demo tick; extinction = that density, broken into puffs by a noise (SMOKE_NOISE), grown
  out of the grenade over its first seconds (GROW_*) and shaped (SMOKE_THRESHOLD, SMOKE_RAMP), x `Extinction`;
  albedo = `Albedo`. Rebuilt in place on every run, so the smokes' instances keep their parent.
- REPLAY_COLLECTION: a material parameter collection whose `ReplayTick` the sequence keys with the demo tick
  (key_replay_tick: two linear keys); every smoke reads it, so the volumes need no keys of their own.
- Per smoke: its atlas imported as a linear grayscale texture without mipmaps, a material instance with the atlas, the
  grid's corner and size, its frames' first tick and spacing, its fading and expiry, its tint; and a Heterogeneous
  Volume actor covering the grid (place_smokes).

Units: Heterogeneous Volumes ray-march in the volume's own space, so `Extinction` is per local unit, here a cell of
the SMOKE_VOLUME_RESOLUTION grid (640 units / 64 = 25.4 cm), not per cm: place_smokes converts SMOKE_EXTINCTION.
"""

import math
import os
import re

import unreal

SMOKE_MATERIAL = "/Game/OFAST/Replays/M_Smoke"
REPLAY_COLLECTION = "/Game/OFAST/Replays/MPC_Replay"
UNITS_TO_CM = 2.54
# The Heterogeneous Volume's cells per axis, which set its ray-march step (half a cell): 640 units / 64 = 10 units,
# half a smoke voxel.
SMOKE_VOLUME_RESOLUTION = 64
# Extinction of a full smoke voxel, per cm: 0.1 hides what is 30 cm behind (CS2's smokes are opaque a step in).
SMOKE_EXTINCTION = 0.1
# CS2 draws smoke above 5 % of a full voxel (5stack's notes on its smoke simulation, facts only), but OFAST's own
# simulation's densities are its own fit, not CS2's numbers: at 5 %, and opaque 10 % above that, OFAST's cloud was
# thin a voxel or two short of where CS2's ends. Round 14 of FaZe vs Falcons on de_anubis (2026-11-03): through
# m0NESY's eyes at 138411 CS2 fills the doorway at the top of the stairs with smoke 454 up to its lintel, where
# OFAST's cloud, 1 – 12 % dense there, showed the windows through a haze (the user's "the smoke is see-through"). So
# OFAST draws it from 1 %, opaque SMOKE_RAMP above that (CS2's cloud shows a clear surface, not a slow fade into the
# air: at 0.15 its edge was a soft blur beside CS2's crisp puffs).
SMOKE_THRESHOLD = 0.01
SMOKE_RAMP = 0.04
# The smoke's albedo: a light grey SMOKE_TINT of the way to its team colour (m_vSmokeColor), times SMOKE_BRIGHTNESS,
# divided by the map's sun colour to the power SUN_COLOUR_SHARE (sun_compensation). CS2 lights its smokes with less
# of the sun's colour than Unreal's light gives them, and shows its teams' smokes apart. Round 14 of FaZe vs Falcons on
# de_anubis (2026-11-04), whose sun is orange (linear 1, 0.62, 0.33): CT smoke 343 in the sun through m0NESY's eyes
# is a neutral grey in CS2 (linear 1 : 0.84 : 0.65, median luminance 157, its darkest tenth 128) where Unreal's was
# tan (1 : 0.76 : 0.50, 190 and 168); T smoke 262 in the shade of a passage is tan-brown in CS2 (red over green 1.24
# against CT's 1.08), where Unreal's two smokes were the same tan. Before that (a cool light grey 0.8, 0.82, 0.86 5 %
# of the way, no sun taken out, full brightness): half way, the T smoke at de_dust2's mid doors (round 17, 2026-10-31)
# was orange-tan, and a 15 % share still tan in the map's warm bounce light, where CS2's is grey with a beige cast in
# the sun. A sun drawn by its temperature (the realistic look's) is taken as white.
SMOKE_ALBEDO = (0.82, 0.82, 0.82)
SMOKE_TINT = 0.3
SUN_COLOUR_SHARE = 0.33
SMOKE_BRIGHTNESS = 0.65
# CS2's cloud is gone a little before smokegrenade_expired (0.4 s before it, nothing showed): the fade ends this many
# ticks early.
SMOKE_FADE_EARLY_TICKS = 48
# The cloud's surface is broken up by a fractal noise in world space (CS2's smokes show puffs a foot or two across,
# not the 20-unit voxels' smooth blobs); features about 1 / SMOKE_NOISE_SCALE cm across. At 0.12 and 60 cm the
# mid-doors smoke showed fine wisps where CS2's shows round billows 20 - 30 units across. Since 2026-11-03 the noise
# scales the density, from 1 − SMOKE_NOISE to 1 + SMOKE_NOISE times (its crests bulge out as puffs, its creases cut
# in), so the puffs are about as deep wherever the surface is; it moved the threshold up and down by 0.25 before,
# which ate holes into a cloud's thin reaches (1 – 12 % dense: smoke 454's doorway above, which showed the windows
# behind it through the holes). Since 2026-11-04 its features are about 110 cm across and it has two octaves (75 cm and
# four before): drawn at full resolution (SMOKE_CVARS), the finer octaves showed as many small wisps where CS2's
# cloud is a few big round billows, each shaded on its own (smoke 343 through m0NESY's eyes).
SMOKE_NOISE = 0.6
SMOKE_NOISE_SCALE = 1.0 / 110.0
SMOKE_NOISE_LEVELS = 2
# The noise rises through the cloud, SMOKE_RISE cm a tick (20 units a second): CS2's billows roll upward, where a
# noise fixed in the world left Unreal's smokes standing still (the user's report, 2026-10-31).
SMOKE_RISE = 20.0 * UNITS_TO_CM / 64.0
# How far the noise moves where the cloud is looked up (cm), and how much its threshold rises as it fades out
# (SMOKE_HLSL).
SMOKE_WARP_CM = 15.0 * UNITS_TO_CM
SMOKE_FADE_ERODE = 0.2
# How the cloud grows out of the grenade (the user's report, 2026-11-03: "the smoke appears way too suddenly, it
# should grow from 0 to 100 %"). OFAST's simulation, fitted to the bricks CS2's server sends, spreads its burst over
# about 180 units on its first step, where CS2 draws nothing yet. Round 14 of FaZe vs Falcons on de_anubis, played in
# CS2 at a quarter speed (2026-11-03): smoke 343 through m0NESY's eyes, 344 units away, nothing but the grenade's spark
# 0.25 s after it went off, a cloud about 170 units across and 140 high at 1.3 s, as big at 2.2 s; smoke 262, through
# a doorway, nothing 4 ticks on, a puff 40 units across at 0.3 s, the passage's width at 1 s. So the cloud is drawn
# only inside a dome round the detonation point that grows from GROW_DELAY s on: its radius GROW_SIZE (1 − e^(−t /
# GROW_S)) + GROW_ON t (units; t the seconds since GROW_DELAY), until it holds the whole grid (smoke_grow_end()). Its
# edge is soft over the last GROW_SOFT of the radius, which the threshold and the noise turn into a lumpy front; below
# the detonation it reaches down as far as the cloud does (smoke rolling down stairs). Its radius is 140 units at
# 1.3 s and 200 at 2.2 s, by when the simulation's own cloud is the smaller (at 90 units, smoke 343 at 1.3 s was
# about 0.7 of CS2's width).
# Those CS2 times were read off the recording's own clock, which runs 10 – 17 ticks behind the demo bar's tick
# (slow_shots, 2026-11-04). On the bar's ticks smoke 262 shows a first puff 0.5 – 0.6 s after it went off, a cloud
# 60 units across at 0.7 s, filling the passage at 0.8 s, about 220 units across and 150 high at 1 s; with the dome
# from 0.2 s and 130 units, Unreal's showed its puff at 0.45 - 0.5 s and was 165 across and 100 high at 1 s (the
# dome's sphere cut its top). So it starts later and grows faster: radius about 55 units at 0.6 s, 90 at 0.7 s, 115
# at 0.8 s, 165 at 1 s, 205 at 1.2 s (with 250 units, Unreal's was CS2's width from 0.8 s on, 0.7 of it at 0.7 s).
GROW_DELAY = 0.45
GROW_SIZE = 290.0
GROW_S = 0.8
GROW_ON = 40.0
GROW_SOFT = 0.3
# The dome is flatter than a sphere above the detonation point (its height over its radius 1 / GROW_FLAT), and its
# edge follows the noise (0.7 - 1.3 times its radius): with a sphere's clean edge the growing cloud showed as a smooth
# igloo where CS2's is a lumpy cloud wider than high (smoke 262 at 0.7 s, 2026-11-04).
GROW_FLAT = 1.2
# The grid's half-edge (units): the dome holds the whole grid once it reaches its corners.
GRID_HALF = 320.0


def grow_radius(seconds):
    """The growth dome's radius (units) `seconds` after the smoke went off."""
    t = max(seconds - GROW_DELAY, 0.0)
    return GROW_SIZE * (1.0 - math.exp(-t / GROW_S)) + GROW_ON * t


def smoke_grow_end():
    """Seconds after which the growth dome holds the whole grid (its soft edge past the grid's top corners): the cloud
    is then drawn as the simulation has it."""
    seconds = 0.0
    corner = GRID_HALF * math.sqrt(2.0 + GROW_FLAT * GROW_FLAT)
    while seconds < 60.0 and grow_radius(seconds) * 0.7 * (1.0 - GROW_SOFT) < corner:
        seconds += 0.1
    return seconds
# Console variables the smoke volumes need: indirect light from Lumen's translucency volume (single scattering), so a
# smoke in the shade is lit by the room around it, not black. The project sets them for every session
# (unreal/Sandbox/Config/DefaultEngine.ini, [SystemSettings]: the editor, Open in Unreal, shoot_sequence.py; before
# 2026-10-30 they were missing there and smokes in the shade showed black); import_demo.py and probe_smoke.py set them
# again for their own screenshots, and render_round.py's Movie Render Queue presets carry them.
# That light is occluded inside the cloud (the volume's ambient occlusion): without it, it filled the smoke evenly, a
# flat tan blob, where CS2's mid-doors smoke of round 17 is shaded in every crease and on its side in the wall's
# shadow (2026-10-31; the sun alone left that side black).
# And drawn at full resolution at every distance: by default Unreal draws Heterogeneous Volumes at half resolution
# (r.HeterogeneousVolumes.DownsampleFactor 2), scaled up, and coarser further away (CLOD 1), which blurred the smokes'
# puffs into a soft mottle beside CS2's crisp billows (round 14 of FaZe vs Falcons on de_anubis, 2026-11-04). And
# without stochastic filtering: in an offline render (Movie Render Queue) Unreal samples each voxel's lighting at a
# random place in it every frame, which its accumulation is meant to smooth; at full resolution a render's smoke was
# grain all over (a preview's, one sample a frame, and a final's, eight), where the editor's is not (it does not filter).
SMOKE_CVARS = [
    "r.HeterogeneousVolumes.IndirectLighting 1",
    "r.HeterogeneousVolumes.IndirectLighting.Mode 2",
    "r.HeterogeneousVolumes.AmbientOcclusion 1",
    "r.HeterogeneousVolumes.DownsampleFactor 1",
    "r.HeterogeneousVolumes.CLOD 0",
    "r.HeterogeneousVolumes.StochasticFiltering 0",
]


def apply_cvars():
    """Sets SMOKE_CVARS in the editor."""
    for command in SMOKE_CVARS:
        unreal.SystemLibrary.execute_console_command(None, command)


# The Heterogeneous Volume component's lighting settings (None keeps Unreal's default). By default Unreal lights a
# volume on a grid half as fine as its own (lighting_downsample_factor 2: 20-unit cells here) and marches its shadows
# in two-cell steps: coarser than the noise's puffs, whose creases then showed hardly any shade (the smoke a soft, flat
# blob beside CS2's crisp billows, 2026-11-04). On the volume's own grid, with one-cell steps, they are shaded.
SMOKE_COMPONENT = {"lighting_downsample_factor": 0.5, "shadow_step_factor": 1.0, "step_factor": None}


def configure_component(component):
    """Sets the smoke's volume component: its grid's resolution and SMOKE_COMPONENT."""
    component.set_editor_property("volume_resolution", unreal.IntVector(*[SMOKE_VOLUME_RESOLUTION] * 3))
    for name, value in SMOKE_COMPONENT.items():
        if value is not None:
            component.set_editor_property(name, value)
    log("smoke volume: " + ", ".join("%s %s" % (name, component.get_editor_property(name))
                                     for name in SMOKE_COMPONENT))

assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary


def log(message):
    unreal.log("OFAST: " + message)


# Helpers shared with grenade_effects.py (its sprite and flashed-view materials are built the same way).

def empty_material(path):
    """The material asset at `path` without expressions: made on first use, emptied on every later run, so it is
    rebuilt in place (its HLSL may have changed) and the instances keep it as their parent."""
    if assets.does_asset_exist(path):
        material = assets.load_asset(path)
        mel.delete_all_material_expressions(material)
        return material
    folder, name = path.rsplit("/", 1)
    return unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew())


def custom_node(material, code, output_type, input_names, x, y):
    """A Custom expression on `material` running `code`, with inputs of these names."""
    node = mel.create_material_expression(material, unreal.MaterialExpressionCustom, x, y)
    node.set_editor_property("output_type", output_type)
    node.set_editor_property("code", code)
    inputs = []
    for input_name in input_names:
        custom_input = unreal.CustomInput()
        custom_input.set_editor_property("input_name", input_name)
        inputs.append(custom_input)
    node.set_editor_property("inputs", inputs)
    return node


def material_instance(folder, name, parent):
    """The material instance asset `folder`/`name` of `parent`, made when missing and otherwise kept (updated in
    place: the level's actors keep it)."""
    path = folder + "/" + name
    if assets.does_asset_exist(path):
        instance = assets.load_asset(path)
    else:
        instance = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
            name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
    mel.set_material_instance_parent(instance, parent)
    return instance


def import_png(replay_path, file_name, folder):
    """A PNG the export wrote next to the replay, imported into `folder` (replacing an earlier import): the texture
    as the importer named it (it changes characters a name may not hold: spaces, dots, brackets)."""
    task = unreal.AssetImportTask()
    task.filename = os.path.join(os.path.dirname(replay_path), file_name)
    task.destination_path = folder
    task.automated = True
    task.replace_existing = True
    task.save = False
    unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    paths = list(task.imported_object_paths)
    texture = assets.load_asset(paths[0]) if paths else None
    if texture is None:
        raise RuntimeError("texture import failed: " + file_name)
    return texture


# REPLAY_COLLECTION's parameters and their defaults: `ReplayTick`, the demo tick the sequence shows (keyed by
# key_replay_tick), which the smoke volumes and the effects read; `EffectBrightness`, what the effects' unlit sprites
# and their lights are multiplied by (1 in CS2's look; in the realistic look, how much brighter its sun is than
# CS2's, keyed by the round's sequence: its auto exposure would leave them invisible otherwise).
REPLAY_PARAMETERS = {"ReplayTick": 0.0, "EffectBrightness": 1.0}


def replay_collection():
    """REPLAY_COLLECTION, made once, with REPLAY_PARAMETERS (one made before a parameter existed gains it)."""
    if assets.does_asset_exist(REPLAY_COLLECTION):
        collection = assets.load_asset(REPLAY_COLLECTION)
    else:
        folder, name = REPLAY_COLLECTION.rsplit("/", 1)
        collection = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
            name, folder, unreal.MaterialParameterCollection, unreal.MaterialParameterCollectionFactoryNew())
    parameters = list(collection.get_editor_property("scalar_parameters"))
    have = {str(p.get_editor_property("parameter_name")) for p in parameters}
    missing = [name for name in REPLAY_PARAMETERS if name not in have]
    for name in missing:
        parameter = unreal.CollectionScalarParameter()
        parameter.set_editor_property("parameter_name", name)
        parameter.set_editor_property("default_value", REPLAY_PARAMETERS[name])
        parameters.append(parameter)
    if missing:
        collection.set_editor_property("scalar_parameters", parameters)
        assets.save_loaded_asset(collection)
    return collection


def key_replay_tick(sequence, replay, end_frame, effect_brightness=1.0):
    """Keys REPLAY_COLLECTION's `ReplayTick` with the demo tick: two linear keys (the sequence runs one frame per
    tick), so sub-frame times get fractional ticks; and its `EffectBrightness`, constant."""
    tick0 = replay["start_tick"]
    track = sequence.add_track(unreal.MovieSceneMaterialParameterCollectionTrack)
    track.set_editor_property("mpc", replay_collection())
    section = track.add_section()
    section.set_range(0, end_frame)
    section.add_scalar_parameter_key("EffectBrightness", unreal.FrameNumber(0), float(effect_brightness),
                                     unreal.MovieSceneKeyInterpolation.CONSTANT)
    linear = unreal.MovieSceneKeyInterpolation.LINEAR
    # Parameter keys take their time in the sequence's tick resolution, not its display rate (one frame per demo
    # tick): keyed at display frames, the tick ran through the whole round in its first half second.
    display = sequence.get_display_rate()
    resolution = sequence.get_tick_resolution()
    for frame in (0, end_frame):
        time = round(frame * resolution.numerator * display.denominator / (resolution.denominator * display.numerator))
        section.add_scalar_parameter_key("ReplayTick", unreal.FrameNumber(time), float(tick0 + frame), linear)


# The smoke's density at a world position and demo tick, from its atlas (per frame 8 x 4 slices of 32 x 32 texels,
# slice z at column z % 8, row z / 8; texel = sqrt(density / full)). Two slices and two frames, blended: the hardware
# filters within a slice, kept half a texel off its edges so neighbouring slices never bleed in. The density is
# scaled by the noise (NoiseAmount) and shaped: nothing below the threshold, opaque Ramp above it; as the cloud fades
# (from the frame with the fading flag to FadeEnd) it turns translucent, its threshold rising by FadeErode only (risen
# to 1, the cloud was eaten into holes as it went: the user's report, 2026-10-31). Where it is looked up is moved by
# the noise, Warp cm at most along a slant: CS2's server marks voxels solid (a clip above a low wall too), and a cloud
# cut by them showed a flat plane where CS2's edge is lumpy. For its first GrowEnd seconds it is drawn only inside the
# growth dome round the detonation point, the grid's middle (GROW_*: its radius in cm, its times in seconds).
SMOKE_HLSL = r"""
if (Tick < FirstTick || Tick > FadeEnd) return 0.0;
// Grid coordinates, 0 - 1, in CS2's axes: Unreal's y is CS2's -y.
float3 g = float3(WorldPos.x - Origin.x, Origin.y - WorldPos.y, WorldPos.z - Origin.z) / Size;
g += (Noise - 0.5) * (Warp / Size) * float3(0.8, 0.5, 0.33);
if (any(g < 0.0) || any(g > 1.0)) return 0.0;
float grown = 1.0;
float age = (Tick - FirstTick) / TicksPerSecond;
if (age < GrowEnd)
{
    float since = max(age - GrowDelay, 0.0);
    float radius = GrowSize * (1.0 - exp(-since / GrowSeconds)) + GrowOn * since;
    float3 off = (g - 0.5) * Size;
    // Flatter than a sphere above the detonation point, and as lumpy as the noise: the front is a cloud's, not a
    // dome's (CS2's growing cloud is wider than high; a sphere's clean edge showed as a smooth igloo).
    off.z = max(off.z, 0.0) * GrowFlat;
    grown = saturate((1.0 - length(off) / max(radius * lerp(0.7, 1.3, Noise), 0.01)) / GrowSoft);
    if (grown <= 0.0) return 0.0;
}
float3 v = g * 32.0 - 0.5;
float z = clamp(v.z, 0.0, 31.0);
float z0 = floor(z);
float tz = z - z0;
float z1 = min(z0 + 1.0, 31.0);
float2 xy = clamp(v.xy, 0.0, 31.0) + 0.5;
float f = clamp((Tick - FirstTick) / TicksPerFrame, 0.0, Frames - 1.0);
float f0 = floor(f);
float tf = f - f0;
float f1 = min(f0 + 1.0, Frames - 1.0);
float d = 0.0;
for (int i = 0; i < 4; i++)
{
    float k = i < 2 ? f0 : f1;
    float s = (i & 1) == 0 ? z0 : z1;
    float w = (i < 2 ? 1.0 - tf : tf) * ((i & 1) == 0 ? 1.0 - tz : tz);
    // The frame's row and column without trusting k / Columns to be exact (GPUs divide by a reciprocal):
    // +0.5 keeps floor() off the integer.
    float row = floor((k + 0.5) / Columns);
    float column = k - row * Columns;
    float2 tile = float2(column * 256.0 + fmod(s, 8.0) * 32.0, row * 128.0 + floor(s / 8.0) * 32.0);
    d += w * Atlas.SampleLevel(AtlasSampler, (tile + xy) / float2(Width, Height), 0).r;
}
// The noise (0 - 1) scales the density: its round crests bulge out of the cloud's surface as puffs, its thin creases
// cut into it (the other way round, the crests carved round pits: a sponge, where CS2's cloud is cauliflower).
d *= d * grown * lerp(1.0 - NoiseAmount, 1.0 + NoiseAmount, Noise);
float fade = saturate((FadeEnd - Tick) / max(FadeEnd - FadeTick, 1.0));
float t = max(Threshold + (1.0 - fade) * FadeErode, 0.005);
return fade * saturate((d - t) / max(Ramp, 0.001));
"""
# The Custom node's scalar inputs, each a material parameter of the same name.
SMOKE_SCALARS = ["Size", "FirstTick", "TicksPerFrame", "Frames", "Columns", "Width", "Height", "FadeTick", "FadeEnd",
                 "Threshold", "Ramp", "NoiseAmount", "Warp", "FadeErode", "TicksPerSecond", "GrowEnd", "GrowDelay",
                 "GrowSize", "GrowSeconds", "GrowOn", "GrowSoft", "GrowFlat"]
# The material's own values (an instance sets the smoke's grid, timing and fading; the growth, in cm and seconds, is
# the same for every smoke).
SMOKE_DEFAULTS = {"Threshold": SMOKE_THRESHOLD, "Ramp": SMOKE_RAMP, "NoiseAmount": SMOKE_NOISE,
                  "Warp": SMOKE_WARP_CM, "FadeErode": SMOKE_FADE_ERODE, "TicksPerSecond": 64.0,
                  "GrowEnd": smoke_grow_end(), "GrowDelay": GROW_DELAY, "GrowSize": GROW_SIZE * UNITS_TO_CM,
                  "GrowSeconds": GROW_S, "GrowOn": GROW_ON * UNITS_TO_CM, "GrowSoft": GROW_SOFT,
                  "GrowFlat": GROW_FLAT}


def smoke_material():
    """SMOKE_MATERIAL, built on first use and rebuilt in place on every run (the HLSL may have changed; the smokes'
    instances keep it as their parent)."""
    material = empty_material(SMOKE_MATERIAL)
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_VOLUME)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_ADDITIVE)
    material.set_editor_property("used_with_heterogeneous_volumes", True)
    custom = custom_node(material, SMOKE_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT1,
                         ["Atlas", "WorldPos", "Tick", "Origin", "Noise"] + SMOKE_SCALARS, -400, 0)
    custom.set_editor_property("description", "OFAST smoke density")

    def connect(node, input_name):
        if not mel.connect_material_expressions(node, "", custom, input_name):
            raise RuntimeError("smoke material: could not connect " + input_name)

    atlas = mel.create_material_expression(material, unreal.MaterialExpressionTextureObjectParameter, -900, -600)
    atlas.set_editor_property("parameter_name", "Atlas")
    atlas.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_GRAYSCALE)
    atlas.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/Black"))
    connect(atlas, "Atlas")
    world_position = mel.create_material_expression(material, unreal.MaterialExpressionWorldPosition, -900, -450)
    connect(world_position, "WorldPos")
    # Fractal gradient noise (ALU: no texture), billowy (turbulence), 0 - 1, over world position.
    noise = mel.create_material_expression(material, unreal.MaterialExpressionNoise, -650, -450)
    noise.set_editor_property("noise_function", unreal.NoiseFunction.NOISEFUNCTION_GRADIENT_ALU)
    noise.set_editor_property("scale", SMOKE_NOISE_SCALE)
    noise.set_editor_property("levels", SMOKE_NOISE_LEVELS)
    noise.set_editor_property("turbulence", True)
    noise.set_editor_property("output_min", 0.0)
    noise.set_editor_property("output_max", 1.0)
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, -350)
    tick.set_editor_property("collection", replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    connect(tick, "Tick")
    # Looked up SMOKE_RISE cm lower every tick: the noise's billows rise through the cloud.
    rise = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -1100, -520)
    rise.set_editor_property("constant", unreal.LinearColor(0.0, 0.0, SMOKE_RISE, 0.0))
    drift = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -800, -520)
    moved = mel.create_material_expression(material, unreal.MaterialExpressionSubtract, -750, -450)
    for source, target, input_name in ((tick, drift, "A"), (rise, drift, "B"), (world_position, moved, "A"),
                                       (drift, moved, "B"), (moved, noise, "World Position")):
        if not mel.connect_material_expressions(source, "", target, input_name):
            raise RuntimeError("smoke material: could not connect %s to %s.%s" % (
                source.get_class().get_name(), target.get_class().get_name(), input_name))
    connect(noise, "Noise")
    origin = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -900, -250)
    origin.set_editor_property("parameter_name", "Origin")
    connect(origin, "Origin")
    for index, param in enumerate(SMOKE_SCALARS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900, -150 + 80 * index)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("default_value", SMOKE_DEFAULTS.get(param, 1.0))
        connect(node, param)
    extinction = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -400, 200)
    extinction.set_editor_property("parameter_name", "Extinction")
    extinction.set_editor_property("default_value", 1.0)
    product = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -150, 100)
    mel.connect_material_expressions(custom, "", product, "A")
    mel.connect_material_expressions(extinction, "", product, "B")
    albedo = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -400, -200)
    albedo.set_editor_property("parameter_name", "Albedo")
    albedo.set_editor_property("default_value", unreal.LinearColor(*SMOKE_ALBEDO, 1.0))
    prop = unreal.MaterialProperty
    if not (mel.connect_material_property(product, "", prop.MP_SUBSURFACE_COLOR)
            and mel.connect_material_property(albedo, "", prop.MP_BASE_COLOR)):
        raise RuntimeError("smoke material: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def asset_name(text):
    """`text` with only the characters an asset name can hold (a demo's file name can have spaces, dots, brackets)."""
    return re.sub(r"[^A-Za-z0-9_-]", "_", text)


def round_folder(parent, replay):
    """The folder under `parent` for one round's own assets: `<end of the demo's name>_r<round>`."""
    return "%s/%s_r%d" % (parent, asset_name(replay["demo"][-12:]), replay["round"])


def smoke_folder(replay):
    return round_folder("/Game/OFAST/Replays/Smokes", replay)


def instance_name(grenade):
    """A smoke's material instance's asset name: after its atlas (which names the demo, round, entity and serial)."""
    return "MI_" + asset_name(os.path.splitext(grenade["smoke"]["volume"]["texture"])[0][-24:])


def import_smoke_texture(replay, replay_path, file_name):
    """One smoke's atlas (8-bit grayscale PNG next to the replay) as a texture: linear, uncompressed, no mipmaps
    (an atlas's mips would blend neighbouring slices and frames)."""
    texture = import_png(replay_path, file_name, smoke_folder(replay))
    texture.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_GRAYSCALE)
    texture.set_editor_property("srgb", False)
    texture.set_editor_property("mip_gen_settings", unreal.TextureMipGenSettings.TMGS_NO_MIPMAPS)
    texture.set_editor_property("never_stream", True)
    assets.save_loaded_asset(texture)
    return texture


def smoke_values(grenade):
    """The smoke material's parameters for one of the replay's smokes: (scalars, Origin, Albedo)."""
    smoke = grenade["smoke"]
    volume = smoke["volume"]
    ticks = volume["frame_ticks"]
    frames = volume["frames"]
    fading = volume.get("fading_frame")
    size_cm = volume["grid_size"] * UNITS_TO_CM
    fade_end = smoke["end_tick"] - SMOKE_FADE_EARLY_TICKS
    scalars = {
        "Size": size_cm,
        "FirstTick": ticks[0],
        # Frames come about 7 ticks apart (7 or 8): evenly spread over the first and last frame's ticks, each is
        # within a tick of its own. One frame alone: any spacing (it is the only one).
        "TicksPerFrame": max((ticks[-1] - ticks[0]) / max(frames - 1, 1), 1.0),
        "Frames": frames,
        "Columns": volume["columns"],
        "Width": volume["width"],
        "Height": volume["height"],
        "FadeTick": min(ticks[fading], fade_end - 1) if fading is not None else fade_end - 1,
        "FadeEnd": fade_end,
        # Per local unit of the volume (a cell of SMOKE_VOLUME_RESOLUTION), see the module docs.
        "Extinction": SMOKE_EXTINCTION * size_cm / SMOKE_VOLUME_RESOLUTION,
    }
    ox, oy, oz = volume["grid_origin"]
    # The grid's corner in Unreal: x and z of CS2's lowest corner; y of CS2's lowest y, which is Unreal's highest.
    origin = unreal.LinearColor(ox * UNITS_TO_CM, -oy * UNITS_TO_CM, oz * UNITS_TO_CM, 0.0)
    tint = [c / 255.0 for c in smoke["color"]]
    albedo = unreal.LinearColor(*[(a + (t - a) * SMOKE_TINT) * SMOKE_BRIGHTNESS * s
                                  for a, t, s in zip(SMOKE_ALBEDO, tint, sun_compensation())], 1.0)
    return scalars, origin, albedo


def sun_compensation():
    """What the smoke's albedo is multiplied by, per channel: the open level's sun colour (linear, its brightest
    channel 1) to the power −SUN_COLOUR_SHARE. (1, 1, 1) without a sun, or for one drawn by its temperature."""
    for actor in unreal.get_editor_subsystem(unreal.EditorActorSubsystem).get_all_level_actors():
        if not isinstance(actor, unreal.DirectionalLight):
            continue
        light = actor.get_editor_property("light_component")
        if light.get_editor_property("use_temperature"):
            break
        # The light's colour is stored sRGB-encoded (set_light_color encodes it).
        color = light.get_editor_property("light_color")
        linear = [((v / 255.0 + 0.055) / 1.055) ** 2.4 if v > 10 else v / 255.0 / 12.92
                  for v in (color.r, color.g, color.b)]
        top = max(max(linear), 1e-3)
        return [max(v / top, 0.05) ** -SUN_COLOUR_SHARE for v in linear]
    return [1.0, 1.0, 1.0]


def smoke_instance(replay, replay_path, grenade, material):
    """The material instance of one smoke (made fresh): its atlas and smoke_values."""
    folder = smoke_folder(replay)
    volume = grenade["smoke"]["volume"]
    texture = import_smoke_texture(replay, replay_path, volume["texture"])
    name = instance_name(grenade)
    if assets.does_asset_exist(folder + "/" + name):
        assets.delete_asset(folder + "/" + name)
    instance = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew())
    mel.set_material_instance_parent(instance, material)
    update_instance(instance, grenade)
    mel.set_material_instance_texture_parameter_value(instance, "Atlas", texture)
    assets.save_loaded_asset(instance)
    return instance


def update_instance(instance, grenade):
    """Sets smoke_values on a smoke's material instance (constant or dynamic)."""
    scalars, origin, albedo = smoke_values(grenade)
    dynamic = isinstance(instance, unreal.MaterialInstanceDynamic)
    for param, value in scalars.items():
        if dynamic:
            instance.set_scalar_parameter_value(param, float(value))
        else:
            mel.set_material_instance_scalar_parameter_value(instance, param, float(value))
    for param, value in (("Origin", origin), ("Albedo", albedo)):
        if dynamic:
            instance.set_vector_parameter_value(param, value)
        else:
            mel.set_material_instance_vector_parameter_value(instance, param, value)


def smokes_of(replay):
    """The replay's smoke grenades whose cloud the export simulated (a smoke `volume`)."""
    return [g for g in replay.get("grenades", []) if (g.get("smoke") or {}).get("volume")]


def smoke_label(grenade):
    return "smoke #%d at %d" % (grenade["entity"], grenade["smoke"]["start_tick"])


def place_smokes(sequence, replay, replay_path, end_frame, spawn):
    """One Heterogeneous Volume per smoke of the replay (smokes_of): a 640-unit cube on the cloud's grid, drawn by
    the smoke's instance of SMOKE_MATERIAL (which reads REPLAY_COLLECTION's tick: key_replay_tick, keyed once per
    sequence by the caller). `spawn(class, label, location)` places a tagged actor. Returns the number of smokes
    placed."""
    smokes = smokes_of(replay)
    if not smokes:
        return 0
    material = smoke_material()
    for grenade in smokes:
        volume = grenade["smoke"]["volume"]
        instance = smoke_instance(replay, replay_path, grenade, material)
        # The volume's box is [0, resolution] in its own space: placed at the grid's corner with CS2's highest y
        # (Unreal's lowest), scaled to the grid's size.
        ox, oy, oz = volume["grid_origin"]
        size_cm = volume["grid_size"] * UNITS_TO_CM
        corner = (ox * UNITS_TO_CM, -(oy + volume["grid_size"]) * UNITS_TO_CM, oz * UNITS_TO_CM)
        actor = spawn(unreal.HeterogeneousVolume, smoke_label(grenade), corner)
        actor.set_actor_scale3d(unreal.Vector(*[size_cm / SMOKE_VOLUME_RESOLUTION] * 3))
        component = actor.get_editor_property("heterogeneous_volume_component")
        configure_component(component)
        component.set_material(0, instance)
    log("smokes: %d volumes, albedo x (%s) for the sun's colour" % (
        len(smokes), ", ".join("%.2f" % s for s in sun_compensation())))
    return len(smokes)
