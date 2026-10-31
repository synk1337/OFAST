"""Smoke grenades' clouds in Unreal: one Heterogeneous Volume per smoke, drawn from the atlas `ofast export-demo` wrote
next to the replay (crates/ofast-core/src/export/smoke.rs; docs/formats/replay.md, "Smoke volumes";
docs/decisions/017-grenades-and-effects.md). Used by import_demo.py, and by probe_smoke.py to try the look.

- SMOKE_MATERIAL: one Volume-domain material for every smoke. A Custom node (SMOKE_HLSL) samples the smoke's atlas at
  the world position and the demo tick; extinction = that density shaped (SMOKE_THRESHOLD, SMOKE_RAMP) x `Extinction`;
  albedo = `Albedo`. Rebuilt in place on every run, so the smokes' instances keep their parent.
- REPLAY_COLLECTION: a material parameter collection whose `ReplayTick` the sequence keys with the demo tick
  (key_replay_tick: two linear keys); every smoke reads it, so the volumes need no keys of their own.
- Per smoke: its atlas imported as a linear grayscale texture without mipmaps, a material instance with the atlas, the
  grid's corner and size, its frames' first tick and spacing, its fading and expiry, its tint; and a Heterogeneous
  Volume actor covering the grid (place_smokes).

Units: Heterogeneous Volumes ray-march in the volume's own space, so `Extinction` is per local unit, here a cell of
the SMOKE_VOLUME_RESOLUTION grid (640 units / 64 = 25.4 cm), not per cm: place_smokes converts SMOKE_EXTINCTION.
"""

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
# CS2 draws smoke above 5 % of a full voxel (5stack's notes on its smoke simulation, facts only); OFAST's cloud is
# opaque SMOKE_RAMP above that (CS2's cloud shows a clear surface, not a slow fade into the air: at 0.15 its edge was a
# soft blur beside CS2's crisp puffs).
SMOKE_THRESHOLD = 0.05
SMOKE_RAMP = 0.1
# The smoke's albedo: a cool light grey SMOKE_TINT of the way to its team colour (m_vSmokeColor): CS2's T smokes read
# tan, its CT smokes grey-blue (de_nuke, 2026-10-23), but mostly from the light they are in. Half way, the T smoke at
# de_dust2's mid doors (round 17, 2026-10-31) was orange-tan, and a 15 % share still tan in the map's warm bounce
# light, where CS2's is grey with a beige cast in the sun.
SMOKE_ALBEDO = (0.8, 0.82, 0.86)
SMOKE_TINT = 0.05
# CS2's cloud is gone a little before smokegrenade_expired (0.4 s before it, nothing showed): the fade ends this many
# ticks early.
SMOKE_FADE_EARLY_TICKS = 48
# The cloud's surface is broken up by a fractal noise in world space that moves the threshold up and down by
# SMOKE_NOISE (CS2's smokes show puffs a foot or two across, not the 20-unit voxels' smooth blobs); features about
# 1 / SMOKE_NOISE_SCALE cm across. At 0.12 and 60 cm the mid-doors smoke showed fine wisps where CS2's shows round
# billows 20 - 30 units across.
SMOKE_NOISE = 0.25
SMOKE_NOISE_SCALE = 1.0 / 75.0
# The noise rises through the cloud, SMOKE_RISE cm a tick (20 units a second): CS2's billows roll upward, where a
# noise fixed in the world left Unreal's smokes standing still (the user's report, 2026-10-31).
SMOKE_RISE = 20.0 * UNITS_TO_CM / 64.0
# How far the noise moves where the cloud is looked up (cm), and how much its threshold rises as it fades out
# (SMOKE_HLSL).
SMOKE_WARP_CM = 15.0 * UNITS_TO_CM
SMOKE_FADE_ERODE = 0.2
# Console variables the smoke volumes need: indirect light from Lumen's translucency volume (single scattering), so a
# smoke in the shade is lit by the room around it, not black. The project sets them for every session
# (unreal/Sandbox/Config/DefaultEngine.ini, [SystemSettings]: the editor, Open in Unreal, shoot_sequence.py; before
# 2026-10-30 they were missing there and smokes in the shade showed black); import_demo.py and probe_smoke.py set them
# again for their own screenshots, and render_round.py's Movie Render Queue presets carry them.
# That light is occluded inside the cloud (the volume's ambient occlusion): without it, it filled the smoke evenly, a
# flat tan blob, where CS2's mid-doors smoke of round 17 is shaded in every crease and on its side in the wall's
# shadow (2026-10-31; the sun alone left that side black).
SMOKE_CVARS = [
    "r.HeterogeneousVolumes.IndirectLighting 1",
    "r.HeterogeneousVolumes.IndirectLighting.Mode 2",
    "r.HeterogeneousVolumes.AmbientOcclusion 1",
]


def apply_cvars():
    """Sets SMOKE_CVARS in the editor."""
    for command in SMOKE_CVARS:
        unreal.SystemLibrary.execute_console_command(None, command)

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


def replay_collection():
    """REPLAY_COLLECTION, made once: the material parameter collection with `ReplayTick`, the demo tick the sequence
    shows (keyed by key_replay_tick), which the smoke volumes read to pick their frame."""
    if assets.does_asset_exist(REPLAY_COLLECTION):
        return assets.load_asset(REPLAY_COLLECTION)
    folder, name = REPLAY_COLLECTION.rsplit("/", 1)
    collection = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.MaterialParameterCollection, unreal.MaterialParameterCollectionFactoryNew())
    parameter = unreal.CollectionScalarParameter()
    parameter.set_editor_property("parameter_name", "ReplayTick")
    parameter.set_editor_property("default_value", 0.0)
    collection.set_editor_property("scalar_parameters", [parameter])
    assets.save_loaded_asset(collection)
    return collection


def key_replay_tick(sequence, replay, end_frame):
    """Keys REPLAY_COLLECTION's `ReplayTick` with the demo tick: two linear keys (the sequence runs one frame per
    tick), so sub-frame times get fractional ticks."""
    tick0 = replay["start_tick"]
    track = sequence.add_track(unreal.MovieSceneMaterialParameterCollectionTrack)
    track.set_editor_property("mpc", replay_collection())
    section = track.add_section()
    section.set_range(0, end_frame)
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
# shaped: nothing below the threshold, opaque Ramp above it; as the cloud fades (from the frame with the fading flag
# to FadeEnd) it turns translucent, its threshold rising by FadeErode only (risen to 1, the cloud was eaten into holes
# as it went: the user's report, 2026-10-31). Where it is looked up is moved by the noise, Warp cm at most along a
# slant: CS2's server marks voxels solid (a clip above a low wall too), and a cloud cut by them showed a flat plane
# where CS2's edge is lumpy.
SMOKE_HLSL = r"""
if (Tick < FirstTick || Tick > FadeEnd) return 0.0;
// Grid coordinates, 0 - 1, in CS2's axes: Unreal's y is CS2's -y.
float3 g = float3(WorldPos.x - Origin.x, Origin.y - WorldPos.y, WorldPos.z - Origin.z) / Size;
g += (Noise - 0.5) * (Warp / Size) * float3(0.8, 0.5, 0.33);
if (any(g < 0.0) || any(g > 1.0)) return 0.0;
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
d *= d;
float fade = saturate((FadeEnd - Tick) / max(FadeEnd - FadeTick, 1.0));
// The noise (0 - 1) moves the threshold: its round crests bulge out of the cloud's surface as puffs, its thin creases
// cut into it (the other way round, the crests carved round pits: a sponge, where CS2's cloud is cauliflower).
float t = max(Threshold + (1.0 - fade) * FadeErode - NoiseAmount * (Noise - 0.5), 0.005);
return fade * saturate((d - t) / max(Ramp, 0.001));
"""
# The Custom node's scalar inputs, each a material parameter of the same name.
SMOKE_SCALARS = ["Size", "FirstTick", "TicksPerFrame", "Frames", "Columns", "Width", "Height", "FadeTick", "FadeEnd",
                 "Threshold", "Ramp", "NoiseAmount", "Warp", "FadeErode"]
SMOKE_DEFAULTS = {"Threshold": SMOKE_THRESHOLD, "Ramp": SMOKE_RAMP, "NoiseAmount": SMOKE_NOISE,
                  "Warp": SMOKE_WARP_CM, "FadeErode": SMOKE_FADE_ERODE}


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
    noise.set_editor_property("levels", 4)
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
    albedo = unreal.LinearColor(*[a + (t - a) * SMOKE_TINT for a, t in zip(SMOKE_ALBEDO, tint)], 1.0)
    return scalars, origin, albedo


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
        component.set_editor_property("volume_resolution", unreal.IntVector(*[SMOKE_VOLUME_RESOLUTION] * 3))
        component.set_material(0, instance)
    log("smokes: %d volumes" % len(smokes))
    return len(smokes)
