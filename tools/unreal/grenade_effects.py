"""The grenades' effects in Unreal other than smokes (smoke_volumes.py): molotov and incendiary fires, HE blasts,
flashbang bursts and the flashed white of a blinded player's view (docs/decisions/017-grenades-and-effects.md;
docs/formats/replay.md, "In Unreal"). Used by import_demo.py and probe_smoke.py.

CS2 draws these with particle systems (particles/inferno_fx/molotov_groundfire.vpcf, explosions_fx/explosion_hegrenade,
explosion_flashbang). Niagara systems cannot be written from editor Python, so OFAST rebuilds their main layers as
"particle systems in a material", with CS2's own sprite sheets (the export's flipbooks, export::effects):

- Each effect (a fire, a blast, a burst) is one actor with one instanced static mesh per layer (FIRE_LAYERS,
  BLAST_LAYERS, FLASH_LAYERS). An instance is a sprite slot; its per-instance data hold where it is (Unreal cm), the
  ticks it may spawn particles in, a phase and a seed.
- The materials (SPRITE_MATERIALS: additive for flames and flares, translucent for smoke; unlit) read the demo tick
  from the parameter collection the smokes use (smoke_volumes.REPLAY_COLLECTION) and, in the vertex and the pixel
  shader alike, the same few lines (SPRITE_STATE): the slot respawns a particle every LifeMax seconds while its span
  lasts; a hash of the slot's seed and the respawn count draws the particle's lifetime, size, place, roll and sheet
  sequence, as CS2's initializers draw them from their ranges. A fire's slot spans its flame's burning ticks; a
  blast's or burst's spans one tick, so it spawns once. The vertex shader turns the quad to face the camera at the
  particle's place and size (from ScaleStart to ScaleEnd of its radius, rising Rise units over its life); the pixel
  shader plays the sequence's frames over the particle's life (CS2's ANIMATION_TYPE_MANUAL_FRAMES driven by the
  normalised age), tinted, fading in and out, and softened where it meets the world.
- Every part of it is a function of the demo tick, so scrubbing the sequence shows the same flames every time.
- A point light per effect, keyed: a fire's flickers with its flames, a blast's and a flashbang's pulse and die out.
- A blinded player's camera gets a post-process material (FLASHED_MATERIAL) whitening its view from the player's
  blindings (the replay's `blindings`) and the demo tick.
"""

import math
import random

import unreal

import smoke_volumes

SPRITE_MATERIALS = {False: "/Game/OFAST/Replays/M_SpriteAdd", True: "/Game/OFAST/Replays/M_SpriteBlend"}
FLASHED_MATERIAL = "/Game/OFAST/Replays/M_Flashed"
# The sprites' quad: a copy of the engine's 1 m plane whose bounds reach SPRITE_BOUNDS_UP (local cm) above it and
# SPRITE_BOUNDS_DOWN below. The vertex shader moves the quad's corners to where the sprite is drawn, but the engine
# culls an instance by its mesh's bounds: the flat plane's have no height, so flames standing above a floor that a
# low wall hides would vanish with it. Instances are scaled 6 x: bounds 3 m to each side, 6 m up, 1.5 m down.
ENGINE_PLANE = "/Engine/BasicShapes/Plane"
SPRITE_MESH = "/Game/OFAST/Replays/Effects/SM_Sprite"
SPRITE_INSTANCE_SCALE = 6.0
SPRITE_BOUNDS_UP = 100.0
SPRITE_BOUNDS_DOWN = 25.0
FOLDER = "/Game/OFAST/Replays/Effects"
UNITS_TO_CM = 2.54
# The layers: the effect texture of that role in the agents manifest, sprite slots (per flame for a fire, per blast or
# burst otherwise), CS2's ranges (its particle systems, read 2026-10-24): lifetime (s), radius (units), size from birth
# to death (x radius), alpha, roll (+- degrees), how far from the centre a particle is born (units), how far it rises
# over its life (units); and for Unreal: a brightness factor (CS2's overbright, tuned against CS2's picture), a tint
# (CS2's grey fireball and smoke sheets are coloured by its shaders), translucent (alpha-blended) or additive.
FIRE_LAYERS = [
    # particles/inferno_fx/molotov_child_flame01a: the main flames.
    {"role": "fire", "slots": 8, "life": (0.65, 1.2), "radius": (28.0, 45.0), "scale": (1.5, 0.75),
     "alpha": (0.47, 0.59), "roll": 24.0, "spread": 40.0, "rise": 15.0, "bright": 3.0},
    # particles/inferno_fx/molotov_child_flame03a: tongues of burning gas among them.
    {"role": "gas", "slots": 3, "life": (0.75, 1.35), "radius": (24.0, 55.0), "scale": (1.0, 0.9),
     "alpha": (0.78, 0.98), "roll": 45.0, "spread": 40.0, "rise": 25.0, "bright": 1.5},
]
BLAST_LAYERS = [
    # particles/explosions_fx/explosion_hegrenade_smoke_core: the fireball (its flame sheet, grey, tinted orange)...
    {"role": "blast_flame", "slots": 3, "life": (0.35, 0.6), "radius": (90.0, 180.0), "scale": (0.7, 1.2),
     "alpha": (0.9, 1.0), "roll": 180.0, "spread": 15.0, "rise": 20.0, "bright": 4.0, "tint": (1.0, 0.55, 0.22)},
    # ... and its smoke, lasting 1 – 2 s.
    {"role": "blast_smoke", "slots": 4, "life": (1.0, 2.0), "radius": (90.0, 180.0), "scale": (0.6, 1.3),
     "alpha": (0.5, 0.7), "roll": 180.0, "spread": 25.0, "rise": 40.0, "bright": 1.0, "tint": (0.25, 0.24, 0.22),
     "translucent": True},
]
FLASH_LAYERS = [
    # particles/explosions_fx/explosion_flashbang: one bright flare.
    {"role": "flare", "slots": 1, "life": (0.15, 0.25), "radius": (120.0, 160.0), "scale": (1.0, 1.6),
     "alpha": (1.0, 1.0), "roll": 0.0, "spread": 0.0, "rise": 0.0, "bright": 8.0},
]
# Depth fade of the sprites against the world, cm.
SOFT_EDGE_CM = 30.0
# A sprite's half-size is SPRITE_SIZE x the particle's radius: CS2's radii read as half-sizes drew flames twice as
# tall as CS2's (a molotov on de_nuke at the same tick, 2026-10-24).
SPRITE_SIZE = 0.5
# Per-instance data: centre x, y, z (cm), first and last tick particles may be born, phase (0 - 1 of LifeMax: where in
# its first lifetime a slot starts, so a flame's slots do not all respawn together; 0 for a one-shot, born on its
# tick), seed (0 - 100: small, so the shaders' sin() hashes keep their precision).
CUSTOM_DATA = ["CenterX", "CenterY", "CenterZ", "FirstTick", "LastTick", "Phase", "Seed"]
# A fire's light: colour, intensity (candela) with LIGHT_FLAMES flames burning (the most a fire had in round 5 on
# de_nuke), reach (units), keyed every this many ticks.
LIGHT_COLOR = (1.0, 0.55, 0.22)
LIGHT_INTENSITY = 60.0
LIGHT_FLAMES = 16
LIGHT_RADIUS = 450.0
FLICKER_TICKS = 4
# A blast's and a flashbang's light: colour, peak intensity (candela), reach (units), and how it dies out: (ticks after
# the detonation, share of the peak). CS2's blast light lives about a third of a second, its flash less.
# (1500 cd over 600 units washed a whole room out white 2 ticks after a blast 272 units away.)
BLAST_LIGHT = {"color": (1.0, 0.62, 0.3), "peak": 200.0, "radius": 400.0,
               "decay": [(0, 1.0), (4, 0.6), (10, 0.25), (20, 0.05), (28, 0.0)]}
FLASH_LIGHT = {"color": (1.0, 1.0, 1.0), "peak": 600.0, "radius": 600.0,
               "decay": [(0, 1.0), (3, 0.4), (8, 0.08), (14, 0.0)]}
# Blindings per player the flashed material holds (two per vector parameter).
FLASHED_SLOTS = 12
# How white a blinding makes the view: FLASH_WHITE x saturate((seconds left - FLASH_END) / FLASH_FADE), in a warm
# white. Fitted to CS2 in round 5 on de_nuke (2026-10-24): 0.2 s into a 0.75 s blinding the view is not visibly
# white; 6 ticks into a 3.8 s one (FalleN, tick 34359) CS2's walls and floor read (181 – 190, 173 – 182, 167 – 175)
# where 0.8 gave (219 – 223, 211 – 215, 194 – 198), and the blend weight solved from each channel is 0.58 – 0.66;
# 0.6 rendered (182 – 191, 174 – 182, 158 – 167), and the colour's blue raised from 0.9 to 0.95 brings blue up 8.
FLASH_WHITE = 0.6
FLASH_END = 0.5
FLASH_FADE = 2.5
FLASH_COLOR = (1.0, 0.98, 0.95)

assets = unreal.EditorAssetLibrary
mel = unreal.MaterialEditingLibrary
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)


def log(message):
    unreal.log("OFAST: " + message)


# The particle a slot shows at the demo tick: its age, normalised age, whether it is alive, and six hashes (0 - 1) of
# the slot's seed and the respawn count. Shared by the vertex and pixel shaders (a Custom node cannot define
# functions). Half a tick of slack: a one-shot's particle is born exactly on its first and last tick.
SPRITE_STATE = r"""
float t = (Tick - FirstTick) / TicksPerSecond;
float cycle = floor((t + Phase * LifeMax) / LifeMax);
float age = t + Phase * LifeMax - cycle * LifeMax;
float born = Tick - age * TicksPerSecond;
float2 key = float2(Seed, cycle);
float h1 = frac(sin(dot(key, float2(12.9898, 78.233))) * 43758.5453);
float h2 = frac(sin(dot(key, float2(39.3468, 11.135))) * 24634.6345);
float h3 = frac(sin(dot(key, float2(73.156, 52.235))) * 63458.2342);
float h4 = frac(sin(dot(key, float2(27.619, 57.583))) * 31337.1234);
float h5 = frac(sin(dot(key, float2(91.211, 13.917))) * 12345.6789);
float h6 = frac(sin(dot(key, float2(19.773, 83.517))) * 54321.9876);
float life = lerp(LifeMin, LifeMax, h1);
float norm = age / life;
bool alive = Tick >= FirstTick && born >= FirstTick - 0.5 && born <= LastTick + 0.5 && norm <= 1.0;
"""
# Vertex shader: the world position offset putting the quad's corner where the particle's camera-facing sprite has it.
SPRITE_WPO = SPRITE_STATE + r"""
float2 corner = float2(UV.x * 2.0 - 1.0, 1.0 - UV.y * 2.0);
float roll = radians((h5 * 2.0 - 1.0) * Roll);
corner = float2(corner.x * cos(roll) - corner.y * sin(roll), corner.x * sin(roll) + corner.y * cos(roll));
float angle = h3 * 6.2831853;
float radius = alive ? lerp(RadiusMin, RadiusMax, h2) * lerp(ScaleStart, ScaleEnd, norm) * 2.54 : 0.0;
// Lifted by most of its size, so the sprite stands on the ground rather than half in it (the frames' flames sit
// low in their cells), rising as it ages.
float3 centre = float3(CenterX, CenterY, CenterZ)
    + float3(cos(angle), sin(angle), 0.0) * sqrt(h4) * Spread * 2.54
    + float3(0.0, 0.0, 1.0) * (Rise * norm * 2.54 + radius * 0.6);
float3 forward = CameraPos - centre;
forward = length(forward) > 0.001 ? normalize(forward) : float3(1.0, 0.0, 0.0);
float3 right = cross(forward, float3(0.0, 0.0, 1.0));
right = length(right) > 0.001 ? normalize(right) : float3(0.0, 1.0, 0.0);
float3 up = cross(right, forward);
return centre + (right * corner.x + up * corner.y) * radius - WorldPos;
"""
# Pixel shader: the sequence's frame at the particle's age, faded in over its first tenth and out over its last
# third; (colour x tint x brightness, opacity) in a float4.
SPRITE_PIXEL = SPRITE_STATE + r"""
if (!alive) return float4(0.0, 0.0, 0.0, 0.0);
float sequence = min(floor(h6 * Sequences), Sequences - 1.0);
float frame = min(floor(norm * Frames), Frames - 1.0);
float cell = sequence * Stride + frame;
float row = floor((cell + 0.5) / Columns);
float column = cell - row * Columns;
float4 texel = Tex.Sample(TexSampler, (float2(column, row) + UV) / float2(Columns, Rows));
float alpha = lerp(AlphaMin, AlphaMax, h2) * saturate(norm / 0.1) * saturate((1.0 - norm) / 0.33) * texel.a;
return float4(texel.rgb * Tint * Bright, alpha);
"""
# Scalar parameters of the Custom nodes (each a material parameter), besides the per-instance data and the tick.
SPRITE_SCALARS = ["TicksPerSecond", "LifeMin", "LifeMax", "RadiusMin", "RadiusMax", "ScaleStart", "ScaleEnd", "Roll",
                  "Spread", "Rise", "AlphaMin", "AlphaMax", "Bright", "Sequences", "Frames", "Stride", "Columns",
                  "Rows"]


def sprite_material(translucent):
    """SPRITE_MATERIALS[translucent], built on first use and rebuilt in place on every run (the layers' instances keep
    it): unlit, two-sided, for instanced static meshes; additive (emissive = colour x opacity) or translucent
    (emissive = colour, opacity)."""
    material = smoke_volumes.empty_material(SPRITE_MATERIALS[translucent])
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_TRANSLUCENT if translucent
                                 else unreal.BlendMode.BLEND_ADDITIVE)
    material.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    material.set_editor_property("two_sided", True)
    material.set_editor_property("used_with_instanced_static_meshes", True)
    shared = ["Tick"] + CUSTOM_DATA + SPRITE_SCALARS
    output = unreal.CustomMaterialOutputType
    wpo = smoke_volumes.custom_node(material, SPRITE_WPO, output.CMOT_FLOAT3, ["UV", "WorldPos", "CameraPos"] + shared,
                                    -400, 400)
    pixel = smoke_volumes.custom_node(material, SPRITE_PIXEL, output.CMOT_FLOAT4, ["UV", "Tex", "Tint"] + shared,
                                      -400, -200)

    def connect(node, input_name, *targets):
        for target in targets:
            if not mel.connect_material_expressions(node, "", target, input_name):
                raise RuntimeError("sprite material: could not connect " + input_name)

    connect(mel.create_material_expression(material, unreal.MaterialExpressionTextureCoordinate, -1200, -600), "UV",
            wpo, pixel)
    connect(mel.create_material_expression(material, unreal.MaterialExpressionWorldPosition, -1200, 500), "WorldPos",
            wpo)
    connect(mel.create_material_expression(material, unreal.MaterialExpressionCameraPositionWS, -1200, 600),
            "CameraPos", wpo)
    texture = mel.create_material_expression(material, unreal.MaterialExpressionTextureObjectParameter, -1200, -500)
    texture.set_editor_property("parameter_name", "Tex")
    texture.set_editor_property("texture", assets.load_asset("/Engine/EngineResources/Black"))
    connect(texture, "Tex", pixel)
    tint = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -1200, -700)
    tint.set_editor_property("parameter_name", "Tint")
    tint.set_editor_property("default_value", unreal.LinearColor(1.0, 1.0, 1.0, 1.0))
    connect(tint, "Tint", pixel)
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -1200, -400)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    connect(tick, "Tick", wpo, pixel)
    for index, name in enumerate(CUSTOM_DATA):
        node = mel.create_material_expression(material, unreal.MaterialExpressionPerInstanceCustomData, -1200,
                                              -300 + 60 * index)
        node.set_editor_property("data_index", index)
        connect(node, name, wpo, pixel)
    for index, name in enumerate(SPRITE_SCALARS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900,
                                              -300 + 60 * index)
        node.set_editor_property("parameter_name", name)
        node.set_editor_property("default_value", 1.0)
        connect(node, name, wpo, pixel)
    # Soft where the sprite meets the ground or a wall (CS2's particles fade by depth the same way).
    depth_fade = mel.create_material_expression(material, unreal.MaterialExpressionDepthFade, -150, 100)
    depth_fade.set_editor_property("fade_distance_default", SOFT_EDGE_CM)
    rgb = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -150, -300)
    rgb.set_editor_property("r", True)
    rgb.set_editor_property("g", True)
    rgb.set_editor_property("b", True)
    rgb.set_editor_property("a", False)
    opacity = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -150, -150)
    for channel in ("r", "g", "b"):
        opacity.set_editor_property(channel, False)
    opacity.set_editor_property("a", True)
    mel.connect_material_expressions(pixel, "", rgb, "")
    mel.connect_material_expressions(pixel, "", opacity, "")
    soft = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, 0, -150)
    mel.connect_material_expressions(opacity, "", soft, "A")
    mel.connect_material_expressions(depth_fade, "", soft, "B")
    prop = unreal.MaterialProperty
    if translucent:
        ok = (mel.connect_material_property(rgb, "", prop.MP_EMISSIVE_COLOR)
              and mel.connect_material_property(soft, "", prop.MP_OPACITY))
    else:
        emissive = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, 150, -250)
        mel.connect_material_expressions(rgb, "", emissive, "A")
        mel.connect_material_expressions(soft, "", emissive, "B")
        ok = mel.connect_material_property(emissive, "", prop.MP_EMISSIVE_COLOR)
    if not (ok and mel.connect_material_property(wpo, "", prop.MP_WORLD_POSITION_OFFSET)):
        raise RuntimeError("sprite material: could not connect its outputs")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def sprite_mesh():
    """SPRITE_MESH: the engine plane copied once, its bounds extended (see SPRITE_BOUNDS_UP)."""
    if assets.does_asset_exist(SPRITE_MESH):
        mesh = assets.load_asset(SPRITE_MESH)
    else:
        mesh = assets.duplicate_asset(ENGINE_PLANE, SPRITE_MESH)
    mesh.set_editor_property("positive_bounds_extension", unreal.Vector(0.0, 0.0, SPRITE_BOUNDS_UP))
    mesh.set_editor_property("negative_bounds_extension", unreal.Vector(0.0, 0.0, SPRITE_BOUNDS_DOWN))
    assets.save_loaded_asset(mesh)
    bounds = mesh.get_bounds()
    log("sprite mesh bounds: extent %s" % bounds.box_extent)
    return mesh


def layer_instance(layer, effect, texture, material, tick_rate):
    """The material instance of one layer (shared by every round: CS2's sheets are the same), updated in place when
    it exists (the level's effects keep it): its texture, flipbook layout, CS2's ranges, tint and brightness."""
    instance = smoke_volumes.material_instance(FOLDER, "MI_Sprite_" + layer["role"], material)
    sequences = effect["sequences"]
    values = {
        "TicksPerSecond": tick_rate,
        "LifeMin": layer["life"][0], "LifeMax": layer["life"][1],
        "RadiusMin": layer["radius"][0] * SPRITE_SIZE, "RadiusMax": layer["radius"][1] * SPRITE_SIZE,
        "ScaleStart": layer["scale"][0], "ScaleEnd": layer["scale"][1],
        "Roll": layer["roll"], "Spread": layer["spread"], "Rise": layer["rise"],
        "AlphaMin": layer["alpha"][0], "AlphaMax": layer["alpha"][1], "Bright": layer["bright"],
        # Every sequence plays as many frames as the shortest has (they differ by one or two).
        "Sequences": len(sequences), "Frames": min(s["frames"] for s in sequences),
        "Stride": effect["stride"], "Columns": effect["columns"], "Rows": effect["rows"],
    }
    for param, value in values.items():
        mel.set_material_instance_scalar_parameter_value(instance, param, float(value))
    r, g, b = layer.get("tint", (1.0, 1.0, 1.0))
    mel.set_material_instance_vector_parameter_value(instance, "Tint", unreal.LinearColor(r=r, g=g, b=b, a=1.0))
    mel.set_material_instance_texture_parameter_value(instance, "Tex", texture)
    assets.save_loaded_asset(instance)
    return instance


def update_layers(replay, replay_path, effects):
    """Rebuilds the sprite materials and updates every layer's instance from the layer tables and the manifest's
    `effects` (the level's effects keep them: probe_smoke.py's quick look at a change). Returns role -> instance for
    the layers whose texture the export wrote."""
    by_role = {e["role"]: e for e in effects}
    materials = {translucent: sprite_material(translucent) for translucent in (False, True)}
    tick_rate = round(1.0 / replay["tick_interval"])
    out = {}
    for layer in FIRE_LAYERS + BLAST_LAYERS + FLASH_LAYERS:
        effect = by_role.get(layer["role"])
        if effect is None:
            continue
        if "stride" not in effect:
            log("effect %s: the agents manifest predates regular flipbooks (no stride): export the round again"
                % layer["role"])
            continue
        texture = smoke_volumes.import_png(replay_path, effect["file"], FOLDER)
        assets.save_loaded_asset(texture)
        out[layer["role"]] = layer_instance(layer, effect, texture, materials[layer.get("translucent", False)],
                                            tick_rate)
    return out


def add_component(actor, cls, name):
    """Adds a component of class `cls` to a level actor (as an editor would), returns it."""
    subsystem = unreal.get_engine_subsystem(unreal.SubobjectDataSubsystem)
    root = subsystem.k2_gather_subobject_data_for_instance(actor)[0]
    params = unreal.AddNewSubobjectParams(parent_handle=root, new_class=cls, blueprint_context=None)
    handle, failure = subsystem.add_new_subobject(params)
    if not failure.is_empty():
        raise RuntimeError("could not add %s to %s: %s" % (cls.__name__, actor.get_actor_label(), failure))
    subsystem.rename_subobject(handle, unreal.Text(name))
    data = unreal.SubobjectDataBlueprintFunctionLibrary.get_data(handle)
    return unreal.SubobjectDataBlueprintFunctionLibrary.get_object(data)


def add_sprites(actor, layer, instance, mesh, slots, rng):
    """One instanced mesh of `layer` on `actor`: an instance per (centre in Source units, first tick, last tick) of
    `slots`, times the layer's slots."""
    ism = add_component(actor, unreal.InstancedStaticMeshComponent, "sprites_" + layer["role"])
    ism.set_static_mesh(mesh)
    ism.set_material(0, instance)
    ism.set_editor_property("cast_shadow", False)
    ism.set_num_custom_data_floats(len(CUSTOM_DATA))
    scale = unreal.Vector(*[SPRITE_INSTANCE_SCALE] * 3)
    for (sx, sy, sz), first, last in slots:
        position = unreal.Vector(sx * UNITS_TO_CM, -sy * UNITS_TO_CM, sz * UNITS_TO_CM)
        for _ in range(layer["slots"]):
            # Scaled up so the instance's bounds hold the sprite the shader draws around it.
            index = ism.add_instance(unreal.Transform(position, unreal.Rotator(0.0, 0.0, 0.0), scale), True)
            # A one-shot's particle is born on its tick: a phase would put its birth up to a lifetime before or
            # after it, outside the half tick the shader allows, and it would never show.
            phase = rng.random() if last > first else 0.0
            values = [position.x, position.y, position.z, first, last, phase, rng.uniform(0.0, 100.0)]
            for k, value in enumerate(values):
                ism.set_custom_data_value(index, k, float(value), k == len(values) - 1)


def spawn_light(spawn, label, position, color, radius):
    light = spawn(unreal.PointLight, label, (position[0] * UNITS_TO_CM, -position[1] * UNITS_TO_CM,
                                             position[2] * UNITS_TO_CM))
    component = light.get_editor_property("light_component")
    # Named: unreal.Color's positional order is FColor's memory order (b, g, r, a), which made a light blue.
    r, g, b = color
    component.set_light_color(unreal.LinearColor(r=r, g=g, b=b, a=1.0))
    component.set_editor_property("attenuation_radius", radius * UNITS_TO_CM)
    component.set_editor_property("intensity", 0.0)
    component.set_editor_property("cast_shadows", True)
    return light


def intensity_channel(sequence, light, end_frame):
    """The float channel keying the light component's intensity."""
    sequence.add_possessable(light)
    binding = sequence.add_possessable(light.get_editor_property("light_component"))
    track = binding.add_track(unreal.MovieSceneFloatTrack)
    track.set_property_name_and_path("Intensity", "Intensity")
    section = track.add_section()
    section.set_range(0, end_frame)
    return section.get_all_channels()[0]


def add_key(channel, frame, value):
    channel.add_key(unreal.FrameNumber(frame), value, 0.0, unreal.MovieSceneTimeUnit.DISPLAY_RATE,
                    unreal.MovieSceneKeyInterpolation.LINEAR)


def burning_flames(fire, tick):
    return sum(1 for f in fire["flames"] if f["first_tick"] <= tick <= f["last_tick"])


def light_intensity(fire, tick, tick_rate, rng=None):
    """A fire's light at a tick: LIGHT_INTENSITY x its burning flames' share of LIGHT_FLAMES (or of all its flames, a
    bigger fire's: one light at its middle, which more flames must not overexpose) x a flicker (two sines, and a
    little noise from `rng` when given)."""
    seconds = (tick - fire["start_tick"]) / tick_rate
    flicker = 0.75 + 0.15 * math.sin(seconds * 23.0) + 0.1 * math.sin(seconds * 37.0 + 1.3)
    if rng is not None:
        flicker += 0.1 * (rng.random() - 0.5)
    return LIGHT_INTENSITY * burning_flames(fire, tick) / max(LIGHT_FLAMES, len(fire["flames"])) * flicker


def light_label(fire):
    return "fire light #%d at %d" % (fire["entity"], fire["start_tick"])


def pulse_intensity(pulse, ticks_after):
    """A blast's or flashbang's light `ticks_after` its detonation (the decay keys, interpolated)."""
    points = pulse["decay"]
    if ticks_after < 0 or ticks_after > points[-1][0]:
        return 0.0
    for (t0, v0), (t1, v1) in zip(points, points[1:]):
        if t0 <= ticks_after <= t1:
            return pulse["peak"] * (v0 + (v1 - v0) * (ticks_after - t0) / max(t1 - t0, 1))
    return pulse["peak"] * points[0][1]


def tick_rate_of(replay):
    return round(1.0 / replay["tick_interval"])


# What goes off with an HE grenade and a flashbang: the sprite layers, the light pulse, the actor's name.
BURSTS = {"he": (BLAST_LAYERS, BLAST_LIGHT, "blast"), "flashbang": (FLASH_LAYERS, FLASH_LIGHT, "flash")}


def bursts_of(replay):
    """The replay's HE grenades and flashbangs that went off (a `detonation`)."""
    return [g for g in replay.get("grenades", []) if g["kind"] in BURSTS and g.get("detonation")]


def burst_label(grenade):
    return "%s #%d at %d" % (BURSTS[grenade["kind"]][2], grenade["entity"], grenade["detonation"]["tick"])


def burst_light_label(grenade):
    return burst_label(grenade) + " light"


def place_fires(sequence, replay, end_frame, instances, mesh, spawn):
    """One actor per fire of the replay (`fires`), with a sprite slot per flame and layer of FIRE_LAYERS (quads:
    `mesh`), and its flickering light. Returns the number of fires placed."""
    fires = [f for f in replay.get("fires", []) if f["flames"]]
    layers = [layer for layer in FIRE_LAYERS if layer["role"] in instances]
    if not fires:
        return 0
    tick0 = replay["start_tick"]
    tick_rate = tick_rate_of(replay)
    for fire in fires:
        flames = fire["flames"]
        x, y, z = [sum(f["position"][k] for f in flames) / len(flames) for k in range(3)]
        actor = spawn(unreal.Actor, "fire #%d at %d" % (fire["entity"], fire["start_tick"]),
                      (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
        rng = random.Random(fire["entity"] * 104729 + fire["start_tick"])
        slots = [(f["position"], f["first_tick"], f["last_tick"]) for f in flames]
        for layer in layers:
            add_sprites(actor, layer, instances[layer["role"]], mesh, slots, rng)
        light = spawn_light(spawn, light_label(fire), (x, y, z + 30.0), LIGHT_COLOR, LIGHT_RADIUS)
        channel = intensity_channel(sequence, light, end_frame)
        flicker = random.Random(fire["entity"] * 7919 + fire["start_tick"])
        start, end = fire["start_tick"], fire["end_tick"]
        add_key(channel, max(start - tick0 - 1, 0), 0.0)
        for tick in range(start, end + 1, FLICKER_TICKS):
            add_key(channel, tick - tick0, light_intensity(fire, tick, tick_rate, flicker))
        add_key(channel, min(end - tick0 + FLICKER_TICKS, end_frame), 0.0)
    log("fires: %d, %d sprite layers" % (len(fires), len(layers)))
    return len(fires)


def place_bursts(sequence, replay, end_frame, instances, mesh, spawn):
    """One actor per HE grenade's and flashbang's detonation (bursts_of): its one-shot sprites (BLAST_LAYERS,
    FLASH_LAYERS on quads `mesh`: every slot spawns once, on the detonation's tick) and its light pulse (BLAST_LIGHT,
    FLASH_LIGHT). Returns the number placed."""
    tick0 = replay["start_tick"]
    placed = 0
    for grenade in bursts_of(replay):
        layers, pulse, _ = BURSTS[grenade["kind"]]
        tick = grenade["detonation"]["tick"]
        x, y, z = grenade["detonation"]["position"]
        label = burst_label(grenade)
        actor = spawn(unreal.Actor, label, (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM))
        rng = random.Random(grenade["entity"] * 15485863 + tick)
        for layer in layers:
            if layer["role"] in instances:
                add_sprites(actor, layer, instances[layer["role"]], mesh, [((x, y, z), tick, tick)], rng)
        light = spawn_light(spawn, burst_light_label(grenade), (x, y, z + 20.0), pulse["color"], pulse["radius"])
        channel = intensity_channel(sequence, light, end_frame)
        if tick - tick0 > 0:
            add_key(channel, tick - tick0 - 1, 0.0)
        for after, share in pulse["decay"]:
            if tick - tick0 + after <= end_frame:
                add_key(channel, tick - tick0 + after, pulse["peak"] * share)
        placed += 1
    if placed:
        log("blasts and flashes: %d" % placed)
    return placed


# The flashed white over a camera's view: for each blinding (start tick, duration in seconds; two per Flash vector,
# FLASHED_VECTORS of them), FlashWhite x saturate((seconds left - FlashEnd) / FlashFade); the whitest blinding wins.
FLASHED_VECTORS = FLASHED_SLOTS // 2
FLASHED_HLSL = r"""
float4 flashes[%(n)d] = { %(names)s };
float white = 0.0;
for (int i = 0; i < %(n)d; i++)
{""" % {"n": FLASHED_VECTORS, "names": ", ".join("Flash%d" % k for k in range(FLASHED_VECTORS))} + r"""
    for (int k = 0; k < 2; k++)
    {
        float start = k == 0 ? flashes[i].x : flashes[i].z;
        float duration = k == 0 ? flashes[i].y : flashes[i].w;
        float remaining = duration - (Tick - start) / TicksPerSecond;
        if (duration > 0.0 && Tick >= start && remaining > 0.0)
            white = max(white, FlashWhite * saturate((remaining - FlashEnd) / FlashFade));
    }
}
return lerp(Scene, FlashColor, white);
"""


def flashed_material():
    """FLASHED_MATERIAL, rebuilt in place: a post-process material after tonemapping whitening the view as FLASHED_HLSL
    says, from its instance's Flash vectors and the demo tick."""
    material = smoke_volumes.empty_material(FLASHED_MATERIAL)
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_POST_PROCESS)
    material.set_editor_property("blendable_location", unreal.BlendableLocation.BL_SCENE_COLOR_AFTER_TONEMAPPING)
    names = (["Scene", "Tick", "TicksPerSecond", "FlashWhite", "FlashEnd", "FlashFade", "FlashColor"]
             + ["Flash%d" % k for k in range(FLASHED_VECTORS)])
    custom = smoke_volumes.custom_node(material, FLASHED_HLSL, unreal.CustomMaterialOutputType.CMOT_FLOAT3, names,
                                       -400, 0)
    scene = mel.create_material_expression(material, unreal.MaterialExpressionSceneTexture, -900, -300)
    scene.set_editor_property("scene_texture_id", unreal.SceneTextureId.PPI_POST_PROCESS_INPUT0)
    rgb = mel.create_material_expression(material, unreal.MaterialExpressionComponentMask, -700, -300)
    for channel in ("r", "g", "b"):
        rgb.set_editor_property(channel, True)
    rgb.set_editor_property("a", False)
    mel.connect_material_expressions(scene, "Color", rgb, "")
    mel.connect_material_expressions(rgb, "", custom, "Scene")
    tick = mel.create_material_expression(material, unreal.MaterialExpressionCollectionParameter, -900, -200)
    tick.set_editor_property("collection", smoke_volumes.replay_collection())
    tick.set_editor_property("parameter_name", "ReplayTick")
    mel.connect_material_expressions(tick, "", custom, "Tick")
    scalars = (("TicksPerSecond", 64.0), ("FlashWhite", FLASH_WHITE), ("FlashEnd", FLASH_END),
               ("FlashFade", FLASH_FADE))
    for index, (param, default) in enumerate(scalars):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900, -100 + 60 * index)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("default_value", default)
        mel.connect_material_expressions(node, "", custom, param)
    color = mel.create_material_expression(material, unreal.MaterialExpressionConstant3Vector, -900, 150)
    color.set_editor_property("constant", unreal.LinearColor(*FLASH_COLOR, 1.0))
    mel.connect_material_expressions(color, "", custom, "FlashColor")
    for k in range(FLASHED_VECTORS):
        node = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -900, 250 + 60 * k)
        node.set_editor_property("parameter_name", "Flash%d" % k)
        node.set_editor_property("default_value", unreal.LinearColor(0.0, 0.0, 0.0, 0.0))
        # All four channels: the default output is RGB only (the shader then saw n x 3 values, not n x 4).
        if not mel.connect_material_expressions(node, "RGBA", custom, "Flash%d" % k):
            raise RuntimeError("flashed material: could not connect Flash%d" % k)
    if not mel.connect_material_property(custom, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR):
        raise RuntimeError("flashed material: could not connect its output")
    mel.recompile_material(material)
    assets.save_loaded_asset(material)
    return material


def flash_cameras(replay, cameras):
    """Gives each blinded player's camera (`cameras`: controller -> camera actor) an instance of FLASHED_MATERIAL with
    the player's blindings (the replay's players' `blindings`, the last FLASHED_SLOTS of them), in the round's own
    folder (another round's import must not change this round's views). Returns the number of cameras."""
    blinded = [p for p in replay["players"] if p.get("blindings") and p["controller"] in cameras]
    if not blinded:
        return 0
    material = flashed_material()
    folder = smoke_volumes.round_folder(FOLDER, replay)
    for player in blinded:
        instance = smoke_volumes.material_instance(folder, "MI_Flashed_%d" % player["controller"], material)
        mel.set_material_instance_scalar_parameter_value(instance, "TicksPerSecond", float(tick_rate_of(replay)))
        blindings = player["blindings"][-FLASHED_SLOTS:]
        values = [(b["tick"], b["duration"]) for b in blindings] + [(0.0, 0.0)] * (FLASHED_SLOTS - len(blindings))
        for k in range(FLASHED_VECTORS):
            (s0, d0), (s1, d1) = values[2 * k], values[2 * k + 1]
            mel.set_material_instance_vector_parameter_value(
                instance, "Flash%d" % k, unreal.LinearColor(r=float(s0), g=float(d0), b=float(s1), a=float(d1)))
        assets.save_loaded_asset(instance)
        component = cameras[player["controller"]].get_editor_property("camera_component")
        settings = component.get_editor_property("post_process_settings")
        blendable = unreal.WeightedBlendable(weight=1.0, object=instance)
        settings.set_editor_property("weighted_blendables", unreal.WeightedBlendables(array=[blendable]))
        component.set_editor_property("post_process_settings", settings)
        component.set_editor_property("post_process_blend_weight", 1.0)
    log("flashed views: %d players" % len(blinded))
    return len(blinded)


def place_effects(sequence, replay, replay_path, end_frame, effects, cameras, spawn):
    """Everything above for a round: the sprite layers (from the agents manifest's `effects`; none written: no sprites),
    the fires, the blasts and flashes, the blinded players' views."""
    instances = update_layers(replay, replay_path, effects) if effects else {}
    if (replay.get("fires") or bursts_of(replay)) and not instances:
        log("fires and blasts: no sprite textures in the agents manifest (export with --players): lights only")
    mesh = sprite_mesh()
    place_fires(sequence, replay, end_frame, instances, mesh, spawn)
    place_bursts(sequence, replay, end_frame, instances, mesh, spawn)
    flash_cameras(replay, cameras)
