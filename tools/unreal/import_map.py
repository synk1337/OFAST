"""Import an OFAST map export into Unreal and take screenshots from known viewpoints — no clicks needed.

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/import_map.py -Gltf <map>.gltf -Shot <folder> -Name <map>).

Inputs (environment variables):
    OFAST_GLTF   absolute path of <map>.gltf (the <map>.ofast.json manifest must sit next to it)
    OFAST_SHOT   folder for screenshots: <map>_ct0.png, <map>_t0.png, <map>_top.png
    OFAST_NAME   map name (asset folder /Game/OFAST/<name>, level /Game/OFAST/Maps/<name>)
    OFAST_VIEWS  optional JSON file of extra viewpoints given as CS2 setpos/setang (see extra_views)
    OFAST_STYLE  optional lighting style (run_ue.ps1 -Style): "standard" (default, CS2's look) or "realistic"
                 (physical lighting); screenshots and level are named <map>[_realistic]
    OFAST_DISABLE  optional, diagnostics only (run_ue.ps1 -Disable): comma list of light sources to leave out —
                 sun, sky (ambient), local, bounce (the local lights' surface-cache bounce); see diagnostic_manifest

Steps: level made once from the Basic template, its environment (sky, clouds, fog, sun, floor) removed →
Interchange *scene* import of the glTF (one actor per placement) → layer-blend and height-blend materials (two-layer
CS2 materials, which glTF cannot describe) → light blockers hidden and
shadow proxies added (no sunlight through single-sided walls) → CS2's environment from the manifest: sun, sky sphere + Sky Light from the exported HDR, local lights with their real shapes and ranges, fixed
exposure, CS2's tone curve (straight or Hable) and colour-correction table (docs/formats/lighting.md) → cameras at the spawns' eyes (and OFAST_VIEWS) with CS2's
default field of view → screenshots → quit. Everything the script adds is tagged "OFAST" and rebuilt on every run.
"""

import json
import math
import os
import time

import unreal

GLTF = os.environ["OFAST_GLTF"]
SHOT_DIR = os.environ["OFAST_SHOT"]
if SHOT_DIR.lower().endswith(".png"):  # a file path was given: use its folder
    SHOT_DIR = os.path.dirname(SHOT_DIR)
NAME = os.environ.get("OFAST_NAME") or os.path.splitext(os.path.basename(GLTF))[0]
MANIFEST = os.path.join(os.path.dirname(GLTF), NAME + ".ofast.json")
# Lighting style: "standard" reproduces CS2's picture (calibrated against CS2 screenshots); "realistic" lights the
# same map physically (Unreal sky, real-world light units, auto exposure, filmic tone curve). Each style has its own
# level (/Game/OFAST/Maps/<name> and <name>_realistic) sharing the imported meshes and materials.
STYLE = os.environ.get("OFAST_STYLE") or "standard"
if STYLE not in ("standard", "realistic"):
    raise RuntimeError("unknown OFAST_STYLE %r (standard or realistic)" % STYLE)
LEVEL = NAME if STYLE == "standard" else "%s_%s" % (NAME, STYLE)
WIDTH, HEIGHT = 1920, 1080
# Longest wait for one screenshot. The first one of a new map waits for all its shaders to compile (de_dust2's
# took over 10 minutes on the first import).
SHOT_TIMEOUT_S = 45 * 60
# Frames to wait before the first screenshot, so meshes and textures have streamed in (shoot_views.WARM_UP_TICKS).
WARM_UP_TICKS = 600
# Seconds on each camera before its screenshot, for Lumen's lighting to settle after the cut. At 2 s de_mirage's
# palace measured 0.79 – 0.93 of CS2 depending on the order of the views; at 6 s and 10 s it is 0.965 every time.
SHOT_DELAY_S = 6.0
# CS2's default fov is 90° measured horizontally on a 4:3 screen; on 16:9 that is 2·atan(tan(45°)·(16/9)/(4/3)).
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))
TAG = unreal.Name("OFAST")
# Actor labels of 3D-skybox geometry start with this (SKYBOX_NODE_PREFIX in ofast-core's map exporter).
SKYBOX_PREFIX = "sky3d_"
# Actor labels of light blockers contain this (LIGHT_BLOCKER_NODE_PREFIX in ofast-core's map exporter).
LIGHT_BLOCKER_PREFIX = "lightblocker_"
# Actor labels of world geometry (built in Hammer, not placed from a model) start with this, unless they are in the
# 3D skybox (WORLD_GEOMETRY_NODE_PREFIX in the exporter).
WORLD_GEOMETRY_PREFIX = "world_"


def log(message):
    unreal.log("OFAST: " + message)


levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)


def prepare_level():
    """Opens /Game/OFAST/Maps/<level> without previous OFAST actors (creates it from the Basic template once)."""
    level_path = "/Game/OFAST/Maps/" + LEVEL
    if unreal.EditorAssetLibrary.does_asset_exist(level_path):
        levels.load_level(level_path)
        old = [a for a in actors.get_all_level_actors() if TAG in a.tags]
        for actor in old:
            actors.destroy_actor(actor)
        log("reused level, removed %d old actors" % len(old))
    elif not levels.new_level_from_template(level_path, "/Engine/Maps/Templates/Template_Default"):
        raise RuntimeError("could not create " + level_path)
    # The template's environment (floor, physical sky, clouds, fog, sun, sky light) is replaced by CS2's own,
    # rebuilt from the manifest on every run (see setup_environment).
    template = (
        unreal.StaticMeshActor,
        unreal.CineCameraActor,
        unreal.SkyAtmosphere,
        unreal.VolumetricCloud,
        unreal.ExponentialHeightFog,
        unreal.DirectionalLight,
        unreal.SkyLight,
    )
    for actor in actors.get_all_level_actors():
        if isinstance(actor, template) and TAG not in actor.tags:
            actors.destroy_actor(actor)


def save_level():
    """Saves the open level — never an engine one (the editor starts on the engine's template map, and saving that
    would change every level made from it)."""
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world().get_path_name()
    if not world.startswith("/Game/"):
        raise RuntimeError("refusing to save " + world)
    levels.save_current_level()


def import_scene():
    before = set(a.get_path_name() for a in actors.get_all_level_actors())
    source = unreal.InterchangeManager.create_source_data(GLTF)
    params = unreal.ImportAssetParameters()
    params.is_automated = True
    params.replace_existing = True
    unreal.InterchangeManager.get_interchange_manager_scripted().import_scene("/Game/OFAST/" + NAME, source, params)
    new = [a for a in actors.get_all_level_actors() if a.get_path_name() not in before]
    for actor in new:
        actor.tags = [TAG]
    lights = sum(1 for a in new if isinstance(a, unreal.Light))
    log("imported %d actors (%d lights)" % (len(new), lights))
    # Mesh actors that ended up without a mesh are holes in the map: report them.
    empty = [
        a.get_actor_label()
        for a in new
        if isinstance(a, unreal.StaticMeshActor) and a.static_mesh_component.static_mesh is None
    ]
    log("%d mesh actors without a mesh%s" % (len(empty), (": " + ", ".join(empty[:15])) if empty else ""))
    # Interchange leaves imported assets in memory only; save them so the project keeps the map after closing.
    unreal.EditorAssetLibrary.save_directory("/Game/OFAST/" + NAME, only_if_is_dirty=True, recursive=True)
    return new


def hide_light_blockers(imported):
    """CS2's "block light" meshes are invisible but seal gaps against light for its bake: made invisible here
    (make_shadow_only), casting sun shadows from both faces and kept in Lumen's distance fields, where they should
    block bounce light as black occluders (not re-checked since their material became masked)."""
    material = ensure_shadow_proxy_material()
    count = 0
    for actor in imported:
        label = actor.get_actor_label()
        if not isinstance(actor, unreal.StaticMeshActor) or LIGHT_BLOCKER_PREFIX not in label:
            continue
        if label.startswith(SKYBOX_PREFIX):
            continue  # the 3D skybox's own blockers: hidden with the rest of its lighting (setup_skybox)
        component = actor.static_mesh_component
        make_shadow_only(component, material, len(component.get_materials()), affect_lumen=True)
        count += 1
    log("light blockers: %d invisible sun-shadow casters" % count)


# Versioned like BLEND_MASTER: the material of 3D-skybox geometry, drawn behind the world (see setup_skybox).
SKYBOX_MASTER = "/Game/OFAST/Materials/M_OFAST_Skybox_v2"
# Shared 1x1 stand-ins for missing glTF texture maps (saved: the master uses them as parameter defaults).
DEFAULT_TEXTURE_FOLDER = "/Game/OFAST/Materials/Defaults"
# How far skybox pixels are pushed back, cm: more than any distance inside a map, so the world always wins.
SKYBOX_DEPTH_OFFSET_CM = 100000.0


def setup_skybox(imported):
    """CS2 draws the 3D skybox first and the world over it, so skybox geometry never hides the world, whatever its
    position. Placed at 16x around the map in one Unreal scene, it can cut through the playable space instead:
    de_dust2's skybox sand lies 1.1 m above the CT spawn's floor and covered it (CS2: 0.12 of the brightness there).

    So every sky3d_ actor gets (component overrides, the world's copies of a mesh are untouched):
    - opaque and masked materials -> an instance of SKYBOX_MASTER (the glTF material's colour, normal and
      metallic-roughness textures and factors) whose pixel depth offset pushes it SKYBOX_DEPTH_OFFSET_CM back;
      translucent ones (clouds) keep their material — they are depth-tested against the world anyway;
    - no shadows and no Lumen presence: the skybox is lit apart in CS2 and must not shade the world (its light
      blockers, at 16x, would otherwise cast huge shadows);
    - its light blockers are simply hidden."""
    with open(GLTF) as f:
        document = json.load(f)
    by_name = {m["name"]: m for m in document.get("materials", [])}
    folder = "/Game/OFAST/%s/Skybox" % NAME
    defaults = skybox_default_textures()
    master = ensure_skybox_master(defaults)
    instances = {}
    actors_done = 0
    for actor in imported:
        if not isinstance(actor, unreal.StaticMeshActor) or not actor.get_actor_label().startswith(SKYBOX_PREFIX):
            continue
        component = actor.static_mesh_component
        component.set_editor_properties({
            "cast_shadow": False,
            "affect_distance_field_lighting": False,
            "affect_dynamic_indirect_lighting": False,
        })
        if LIGHT_BLOCKER_PREFIX in actor.get_actor_label():
            component.set_visibility(False)
            continue
        mesh = component.static_mesh
        if mesh is None:
            continue
        for index, slot in enumerate(mesh.static_materials):
            material = by_name.get(str(slot.material_slot_name))
            if material is None or material.get("alphaMode") == "BLEND":
                continue
            name = material["name"]
            if name not in instances:
                instances[name] = skybox_instance(name, material, document, master, defaults, folder)
            component.set_material(index, instances[name])
        actors_done += 1
    # The instances and the textures they sample must be saved, or the level loses its overrides when reopened.
    unreal.EditorAssetLibrary.save_directory("/Game/OFAST", only_if_is_dirty=True, recursive=True)
    log("3D skybox: %d actors drawn behind the world (%d materials), no shadows" % (actors_done, len(instances)))


def skybox_default_textures():
    """1x1 textures for missing glTF maps (white colour, flat normal, roughness 1 / metalness 0), shared by every
    map and saved."""
    import struct
    import tempfile
    import zlib

    def png(path, rgba):
        raw = b"\x00" + bytes(rgba)
        chunk = lambda kind, data: (struct.pack(">I", len(data)) + kind + data
                                    + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
        with open(path, "wb") as f:
            f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", 1, 1, 8, 6, 0, 0, 0))
                    + chunk(b"IDAT", zlib.compress(raw)) + chunk(b"IEND", b""))

    out = {}
    directory = tempfile.mkdtemp()
    for key, rgba, kind in (("color", (255, 255, 255, 255), "color"), ("normal", (128, 128, 255, 255), "normal"),
                            ("orm", (255, 255, 0, 255), "linear")):
        path = os.path.join(directory, "T_OFAST_Default_%s.png" % key)
        png(path, rgba)
        out[key] = import_texture(path, DEFAULT_TEXTURE_FOLDER, kind)
        unreal.EditorAssetLibrary.save_loaded_asset(out[key])
    return out


def ensure_skybox_master(defaults):
    """Masked material with glTF's PBR inputs (colour, normal and metallic-roughness textures with their factors)
    plus a constant pixel depth offset."""
    if unreal.EditorAssetLibrary.does_asset_exist(SKYBOX_MASTER):
        return unreal.EditorAssetLibrary.load_asset(SKYBOX_MASTER)
    folder, name = SKYBOX_MASTER.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew())
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_MASKED)
    material.set_editor_property("opacity_mask_clip_value", 0.5)  # glTF's default alpha cutoff
    mel = unreal.MaterialEditingLibrary
    sampler = unreal.MaterialSamplerType
    prop = unreal.MaterialProperty

    def texture(param, kind, y):
        node = mel.create_material_expression(material, unreal.MaterialExpressionTextureSampleParameter2D, -900, y)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("sampler_type", {"color": sampler.SAMPLERTYPE_COLOR, "normal": sampler.SAMPLERTYPE_NORMAL,
                                                  "orm": sampler.SAMPLERTYPE_MASKS}[kind])
        node.set_editor_property("texture", defaults[kind])
        return node

    def scalar(param, default, y):
        node = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -600, y)
        node.set_editor_property("parameter_name", param)
        node.set_editor_property("default_value", default)
        return node

    def multiply(a, a_out, b, b_out, y):
        node = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -300, y)
        mel.connect_material_expressions(a, a_out, node, "A")
        mel.connect_material_expressions(b, b_out, node, "B")
        return node

    color = texture("BaseColorTexture", "color", -400)
    normal = texture("NormalTexture", "normal", 0)
    orm = texture("MetallicRoughnessTexture", "orm", 400)
    factor = mel.create_material_expression(material, unreal.MaterialExpressionVectorParameter, -600, -250)
    factor.set_editor_property("parameter_name", "BaseColorFactor")
    factor.set_editor_property("default_value", unreal.LinearColor(1, 1, 1, 1))
    depth = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -300, 850)
    depth.set_editor_property("r", SKYBOX_DEPTH_OFFSET_CM)
    # Python's MaterialProperty has no pixel depth offset: everything goes through a Make Material Attributes node.
    material.set_editor_property("use_material_attributes", True)
    attributes = mel.create_material_expression(material, unreal.MaterialExpressionMakeMaterialAttributes, 0, 0)
    for source, output, pin in (
        (multiply(color, "RGB", factor, "", -400), "", "BaseColor"),
        (multiply(color, "A", factor, "A", -150), "", "OpacityMask"),
        (normal, "RGB", "Normal"),
        (multiply(orm, "G", scalar("RoughnessFactor", 1.0, 500), "", 450), "", "Roughness"),
        (multiply(orm, "B", scalar("MetallicFactor", 0.0, 650), "", 600), "", "Metallic"),
        (depth, "", "PixelDepthOffset"),
    ):
        if not mel.connect_material_expressions(source, output, attributes, pin):
            raise RuntimeError("skybox master: could not connect " + pin)
    mel.connect_material_property(attributes, "", prop.MP_MATERIAL_ATTRIBUTES)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_asset(SKYBOX_MASTER)
    log("created " + SKYBOX_MASTER)
    return material


def skybox_instance(name, material, document, master, defaults, folder):
    """An instance of SKYBOX_MASTER with a glTF material's textures and factors."""
    def image(slot):
        info = slot and document["textures"][slot["index"]]
        return document["images"][info["source"]]["uri"] if info else None

    pbr = material.get("pbrMetallicRoughness", {})
    textures = {
        "BaseColorTexture": (image(pbr.get("baseColorTexture")), "color"),
        "NormalTexture": (image(material.get("normalTexture")), "normal"),
        "MetallicRoughnessTexture": (image(pbr.get("metallicRoughnessTexture")), "orm"),
    }
    instance = blend_instance("Sky_" + name, master, folder)
    for param, (uri, kind) in textures.items():
        texture = import_texture(uri, folder, "linear" if kind == "orm" else kind) if uri else defaults[kind]
        set_texture_parameter(instance, param, texture)
    r, g, b, a = pbr.get("baseColorFactor", [1.0, 1.0, 1.0, 1.0])
    unreal.MaterialEditingLibrary.set_material_instance_vector_parameter_value(
        instance, "BaseColorFactor", unreal.LinearColor(r, g, b, a))
    set_scalar_parameter(instance, "RoughnessFactor", pbr.get("roughnessFactor", 1.0))
    set_scalar_parameter(instance, "MetallicFactor", pbr.get("metallicFactor", 1.0))
    if material.get("doubleSided"):
        overrides = instance.get_editor_property("base_property_overrides")
        overrides.set_editor_property("override_two_sided", True)
        overrides.set_editor_property("two_sided", True)
        instance.set_editor_property("base_property_overrides", overrides)
    CHANGED_INSTANCES.add(instance.get_path_name())  # the vector and override writes are not tracked one by one
    return instance


# Versioned name: bump when the graph changes so existing projects get the new master.
BLEND_MASTER = "/Game/OFAST/Materials/M_OFAST_LayerBlend_v2"
EXPORT_DIR = os.path.dirname(GLTF)
# Blend material instances and the layer textures they sample.
BLEND_FOLDER = "/Game/OFAST/%s/Blend" % NAME


def import_texture(relative_png, folder, kind):
    """Imports one exported PNG as a texture asset (once). kind: 'color' (sRGB), 'normal', or 'linear' (masks).

    Normal maps: the exported PNGs follow glTF's convention (+Y up), Unreal's is DirectX (+Y down), so they get
    flip_green_channel like the ones Interchange imports for the glTF materials (checked: Interchange sets it)."""
    name = os.path.splitext(os.path.basename(relative_png))[0]
    path = folder + "/" + name
    if not unreal.EditorAssetLibrary.does_asset_exist(path):
        task = unreal.AssetImportTask()
        task.filename = os.path.join(EXPORT_DIR, relative_png)
        task.destination_path = folder
        task.automated = True
        task.replace_existing = True
        task.save = False
        unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    texture = unreal.EditorAssetLibrary.load_asset(path)
    if texture is None:
        raise RuntimeError("texture import failed: " + relative_png)
    settings = {
        "color": {},
        "normal": {"compression_settings": unreal.TextureCompressionSettings.TC_NORMALMAP, "srgb": False,
                   "flip_green_channel": True},
        "linear": {"compression_settings": unreal.TextureCompressionSettings.TC_MASKS, "srgb": False},
    }[kind]
    # Only changed settings are written: each write rebuilds the texture. Also fixes textures imported by older runs.
    for prop, value in settings.items():
        if texture.get_editor_property(prop) != value:
            texture.set_editor_property(prop, value)
    return texture


def ensure_blend_master(defaults):
    """The two-layer blend material, built once by script, following csgo_lightmappedgeneric's "fancy blending":

    v = vertexColor.R (painted weight)
    m = modulation mask: texture G (modes 1, 2) or A (mode 3)      -> parameter UseAlphaMask
    s = softness: texture R (mode 1) or BlendSoftness (modes 2, 3) -> parameter UseParamSoftness
    blend = lerp(v, smoothstep(saturate(m - s), saturate(m + s), v), UseModulation)   (mode 0: plain v)
    colour / normal / roughness = lerp(layer 1, layer 2, blend)
    `defaults` are texture assets used as parameter defaults (a sampler parameter needs a matching texture).
    """
    if unreal.EditorAssetLibrary.does_asset_exist(BLEND_MASTER):
        return unreal.EditorAssetLibrary.load_asset(BLEND_MASTER)
    tools = unreal.AssetToolsHelpers.get_asset_tools()
    folder, name = BLEND_MASTER.rsplit("/", 1)
    material = tools.create_asset(name, folder, unreal.Material, unreal.MaterialFactoryNew())
    mel = unreal.MaterialEditingLibrary
    sampler = unreal.MaterialSamplerType

    def node(cls, x, y):
        return mel.create_material_expression(material, cls, x, y)

    def texture(param, x, y, kind, default):
        n = node(unreal.MaterialExpressionTextureSampleParameter2D, x, y)
        n.set_editor_property("parameter_name", param)
        n.set_editor_property(
            "sampler_type",
            {"color": sampler.SAMPLERTYPE_COLOR, "normal": sampler.SAMPLERTYPE_NORMAL, "linear": sampler.SAMPLERTYPE_LINEAR_COLOR}[kind],
        )
        n.set_editor_property("texture", default)
        return n

    c1 = texture("Layer1Color", -900, -500, "color", defaults["c1"])
    c2 = texture("Layer2Color", -900, -250, "color", defaults["c2"])
    n1 = texture("Layer1Normal", -900, 0, "normal", defaults["n1"])
    n2 = texture("Layer2Normal", -900, 250, "normal", defaults["n2"])
    r1 = texture("Layer1Rough", -900, 500, "linear", defaults["r1"])
    r2 = texture("Layer2Rough", -900, 750, "linear", defaults["r2"])
    mod = texture("BlendModulation", -1500, -100, "linear", defaults["mod"])
    def scalar(param, default, x, y):
        n = node(unreal.MaterialExpressionScalarParameter, x, y)
        n.set_editor_property("parameter_name", param)
        n.set_editor_property("default_value", default)
        return n

    def lerp_nodes(a, a_out, b, b_out, alpha, x, y):
        n = node(unreal.MaterialExpressionLinearInterpolate, x, y)
        mel.connect_material_expressions(a, a_out, n, "A")
        mel.connect_material_expressions(b, b_out, n, "B")
        mel.connect_material_expressions(alpha, "", n, "Alpha")
        return n

    param_softness = scalar("BlendSoftness", 0.5, -1800, 200)
    use_alpha_mask = scalar("UseAlphaMask", 0.0, -1800, 300)
    use_param_softness = scalar("UseParamSoftness", 0.0, -1800, 400)
    use_modulation = scalar("UseModulation", 1.0, -1800, 500)
    vertex = node(unreal.MaterialExpressionVertexColor, -1500, 600)

    mask = lerp_nodes(mod, "G", mod, "A", use_alpha_mask, -1350, -100)
    softness = lerp_nodes(mod, "R", param_softness, "", use_param_softness, -1350, 100)
    low = node(unreal.MaterialExpressionSubtract, -1200, -100)
    high = node(unreal.MaterialExpressionAdd, -1200, 100)
    for op in (low, high):
        mel.connect_material_expressions(mask, "", op, "A")
        mel.connect_material_expressions(softness, "", op, "B")
    low_sat = node(unreal.MaterialExpressionSaturate, -1050, -100)
    high_sat = node(unreal.MaterialExpressionSaturate, -1050, 100)
    mel.connect_material_expressions(low, "", low_sat, "")
    mel.connect_material_expressions(high, "", high_sat, "")
    modulated = node(unreal.MaterialExpressionSmoothStep, -900, 900)
    mel.connect_material_expressions(low_sat, "", modulated, "Min")
    mel.connect_material_expressions(high_sat, "", modulated, "Max")
    mel.connect_material_expressions(vertex, "R", modulated, "Value")
    blend = lerp_nodes(vertex, "R", modulated, "", use_modulation, -700, 1000)

    def lerp(a, a_out, b, b_out, y):
        n = node(unreal.MaterialExpressionLinearInterpolate, -400, y)
        mel.connect_material_expressions(a, a_out, n, "A")
        mel.connect_material_expressions(b, b_out, n, "B")
        mel.connect_material_expressions(blend, "", n, "Alpha")
        return n

    mel.connect_material_property(lerp(c1, "RGB", c2, "RGB", -400), "", unreal.MaterialProperty.MP_BASE_COLOR)
    mel.connect_material_property(lerp(n1, "RGB", n2, "RGB", 0), "", unreal.MaterialProperty.MP_NORMAL)
    mel.connect_material_property(lerp(r1, "G", r2, "G", 400), "", unreal.MaterialProperty.MP_ROUGHNESS)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_asset(BLEND_MASTER)
    log("created " + BLEND_MASTER)
    return material


# Versioned like BLEND_MASTER: csgo_environment_blend's height blend.
HEIGHT_BLEND_MASTER = "/Game/OFAST/Materials/M_OFAST_HeightBlend_v1"

# Layer 2's weight, as in Valve's csgo_environment_blend pixel shader (default path, without its view-distance
# softening). The reference is HeightBlend::weight in crates/ofast-core/src/export/environment.rs, which has tests;
# keep the two identical. px / pw: the vertex paint (x = layer 2, w = extra softness), h1 / h2: the height maps' R.
HEIGHT_BLEND_HLSL = """
float paint = saturate(px * 1.1 - 0.05);
float soft = clamp(pw + Softness, 0.0001, 1.0);
float S1 = Scale1 + soft;
float S2 = Scale2 + soft;
float H1 = (h1 - Zero1) * S1;
float slope = S2 - 2.0 * soft;
if (slope == 0.0) slope = 0.0001;
float low = -Zero1 * S1 - (1.0 - Zero2) * S2 - soft;
float high = (1.0 - Zero1) * S1 + Zero2 * S2;
float H2 = (h2 - Zero2) * slope + lerp(low, high, paint);
float top = max(H1, H2);
float w1 = max(H1 - top + soft, 0.0);
float w2 = max(H2 - top + soft, 0.0);
return saturate(w2 / (w1 + w2 + 0.0001) * lerp(0.5, h2, MaskWithHeight) * 2.0);
"""
HEIGHT_BLEND_SCALARS = {  # parameter -> (manifest key, index or None, default)
    "Softness": ("softness", None, 0.01),
    "Zero1": ("zero_point", 0, 0.5),
    "Zero2": ("zero_point", 1, 0.5),
    "Scale1": ("scale", 0, 1.0),
    "Scale2": ("scale", 1, 1.0),
    "MaskWithHeight": ("mask_with_height", None, 0.0),
}
# Texture parameters of the height-blend master: (parameter pattern, file key in the layer, import kind).
HEIGHT_BLEND_TEXTURES = [
    ("%sColor", "color", "color"),
    ("%sNormal", "normal", "normal"),
    ("%sSurface", "surface", "linear"),
    ("%sHeight", "height", "linear"),
]


def ensure_height_blend_master(defaults):
    """csgo_environment_blend's material, built once by script. Per layer: colour (adjustments already baked by
    the exporter), normal, surface (R occlusion, G roughness, B metalness) and height (R). The vertex colour carries
    the paint (R = layer 2 weight, A = extra softness; Unreal hands it over linear — see probe_vertex_color.py).
    A Custom node computes layer 2's weight (HEIGHT_BLEND_HLSL); every output is lerp(layer 1, layer 2, weight).
    `defaults` maps texture parameter names to texture assets (a sampler parameter needs a matching texture)."""
    if unreal.EditorAssetLibrary.does_asset_exist(HEIGHT_BLEND_MASTER):
        return unreal.EditorAssetLibrary.load_asset(HEIGHT_BLEND_MASTER)
    folder, name = HEIGHT_BLEND_MASTER.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew()
    )
    mel = unreal.MaterialEditingLibrary
    sampler = unreal.MaterialSamplerType
    sampler_of = {"color": sampler.SAMPLERTYPE_COLOR, "normal": sampler.SAMPLERTYPE_NORMAL,
                  "linear": sampler.SAMPLERTYPE_MASKS}
    textures = {}
    for row, (pattern, _key, kind) in enumerate(HEIGHT_BLEND_TEXTURES):
        for column, layer in enumerate(("Layer1", "Layer2")):
            param = pattern % layer
            node = mel.create_material_expression(
                material, unreal.MaterialExpressionTextureSampleParameter2D, -1400, -700 + (row * 2 + column) * 260
            )
            node.set_editor_property("parameter_name", param)
            node.set_editor_property("sampler_type", sampler_of[kind])
            node.set_editor_property("texture", defaults[param])
            textures[param] = node
    weight = mel.create_material_expression(material, unreal.MaterialExpressionCustom, -800, 900)
    weight.set_editor_property("description", "HeightBlendWeight")
    weight.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT1)
    weight.set_editor_property("code", HEIGHT_BLEND_HLSL)
    names = ["h1", "h2", "px", "pw"] + list(HEIGHT_BLEND_SCALARS)
    inputs = []
    for input_name in names:
        custom_input = unreal.CustomInput()
        custom_input.set_editor_property("input_name", input_name)
        inputs.append(custom_input)
    weight.set_editor_property("inputs", inputs)
    vertex = mel.create_material_expression(material, unreal.MaterialExpressionVertexColor, -1100, 1300)
    mel.connect_material_expressions(textures["Layer1Height"], "R", weight, "h1")
    mel.connect_material_expressions(textures["Layer2Height"], "R", weight, "h2")
    mel.connect_material_expressions(vertex, "R", weight, "px")
    mel.connect_material_expressions(vertex, "A", weight, "pw")
    for index, (param, (_key, _i, default)) in enumerate(HEIGHT_BLEND_SCALARS.items()):
        scalar = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -1100,
                                                1450 + index * 120)
        scalar.set_editor_property("parameter_name", param)
        scalar.set_editor_property("default_value", default)
        mel.connect_material_expressions(scalar, "", weight, param)

    def lerp(param, output, y):
        node = mel.create_material_expression(material, unreal.MaterialExpressionLinearInterpolate, -400, y)
        mel.connect_material_expressions(textures["Layer1" + param], output, node, "A")
        mel.connect_material_expressions(textures["Layer2" + param], output, node, "B")
        mel.connect_material_expressions(weight, "", node, "Alpha")
        return node

    prop = unreal.MaterialProperty
    mel.connect_material_property(lerp("Color", "RGB", -600), "", prop.MP_BASE_COLOR)
    mel.connect_material_property(lerp("Normal", "RGB", -300), "", prop.MP_NORMAL)
    mel.connect_material_property(lerp("Surface", "R", 0), "", prop.MP_AMBIENT_OCCLUSION)
    mel.connect_material_property(lerp("Surface", "G", 300), "", prop.MP_ROUGHNESS)
    mel.connect_material_property(lerp("Surface", "B", 600), "", prop.MP_METALLIC)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_asset(HEIGHT_BLEND_MASTER)
    log("created " + HEIGHT_BLEND_MASTER)
    return material


def height_blend_instances():
    """An instance of the height-blend master for every csgo_environment_blend material (glTF stand-in: layer 1
    only), with both layers' textures and its blend settings (extras.ofast.height_blend, written by the exporter).
    Returns glTF material name -> instance; main() puts them on the meshes after the scene import."""
    with open(GLTF) as f:
        document = json.load(f)
    blends = {}
    for material in document.get("materials", []):
        blend = material.get("extras", {}).get("ofast", {}).get("height_blend")
        if not blend:
            continue
        missing = ["%s.%s" % (layer, key) for layer in ("layer1", "layer2") for _p, key, _k in HEIGHT_BLEND_TEXTURES
                   if not blend[layer].get(key)]
        if missing:
            log("height blend: %s keeps layer 1 only (no %s)" % (material["name"], ", ".join(missing)))
            continue
        blends[material["name"]] = blend
    instances = {}
    master = None
    for name, blend in blends.items():
        textures = {}
        for pattern, key, kind in HEIGHT_BLEND_TEXTURES:
            for layer in ("Layer1", "Layer2"):
                textures[pattern % layer] = import_texture(blend[layer.lower()][key], BLEND_FOLDER, kind)
        master = master or ensure_height_blend_master(textures)
        instance = blend_instance(name, master)
        for param, texture in textures.items():
            set_texture_parameter(instance, param, texture)
        for param, (key, index, default) in HEIGHT_BLEND_SCALARS.items():
            value = blend.get(key, default)
            if index is not None:
                value = value[index]
            set_scalar_parameter(instance, param, value)
        instances[name] = instance
    if instances:
        log("height blend: %d materials" % len(instances))
    return instances


def blend_instance(name, master, folder=BLEND_FOLDER):
    """The material instance <folder>/MI_<name> (created on first use), parented to `master`."""
    path = folder + "/MI_" + name
    if unreal.EditorAssetLibrary.does_asset_exist(path):
        instance = unreal.EditorAssetLibrary.load_asset(path)
    else:
        instance = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
            "MI_" + name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew()
        )
        CHANGED_INSTANCES.add(instance.get_path_name())
    if instance.get_editor_property("parent") != master:
        unreal.MaterialEditingLibrary.set_material_instance_parent(instance, master)
        CHANGED_INSTANCES.add(instance.get_path_name())
    return instance


# Material instances whose parent or parameters this run changed. These Python calls do not mark the package dirty,
# so save_directory(only_if_is_dirty) skipped them: de_inferno's tone mapper instance kept its old parent on disk
# and the reopened level looked different from the import's screenshots. save_changed_instances() saves them.
CHANGED_INSTANCES = set()


def save_changed_instances():
    for path in sorted(CHANGED_INSTANCES):
        unreal.EditorAssetLibrary.save_asset(path, only_if_is_dirty=False)
    CHANGED_INSTANCES.clear()


# Material-instance parameters are only written when they change: every write updates the instance. When the
# instances were set up after the scene import, a re-run (instances already on the meshes) took 8 minutes for
# de_inferno's 369 instances × 14 writes and crashed the GPU twice (DXGI_ERROR_DEVICE_HUNG during the GPU scene
# update); set up before the import and written only when changed, it takes 2 seconds.
def set_texture_parameter(instance, param, texture):
    mel = unreal.MaterialEditingLibrary
    if mel.get_material_instance_texture_parameter_value(instance, param) != texture:
        mel.set_material_instance_texture_parameter_value(instance, param, texture)
        CHANGED_INSTANCES.add(instance.get_path_name())


def set_scalar_parameter(instance, param, value):
    mel = unreal.MaterialEditingLibrary
    if abs(mel.get_material_instance_scalar_parameter_value(instance, param) - float(value)) > 1e-6:
        mel.set_material_instance_scalar_parameter_value(instance, param, float(value))
        CHANGED_INSTANCES.add(instance.get_path_name())


# Versioned like BLEND_MASTER: the material of the shadow proxies and light blockers (two-sided, drawn only in
# shadow passes).
SHADOW_PROXY_MATERIAL = "/Game/OFAST/Materials/M_OFAST_ShadowProxy_v2"
# A mesh needs a shadow proxy only if it is at least this wide in two directions (see add_shadow_proxies).
PROXY_MIN_SPAN_CM = 100.0
# Lighting channels: the shadow-only meshes (proxies, light blockers) are in channel 1 alone and only the sun shares
# it, so they block sunlight but no local light (see make_shadow_only). Everything else stays in channel 0.
SHADOW_ONLY_CHANNELS = unreal.LightingChannels(channel0=False, channel1=True, channel2=False)
SUN_CHANNELS = unreal.LightingChannels(channel0=True, channel1=True, channel2=False)


def ensure_shadow_proxy_material():
    """Two-sided, masked: opacity 0 everywhere except in shadow passes (Shadow Pass Switch), so a proxy is an
    ordinary visible mesh that nobody sees but that casts shadows. Hiding the mesh instead (bVisible off +
    bCastHiddenShadow, or out of the main pass) silently removes a Nanite mesh from the shadow passes too; see
    make_shadow_only for why the components also render without Nanite.

    Tried and dropped: keeping only the faces the light sees from behind (TwoSidedSign in the shadow branch), so a
    proxy would not cast again what its visible mesh already casts. It let sunlight onto de_mirage's B-apartment
    stairs, which both faces block, and changed nothing measurable elsewhere."""
    if unreal.EditorAssetLibrary.does_asset_exist(SHADOW_PROXY_MATERIAL):
        return unreal.EditorAssetLibrary.load_asset(SHADOW_PROXY_MATERIAL)
    folder, name = SHADOW_PROXY_MATERIAL.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew()
    )
    material.set_editor_property("two_sided", True)
    material.set_editor_property("blend_mode", unreal.BlendMode.BLEND_MASKED)
    mel = unreal.MaterialEditingLibrary
    switch = mel.create_material_expression(material, unreal.MaterialExpressionShadowReplace, -300, 0)
    for value, pin, y in ((0.0, "Default", -100), (1.0, "Shadow", 100)):
        constant = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -600, y)
        constant.set_editor_property("r", value)
        if not mel.connect_material_expressions(constant, "", switch, pin):
            raise RuntimeError("shadow proxy material: could not connect " + pin)
    mel.connect_material_property(switch, "", unreal.MaterialProperty.MP_OPACITY_MASK)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_loaded_asset(material)
    return material


def add_shadow_proxies(imported):
    """CS2's walls are single-sided and Nanite only casts shadows from front faces (two-sidedness comes from the
    material; the component's bCastShadowAsTwoSided does not reach Nanite). Seen from the sun, an interior wall
    shows its back, so sunlight leaked through walls into interiors (de_mirage's B apartments); CS2's light bake
    treats faces as two-sided.

    Making the real materials two-sided would fix the shadows but also turn the meshes' distance fields into thin
    shells (DistanceFieldAtlas.cpp reads the material's two-sidedness), which lets Lumen's sky light and bounce leak
    — measured: the calibration views moved by up to +80 %. So every opaque, single-sided mesh instead gets an
    invisible copy with a two-sided material that only casts shadows and that Lumen ignores (make_shadow_only).

    Only world geometry gets one (labels with WORLD_GEOMETRY_PREFIX: built in Hammer, not placed from a model).
    Hammer geometry is often a lone wall with no outer face; models are usually closed. The casters CS2 needs from
    behind were all world geometry (de_inferno's T-spawn street: a plaster wall; de_mirage's B-apartment stairs: two
    plaster walls), and de_inferno's spawns measured the same with every mesh proxied (0.72 / 1.21 of CS2) as with
    world geometry only (0.73 / 1.21), with 57 – 86 % fewer proxies.

    Only meshes at least PROXY_MIN_SPAN_CM wide in two directions (walls, floors, roofs) get one: thin or small
    pieces (pipes, wires, bottles, signs) cannot let a visible patch of sunlight through. On de_nuke this kept
    735 of 5368 candidates (before the world-geometry rule)."""
    material = ensure_shadow_proxy_material()
    count = 0
    small = 0
    others = 0
    for actor in imported:
        if not isinstance(actor, unreal.StaticMeshActor):
            continue
        label = actor.get_actor_label()
        if label.startswith(SKYBOX_PREFIX) or LIGHT_BLOCKER_PREFIX in label:
            continue  # far away / handled by hide_light_blockers
        if not label.startswith(WORLD_GEOMETRY_PREFIX):
            others += 1  # props, model pieces and decals: usually closed or not walls
            continue
        component = actor.static_mesh_component
        mesh = component.static_mesh
        slots = component.get_materials()
        if mesh is None or not slots or any(slot is None for slot in slots):
            continue
        # Masked / translucent surfaces (decals, foliage, fences, glass) would cast solid shadows, and two-sided
        # ones already cast from both faces: no proxy for them. get_blend_mode honours material-instance overrides
        # (Interchange's glTF instances inherit their blend mode from overriding parents).
        if any(slot.get_blend_mode() != unreal.BlendMode.BLEND_OPAQUE or is_two_sided(slot) for slot in slots):
            continue
        if sorted(mesh_spans(mesh, actor))[1] < PROXY_MIN_SPAN_CM:
            small += 1
            continue
        proxy = actors.spawn_actor_from_object(mesh, actor.get_actor_location(), actor.get_actor_rotation())
        proxy.set_actor_transform(actor.get_actor_transform(), False, False)
        proxy.tags = [TAG]
        proxy.set_actor_label("shadowproxy_" + label)
        make_shadow_only(proxy.static_mesh_component, material, len(slots), affect_lumen=False)
        count += 1
    log("shadow proxies: %d (two-sided shadows for single-sided walls; %d smaller meshes and %d props, model pieces "
        "and decals skipped)" % (count, small, others))


def mesh_spans(mesh, actor):
    """The size (cm) of an actor's mesh along its three local axes, with the actor's scale."""
    box = mesh.get_bounding_box()
    scale = actor.get_actor_scale3d()
    return [abs((box.max.x - box.min.x) * scale.x), abs((box.max.y - box.min.y) * scale.y),
            abs((box.max.z - box.min.z) * scale.z)]


def is_two_sided(material):
    """A material's two-sidedness, following material-instance overrides up to the base material (Python has no
    IsTwoSided)."""
    while isinstance(material, unreal.MaterialInstance):
        overrides = material.get_editor_property("base_property_overrides")
        if overrides.get_editor_property("override_two_sided"):
            return overrides.get_editor_property("two_sided")
        material = material.get_editor_property("parent")
    return bool(material and material.get_editor_property("two_sided"))


def make_shadow_only(component, material, slot_count, affect_lumen):
    """Turns a static mesh component into an invisible, two-sided shadow caster without collision. `affect_lumen`
    keeps it in Lumen's scene as a black (absorbing) occluder.

    It stays an ordinary visible mesh whose material (ensure_shadow_proxy_material) only shows in shadow passes,
    and it renders without Nanite (bDisallowNanite: the mesh's fallback LOD, an ordinary static mesh). Measured on
    de_inferno's T-spawn street, which stayed sunlit (2.7x CS2) although its wall had a proxy:
    - a hidden Nanite mesh (bVisible off + bCastHiddenShadow, or out of the main pass) casts no shadow at all;
    - a visible Nanite mesh with this material casts none either, although the material has bUsedWithNanite and
      Nanite's virtual-shadow-map raster takes the switch's shadow branch (NaniteRasterizer.usf defines
      SHADOW_DEPTH_SHADER as DEPTH_ONLY); cause not found;
    - the same mesh without Nanite shades the street as in CS2 (2.05x -> 1.20x for the whole view).

    It casts shadows for the sun only (lighting channel 1, which only the sun shares: SUN_CHANNELS): a proxy lies
    on its wall, and for the lights mounted on walls and ceilings it shaded the very surfaces they light (de_mirage's
    palace went from 0.97 to 0.62 of CS2)."""
    for index in range(slot_count):
        component.set_material(index, material)
    component.set_collision_enabled(unreal.CollisionEnabled.NO_COLLISION)
    component.set_editor_properties(
        {
            "disallow_nanite": True,
            "lighting_channels": SHADOW_ONLY_CHANNELS,
            "cast_shadow": True,
            "visible_in_reflection_captures": False,
            "visible_in_real_time_sky_captures": False,
            "visible_in_ray_tracing": False,
            "affect_distance_field_lighting": affect_lumen,
            "affect_dynamic_indirect_lighting": affect_lumen,
        }
    )


def layer_blend_instances():
    """A blend instance for every two-layer csgo_lightmappedgeneric material (glTF stand-in: layer 1 only).
    Returns glTF material name -> instance; main() puts them on the meshes after the scene import."""
    with open(GLTF) as f:
        document = json.load(f)
    layered = {}
    for material in document.get("materials", []):
        layers = material.get("extras", {}).get("ofast", {}).get("layers")
        if not layers:
            continue
        # The blend master samples all six textures; a layer without one keeps the glTF stand-in (layer 1).
        missing = [layer + "." + kind for layer in ("layer1", "layer2") for kind in ("color", "normal", "roughness")
                   if not layers[layer].get(kind)]
        if missing:
            log("layer blend: %s keeps layer 1 only (no %s)" % (material["name"], ", ".join(missing)))
            continue
        layered[material["name"]] = layers
    instances = {}
    master = None
    for name, layers in layered.items():
        textures = {
            "c1": import_texture(layers["layer1"]["color"], BLEND_FOLDER, "color"),
            "c2": import_texture(layers["layer2"]["color"], BLEND_FOLDER, "color"),
            "n1": import_texture(layers["layer1"]["normal"], BLEND_FOLDER, "normal"),
            "n2": import_texture(layers["layer2"]["normal"], BLEND_FOLDER, "normal"),
            "r1": import_texture(layers["layer1"]["roughness"], BLEND_FOLDER, "linear"),
            "r2": import_texture(layers["layer2"]["roughness"], BLEND_FOLDER, "linear"),
            "mod": import_texture(layers["modulation"], BLEND_FOLDER, "linear") if layers.get("modulation") else None,
        }
        if textures["mod"] is None:
            textures["mod"] = textures["r1"]
        master = master or ensure_blend_master(textures)
        instance = blend_instance(name, master)
        for param, key in (("Layer1Color", "c1"), ("Layer2Color", "c2"), ("Layer1Normal", "n1"),
                           ("Layer2Normal", "n2"), ("Layer1Rough", "r1"), ("Layer2Rough", "r2"),
                           ("BlendModulation", "mod")):
            set_texture_parameter(instance, param, textures[key])
        if layers.get("softness") is not None:
            set_scalar_parameter(instance, "BlendSoftness", layers["softness"])
        mode = int(layers.get("fancy_blending") or 0)
        has_mask = layers.get("modulation") is not None
        set_scalar_parameter(instance, "UseModulation", 1.0 if (mode > 0 and has_mask) else 0.0)
        set_scalar_parameter(instance, "UseAlphaMask", 1.0 if mode == 3 else 0.0)
        set_scalar_parameter(instance, "UseParamSoftness", 1.0 if mode in (2, 3) else 0.0)
        instances[name] = instance
    if instances:
        log("layer blend: %d materials" % len(instances))
    return instances


def swap_material_slots(imported, instances):
    """Puts `instances` (glTF material name -> material) on the imported static meshes' slots that use those glTF
    materials (meshes are shared by every actor that places them). Returns the number of slots changed."""
    swapped = 0
    seen = set()
    for actor in imported:
        if not isinstance(actor, unreal.StaticMeshActor):
            continue
        mesh = actor.static_mesh_component.static_mesh
        if mesh is None or mesh.get_path_name() in seen:
            continue
        seen.add(mesh.get_path_name())
        for index, slot in enumerate(mesh.static_materials):
            slot_name = str(slot.material_slot_name)
            current = slot.material_interface.get_name() if slot.material_interface else ""
            # Exact names only (tinted variants are their own glTF materials); Interchange may prefix "MI_".
            for candidate in (slot_name, current, current[3:] if current.startswith("MI_") else None):
                if candidate in instances:
                    mesh.set_material(index, instances[candidate])
                    swapped += 1
                    break
    return swapped


# Lumen carries less bounce light than CS2's path-traced lightmaps. Measured compensation on top of CS2's own bounce
# scales (light_environment bouncescale / skybouncescale, each light's bouncescale), fitted on de_mirage's four
# calibration views (docs/formats/lighting.md): CT / T / palace / B apartments at 0.95 / 0.86 / 0.81 / 0.74 of CS2
# before, 0.99 / 0.96 / 0.93 / 0.75 after (CT and T cells 0.88-1.04; the apartments' corridor bounce stays short).
OUTDOOR_BOUNCE_GAIN = 1.25  # sun and sky light
LOCAL_BOUNCE_GAIN = 2.0  # local lights (palace: CS2 bounces 0.7x the direct light, Lumen 0.43x)

# Versioned like BLEND_MASTER.
SKY_MASTER = "/Game/OFAST/Materials/M_OFAST_Sky_v1"
# Radius of the sky sphere (cm): beyond the 3D skybox, which reaches about 3 km from the map centre.
SKY_RADIUS = 1.0e6


def spawn(cls, location=(0.0, 0.0, 0.0), rotation=None):
    actor = actors.spawn_actor_from_class(cls, unreal.Vector(*location), rotation or unreal.Rotator())
    actor.tags = [TAG]
    return actor


def import_sky_cube(hdr, asset_name):
    """Imports an exported equirect .hdr; Unreal turns a 2:1 .hdr into a TextureCube by itself."""
    task = unreal.AssetImportTask()
    task.filename = os.path.join(EXPORT_DIR, hdr)
    task.destination_path = "/Game/OFAST/" + NAME
    task.destination_name = asset_name
    task.automated = True
    task.replace_existing = True
    task.save = False
    unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    paths = list(task.imported_object_paths)
    cube = unreal.load_asset(paths[0]) if paths else None
    if not isinstance(cube, unreal.TextureCube):
        raise RuntimeError("sky did not import as a TextureCube: %s" % cube)
    cube.set_editor_properties(
        {
            "compression_settings": unreal.TextureCompressionSettings.TC_HDR,
            "srgb": False,
            "lod_group": unreal.TextureGroup.TEXTUREGROUP_SKYBOX,
        }
    )
    unreal.EditorAssetLibrary.save_loaded_asset(cube)
    return cube


def ensure_sky_master(cube):
    """Unlit sky material: the cube map looked up along the view direction (−CameraVector), times SkyIntensity.
    Looking up by direction makes the sphere's size and orientation irrelevant."""
    if unreal.EditorAssetLibrary.does_asset_exist(SKY_MASTER):
        return unreal.EditorAssetLibrary.load_asset(SKY_MASTER)
    folder, name = SKY_MASTER.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew()
    )
    material.set_editor_properties(
        {"shading_model": unreal.MaterialShadingModel.MSM_UNLIT, "two_sided": True, "is_sky": True}
    )
    mel = unreal.MaterialEditingLibrary
    camera = mel.create_material_expression(material, unreal.MaterialExpressionCameraVectorWS, -900, 0)
    flip = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -700, 0)
    flip.set_editor_property("const_b", -1.0)
    mel.connect_material_expressions(camera, "", flip, "A")
    sample = mel.create_material_expression(material, unreal.MaterialExpressionTextureSampleParameterCube, -500, 0)
    sample.set_editor_properties(
        {"parameter_name": "SkyCube", "texture": cube, "sampler_type": unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR}
    )
    mel.connect_material_expressions(flip, "", sample, "UVs")
    intensity = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -500, 250)
    intensity.set_editor_properties({"parameter_name": "SkyIntensity", "default_value": 1.0})
    scaled = mel.create_material_expression(material, unreal.MaterialExpressionMultiply, -250, 0)
    mel.connect_material_expressions(sample, "RGB", scaled, "A")
    mel.connect_material_expressions(intensity, "", scaled, "B")
    mel.connect_material_property(scaled, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_loaded_asset(material)
    return material


def spawn_physical_sky(sky_light_intensity=1.0):
    """Unreal's physical sky: atmosphere + a real-time sky light capturing it."""
    spawn(unreal.SkyAtmosphere)
    light = spawn(unreal.SkyLight)
    light.light_component.set_mobility(unreal.ComponentMobility.MOVABLE)
    light.light_component.set_editor_properties({"real_time_capture": True, "intensity": sky_light_intensity})


def spawn_post_volume():
    """An unbound post-process volume; returns (volume, settings, override) — call override(name, value) for each
    setting, then volume.set_editor_property("settings", settings)."""
    volume = spawn(unreal.PostProcessVolume)
    volume.set_actor_label("OFAST_PostProcess")
    volume.set_editor_properties({"unbound": True, "priority": 10.0, "blend_weight": 1.0, "enabled": True})
    settings = volume.settings

    def override(name, value):
        settings.set_editor_property("override_" + name, True)
        settings.set_editor_property(name, value)

    return volume, settings, override


def setup_fallback_sky():
    """Unreal's physical sky, for exports without a CS2 sky."""
    spawn_physical_sky()
    log("WARNING: no sky in the manifest; using Unreal's physical sky (not calibrated against CS2)")


def setup_sky(manifest):
    """Visible sky (a big sphere with the CS2 sky) and the Sky Light: lit by the same cube map, or by the flat sky
    colour's cube map when the map's ambient comes from `skycolor` (manifest ambient.hdr)."""
    sky = manifest.get("sky")
    if not sky:
        if not manifest.get("no_ambient"):
            setup_fallback_sky()
        return
    cube = import_sky_cube(sky["hdr"], "T_%s_Sky" % NAME)
    master = ensure_sky_master(cube)
    instance_path = "/Game/OFAST/%s/MI_%s_Sky" % (NAME, NAME)
    if unreal.EditorAssetLibrary.does_asset_exist(instance_path):
        instance = unreal.EditorAssetLibrary.load_asset(instance_path)
    else:
        folder, name = instance_path.rsplit("/", 1)
        instance = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
            name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew()
        )
    instance.set_editor_property("parent", master)
    mel = unreal.MaterialEditingLibrary
    mel.set_material_instance_texture_parameter_value(instance, "SkyCube", cube)
    # CS2 radiance → Unreal luminance, and the render-only bias (brightens the visible sky, not the lighting).
    mel.set_material_instance_scalar_parameter_value(instance, "SkyIntensity", sky.get("sky_intensity", 1.0))
    unreal.EditorAssetLibrary.save_loaded_asset(instance)

    sphere = spawn(unreal.StaticMeshActor)
    sphere.set_actor_label("OFAST_SkySphere")
    component = sphere.static_mesh_component
    mesh = unreal.load_asset("/Engine/BasicShapes/Sphere")
    component.set_static_mesh(mesh)
    component.set_material(0, instance)
    sphere.set_actor_scale3d(unreal.Vector(1, 1, 1) * (SKY_RADIUS / mesh.get_bounds().sphere_radius))
    # The sphere is only a backdrop: it must not shadow, occlude or bounce light (it encloses the whole map).
    component.set_editor_properties(
        {
            "cast_shadow": False,
            "affect_dynamic_indirect_lighting": False,
            "affect_distance_field_lighting": False,
            "visible_in_ray_tracing": False,
        }
    )

    ambient = manifest.get("ambient") or {}
    ambient_cube = import_sky_cube(ambient["hdr"], "T_%s_Ambient" % NAME) if ambient.get("hdr") else cube
    light = spawn(unreal.SkyLight)
    component = light.light_component
    component.set_mobility(unreal.ComponentMobility.MOVABLE)
    # Below the horizon: CS2's virtual ground (radiance computed by the exporter; black when its albedo is 0).
    lower = ambient.get("lower_hemisphere_color") or [0, 0, 0]
    component.set_editor_properties(
        {
            "real_time_capture": False,
            "source_type": unreal.SkyLightSourceType.SLS_SPECIFIED_CUBEMAP,
            "cubemap": ambient_cube,
            "intensity": ambient.get("sky_light_intensity", 1.0),
            "indirect_lighting_intensity": ambient.get("indirect_intensity", 1.0) * OUTDOOR_BOUNCE_GAIN,
            "lower_hemisphere_is_black": True,
            "lower_hemisphere_color": unreal.LinearColor(lower[0], lower[1], lower[2], 1.0),
        }
    )
    component.recapture_sky()
    log("sky: %s, sky light %s × %.3f" % (cube.get_name(), ambient_cube.get_name(), ambient.get("sky_light_intensity", 1.0)))


def setup_sun(manifest):
    sun = manifest.get("sun")
    if not sun:
        # No light_environment: a plain sun so the map is not black (not calibrated against CS2).
        actor = spawn(unreal.DirectionalLight, rotation=unreal.Rotator(roll=0.0, pitch=-50.0, yaw=30.0))
        actor.light_component.set_editor_property("lighting_channels", SUN_CHANNELS)
        log("WARNING: no sun in the manifest; using a default sun")
        return
    pitch, yaw, roll = sun["rotation"]
    actor = spawn(unreal.DirectionalLight, rotation=unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw))
    component = actor.light_component
    component.set_mobility(unreal.ComponentMobility.MOVABLE)
    color = sun.get("color") or [1, 1, 1]
    component.set_light_color(unreal.LinearColor(color[0], color[1], color[2], 1.0))
    component.set_editor_properties(
        {
            "intensity": sun["illuminance_lux"],
            "light_source_angle": sun.get("source_angle", 1.0),
            "indirect_lighting_intensity": sun.get("indirect_intensity", 1.0) * OUTDOOR_BOUNCE_GAIN,
            "atmosphere_sun_light": False,
            "use_temperature": False,
            "cast_shadows": True,
            "lighting_channels": SUN_CHANNELS,
        }
    )
    log("sun: pitch %.1f yaw %.1f, %.2f lux" % (pitch, yaw, sun["illuminance_lux"]))


def setup_post(manifest):
    """Unbound post-process volume reproducing CS2's picture: fixed exposure (EV100 from the exporter, measured
    against CS2 screenshots), then OFAST's tone mapper (cs2_tonemapper_instance): a straight line when the map's curve
    is one (a shoulder of 0), CS2's Hable curve otherwise, then the map's colour-correction table. No bloom, no
    vignette."""
    post = manifest.get("post") or {}
    # Sunlit views sit at CS2's minimum exposure, the highest EV100.
    ev100 = post.get("ev100_max", 0.0)
    volume, settings, override = spawn_post_volume()

    override("auto_exposure_method", unreal.AutoExposureMethod.AEM_MANUAL)
    override("auto_exposure_apply_physical_camera_exposure", False)
    # Manual exposure without the physical camera: picture = luminance × 2^bias, so bias = −EV100.
    override("auto_exposure_bias", -ev100)
    override("tone_curve_amount", 0.0)
    override("expand_gamut", 0.0)
    override("blue_correction", 0.0)
    # CS2's bloom, colour correction and fog change nothing measurable on de_mirage (toggled in CS2:
    # r_csgo_render_post_bloom, r_csgo_render_post_colorcorrection, fog_enable), so none of them is added here.
    override("bloom_intensity", 0.0)
    override("vignette_intensity", 0.0)
    # Keys the .vpost lacks are written as null: drop them so the defaults below apply.
    tonemap = {key: value for key, value in (post.get("tonemap") or {}).items() if value is not None}
    # Unreal's own tone mapper is always replaced: CS2 applies its colour-correction table after the curve, on every
    # map with one (measured on de_anubis), and a replacing tone mapper is the last step Unreal runs.
    hable = tonemap.get("shoulder_strength", 0.0) > 0.0
    color_correction = post.get("color_correction") or {}
    lut = import_lut(color_correction)
    tonemapper = cs2_tonemapper_instance(tonemap, hable, lut, color_correction.get("size", 32))
    settings.set_editor_property(
        "weighted_blendables",
        unreal.WeightedBlendables(array=[unreal.WeightedBlendable(weight=1.0, object=tonemapper)]),
    )
    curve = "straight tone curve"
    if hable:
        curve = "Hable tone curve (shoulder %.2f, white point %.2f)" % (tonemap["shoulder_strength"],
                                                                     tonemap.get("white_point", 1.0))
    volume.set_editor_property("settings", settings)
    log("post: manual exposure EV100 %.3f, %s, %s" % (
        ev100, curve, "colour table " + lut.get_name() if lut else "no colour table"))


# Versioned like BLEND_MASTER: OFAST's replacement tone mapper (CS2's curve, then CS2's colour-correction table).
TONEMAPPER = "/Game/OFAST/Materials/M_OFAST_Tonemap_v3"
# Hable maps: shown = LUT(srgb(H(INPUT x) / H(WHITE W))), x = exposure x scene (what Unreal shows linearly with the
# manual exposure above), W the .vpost white point, LUT the .vpost's colour-correction table. Measured on de_anubis,
# whose visible sky is the sky that lights it (docs/formats/lighting.md): with the table inverted, CS2's sky pixels
# follow the stock Hable shape; three exposure pairs (de_anubis CT and T, de_inferno CT) fix the white at 22 – 24
# (WHITE 6, W = 4), and sky pixels of known radiance then give INPUT 3.37 – 3.44 on both de_anubis spawns. Its lit
# surfaces agree (3.27 and 3.41), so OFAST's light is right there. The earlier rule (H(2.5 x) / H(1.25 W), no table)
# was fitted on de_inferno's surfaces, whose light OFAST underestimates; it made de_anubis 1.3x too bright.
HABLE_INPUT_SCALE = 3.4
HABLE_WHITE_SCALE = 6.0
TONEMAP_SCALES = {"InputScale": HABLE_INPUT_SCALE, "WhiteScale": HABLE_WHITE_SCALE}
HABLE_PARAMETERS = {  # material parameter -> .vpost tone map key (manifest post.tonemap)
    "ShoulderStrength": "shoulder_strength",
    "LinearStrength": "linear_strength",
    "LinearAngle": "linear_angle",
    "ToeStrength": "toe_strength",
    "ToeNumerator": "toe_numerator",
    "ToeDenominator": "toe_denominator",
    "WhitePoint": "white_point",
}
# Switches and table size, set per map (cs2_tonemapper_instance).
TONEMAP_SWITCHES = {"Hable": 1.0, "UseLut": 0.0, "LutSize": 32.0}
TONEMAP_HLSL = """
float3 x = max(Color * Exposure, 0.0);
float3 c;
if (Hable > 0.5)
{
    x *= InputScale;
    float A = ShoulderStrength, B = LinearStrength, C = LinearAngle, D = ToeStrength, E = ToeNumerator, F = ToeDenominator;
    float3 h = (x * (A * x + C * B) + D * E) / (x * (A * x + B) + D * F) - E / F;
    float w = WhitePoint * WhiteScale;
    float hw = (w * (A * w + C * B) + D * E) / (w * (A * w + B) + D * F) - E / F;
    c = saturate(h / hw);
}
else
{
    c = saturate(x);
}
// sRGB encoding (lerp/step instead of a vector ?: so it compiles under HLSL 2021 too).
float3 s = lerp(1.055 * pow(c, 1.0 / 2.4) - 0.055, c * 12.92, step(c, 0.0031308));
if (UseLut > 0.5)
{
    // CS2's 3D table, stored as LutSize blue slices side by side (pixel (b * n + r, g)): trilinear lookup as two
    // bilinear ones on neighbouring slices, at texel centres so a slice never bleeds into the next.
    float n = LutSize;
    float3 p = s * (n - 1.0);
    float b0 = floor(p.b);
    float b1 = min(b0 + 1.0, n - 1.0);
    float v = (p.g + 0.5) / n;
    float3 l0 = Texture2DSampleLevel(Lut, LutSampler, float2((b0 * n + p.r + 0.5) / (n * n), v), 0).rgb;
    float3 l1 = Texture2DSampleLevel(Lut, LutSampler, float2((b1 * n + p.r + 0.5) / (n * n), v), 0).rgb;
    s = lerp(l0, l1, p.b - b0);
}
return s;
"""
LUT_TEXTURE_SETTINGS = {
    # Uncompressed RGBA8, no sRGB decoding, no mips, bilinear, clamped: the table's bytes as they are.
    "compression_settings": unreal.TextureCompressionSettings.TC_VECTOR_DISPLACEMENTMAP,
    "srgb": False,
    "mip_gen_settings": unreal.TextureMipGenSettings.TMGS_NO_MIPMAPS,
    "lod_group": unreal.TextureGroup.TEXTUREGROUP_COLOR_LOOKUP_TABLE,
    "filter": unreal.TextureFilter.TF_BILINEAR,
    "address_x": unreal.TextureAddress.TA_CLAMP,
    "address_y": unreal.TextureAddress.TA_CLAMP,
    "never_stream": True,
}


def import_lut(color_correction):
    """The exported colour-correction table (<map>_lut.png, see the exporter's lut_strip) as a texture, or None."""
    if not color_correction or not color_correction.get("lut"):
        return None
    task = unreal.AssetImportTask()
    task.filename = os.path.join(EXPORT_DIR, color_correction["lut"])
    task.destination_path = "/Game/OFAST/" + NAME
    task.automated = True
    task.replace_existing = True
    task.save = False
    unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    paths = list(task.imported_object_paths)
    texture = unreal.load_asset(paths[0]) if paths else None
    if not isinstance(texture, unreal.Texture2D):
        raise RuntimeError("colour table did not import as a texture: %s" % color_correction["lut"])
    texture.set_editor_properties(LUT_TEXTURE_SETTINGS)
    unreal.EditorAssetLibrary.save_loaded_asset(texture)
    return texture


def identity_lut():
    """A 32^3 identity table in the same layout, the Lut parameter's default (maps without a table switch it off)."""
    import struct
    import tempfile
    import zlib

    n = 32
    rows = []
    for g in range(n):
        row = bytearray(b"\x00")  # PNG filter type: none
        for b in range(n):
            for r in range(n):
                row += bytes((round(r * 255 / (n - 1)), round(g * 255 / (n - 1)), round(b * 255 / (n - 1)), 255))
        rows.append(bytes(row))
    chunk = lambda kind, data: (struct.pack(">I", len(data)) + kind + data
                                + struct.pack(">I", zlib.crc32(kind + data) & 0xFFFFFFFF))
    path = os.path.join(tempfile.mkdtemp(), "T_OFAST_IdentityLut.png")
    with open(path, "wb") as f:
        f.write(b"\x89PNG\r\n\x1a\n" + chunk(b"IHDR", struct.pack(">IIBBBBB", n * n, n, 8, 6, 0, 0, 0))
                + chunk(b"IDAT", zlib.compress(b"".join(rows))) + chunk(b"IEND", b""))
    asset = DEFAULT_TEXTURE_FOLDER + "/T_OFAST_IdentityLut"
    if not unreal.EditorAssetLibrary.does_asset_exist(asset):
        task = unreal.AssetImportTask()
        task.filename = path
        task.destination_path = DEFAULT_TEXTURE_FOLDER
        task.automated = True
        task.replace_existing = True
        task.save = False
        unreal.AssetToolsHelpers.get_asset_tools().import_asset_tasks([task])
    texture = unreal.EditorAssetLibrary.load_asset(asset)
    texture.set_editor_properties(LUT_TEXTURE_SETTINGS)
    unreal.EditorAssetLibrary.save_loaded_asset(texture)
    return texture


def ensure_tonemapper():
    """Post-process material that replaces Unreal's tone mapper with CS2's picture: scene colour x exposure (Unreal's
    eye adaptation value, the manual exposure) -> straight line or H(InputScale x) / H(WhiteScale W) per channel ->
    sRGB -> CS2's colour-correction table. Unreal applies nothing after a replacing tone mapper, so the sRGB encoding
    is done here."""
    if unreal.EditorAssetLibrary.does_asset_exist(TONEMAPPER):
        return unreal.EditorAssetLibrary.load_asset(TONEMAPPER)
    folder, name = TONEMAPPER.rsplit("/", 1)
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        name, folder, unreal.Material, unreal.MaterialFactoryNew())
    material.set_editor_property("material_domain", unreal.MaterialDomain.MD_POST_PROCESS)
    material.set_editor_property("blendable_location", unreal.BlendableLocation.BL_REPLACING_TONEMAPPER)
    mel = unreal.MaterialEditingLibrary
    scene = mel.create_material_expression(material, unreal.MaterialExpressionSceneTexture, -900, 0)
    scene.set_editor_property("scene_texture_id", unreal.SceneTextureId.PPI_POST_PROCESS_INPUT0)
    exposure = mel.create_material_expression(material, unreal.MaterialExpressionEyeAdaptation, -900, 200)
    lut = mel.create_material_expression(material, unreal.MaterialExpressionTextureObjectParameter, -900, 300)
    lut.set_editor_property("parameter_name", "Lut")
    lut.set_editor_property("texture", identity_lut())
    lut.set_editor_property("sampler_type", unreal.MaterialSamplerType.SAMPLERTYPE_LINEAR_COLOR)
    curve = mel.create_material_expression(material, unreal.MaterialExpressionCustom, -400, 0)
    curve.set_editor_property("description", "CS2 tone curve and colour table")
    curve.set_editor_property("output_type", unreal.CustomMaterialOutputType.CMOT_FLOAT3)
    curve.set_editor_property("code", TONEMAP_HLSL)
    scalars = list(TONEMAP_SCALES) + list(HABLE_PARAMETERS) + list(TONEMAP_SWITCHES)
    inputs = []
    for input_name in ["Color", "Exposure", "Lut"] + scalars:
        custom_input = unreal.CustomInput()
        custom_input.set_editor_property("input_name", input_name)
        inputs.append(custom_input)
    curve.set_editor_property("inputs", inputs)
    mel.connect_material_expressions(scene, "Color", curve, "Color")
    mel.connect_material_expressions(exposure, "", curve, "Exposure")
    mel.connect_material_expressions(lut, "", curve, "Lut")
    for index, param in enumerate(scalars):
        scalar = mel.create_material_expression(material, unreal.MaterialExpressionScalarParameter, -900,
                                                450 + index * 110)
        scalar.set_editor_property("parameter_name", param)
        scalar.set_editor_property("default_value", {**TONEMAP_SCALES, **TONEMAP_SWITCHES}.get(param, 1.0))
        mel.connect_material_expressions(scalar, "", curve, param)
    mel.connect_material_property(curve, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    mel.recompile_material(material)
    unreal.EditorAssetLibrary.save_asset(TONEMAPPER)
    log("created " + TONEMAPPER)
    return material


def cs2_tonemapper_instance(tonemap, hable, lut, lut_size):
    """An instance of the tone mapper with a map's .vpost curve parameters and colour table (None: no table)."""
    instance = blend_instance("Tonemap_" + NAME, ensure_tonemapper(), "/Game/OFAST/%s" % NAME)
    for param, value in TONEMAP_SCALES.items():
        set_scalar_parameter(instance, param, value)
    for param, key in HABLE_PARAMETERS.items():
        set_scalar_parameter(instance, param, tonemap.get(key, 1.0))
    set_scalar_parameter(instance, "Hable", 1.0 if hable else 0.0)
    set_scalar_parameter(instance, "UseLut", 1.0 if lut else 0.0)
    if lut:
        set_scalar_parameter(instance, "LutSize", lut_size)
        set_texture_parameter(instance, "Lut", lut)
    unreal.EditorAssetLibrary.save_loaded_asset(instance)
    return instance


LIGHT_CLASSES = {"point": unreal.PointLight, "spot": unreal.SpotLight, "rect": unreal.RectLight}


def setup_local_lights(manifest, imported, physical=False):
    """Replaces the glTF lights (punctual approximations, and Interchange drops their range) with Unreal lights
    built from the manifest: real rect lights, CS2's fade-out range, source size and bounce strength. `physical`
    (realistic style) uses Hammer's real candela and plain physical bounce instead of the CS2-matched values."""
    for actor in imported:
        if isinstance(actor, unreal.Light):
            actors.destroy_actor(actor)
    imported[:] = [a for a in imported if not isinstance(a, unreal.Light)]
    lights = manifest.get("lights") or []
    for light in lights:
        pitch, yaw, roll = light["rotation"]
        actor = spawn(LIGHT_CLASSES[light["shape"]], light["location"], unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw))
        actor.set_actor_label("%s_%s" % (light["class"], light["shape"]))
        component = actor.light_component
        component.set_mobility(unreal.ComponentMobility.MOVABLE)
        r, g, b = light["color"]
        component.set_light_color(unreal.LinearColor(r, g, b, 1.0))
        # Unreal's rect lights shine twice their candela value along their axis: RectLightSceneProxy.cpp sets the
        # luminance to intensity / (0.5 × width × height), point and spot lights have no such factor. Halving keeps
        # CS2's candela (measured on de_mirage's palace: rect lights came out ~2× too bright before).
        key = "intensity_cd_photometric" if physical else "intensity_cd"
        if key not in light:
            raise RuntimeError("the manifest has no %s: re-export the map with this version of OFAST" % key)
        candela = light[key] / (2.0 if light["shape"] == "rect" else 1.0)
        scale = light.get("indirect_intensity", 1.0)
        # Realistic: plain physical bounce (0 only when the light has none, or for the -Disable bounce diagnostic).
        bounce = (1.0 if scale else 0.0) if physical else scale * LOCAL_BOUNCE_GAIN
        settings = {
            "intensity_units": unreal.LightUnits.CANDELAS,
            "intensity": candela,
            "indirect_lighting_intensity": bounce,
            "cast_shadows": True,
        }
        if light.get("attenuation_radius"):
            settings["attenuation_radius"] = light["attenuation_radius"]
        if light["shape"] == "rect":
            # No barn doors: CS2's rect lights emit over the whole hemisphere in front of them.
            settings.update(
                {"source_width": light["source_width"], "source_height": light["source_height"], "barn_door_length": 0.0}
            )
        else:
            settings["source_radius"] = light.get("source_radius", 0.0)
        if light["shape"] == "spot":
            settings.update({"inner_cone_angle": light["inner_cone"], "outer_cone_angle": min(light["outer_cone"], 80.0)})
        component.set_editor_properties(settings)
    log("local lights: %d from the manifest" % len(lights))


DIAGNOSTIC_SWITCHES = {"sun", "sky", "local", "bounce"}


def diagnostic_manifest(manifest):
    """Applies OFAST_DISABLE (diagnostics only): a comma list of sun, sky, local, bounce. "sky" turns the ambient
    light off (the sky stays visible), "bounce" turns off the local lights' Lumen surface-cache bounce (screen traces
    still pick up some of their light)."""
    disabled = {s.strip() for s in os.environ.get("OFAST_DISABLE", "").split(",") if s.strip()}
    unknown = disabled - DIAGNOSTIC_SWITCHES
    if unknown:
        raise RuntimeError("unknown OFAST_DISABLE switch: %s" % ", ".join(sorted(unknown)))
    if not disabled:
        return manifest
    log("WARNING: diagnostic run without %s; the saved level keeps this lighting" % ", ".join(sorted(disabled)))
    manifest = dict(manifest, disabled=sorted(disabled))
    if "sun" in disabled and manifest.get("sun"):
        manifest["sun"] = dict(manifest["sun"], illuminance_lux=0.0)
    if "sky" in disabled:
        manifest["ambient"] = dict(manifest.get("ambient") or {}, sky_light_intensity=0.0)
        manifest["no_ambient"] = True
    if "local" in disabled:
        manifest["lights"] = []
    if "bounce" in disabled:
        manifest["lights"] = [dict(light, indirect_intensity=0.0) for light in manifest.get("lights") or []]
    return manifest


def setup_environment(manifest, imported):
    manifest = diagnostic_manifest(manifest)
    if STYLE == "realistic":
        setup_realistic(manifest, imported)
        return
    setup_sun(manifest)
    setup_sky(manifest)
    setup_local_lights(manifest, imported)
    if manifest.get("sun") and manifest.get("sky"):
        setup_post(manifest)
    else:
        # The fixed exposure is only right for CS2's own sun and sky; let Unreal's auto exposure cope otherwise.
        log("WARNING: exposure left to Unreal (no calibrated sun and sky)")


# "realistic" style: real-world light levels instead of CS2's. The sun is 100 000 lux before the atmosphere (about
# 80 000 on the ground once the atmosphere has dimmed and tinted it); the sky comes from Unreal's physical atmosphere;
# local lights use Hammer's photometric candela, which next to a real sun makes them far weaker than CS2 shows them
# (CS2's own sun is only 9.42 lux in its units) — interiors are then lit mostly by daylight. A camera-like auto
# exposure and filmic tone curve turn that range into a picture. CS2's sun direction and the map's lights are kept.
REALISTIC_SUN_LUX = 100000.0
REALISTIC_SUN_KELVIN = 5800.0
# Auto exposure range in EV100 (project setting ExtendDefaultLuminanceRange): from dim interiors to bright sun.
REALISTIC_EV100_RANGE = (2.0, 16.0)


def setup_realistic(manifest, imported):
    """Physically lit environment: atmosphere sky + real-time sky light, real sun, light haze, photometric local
    lights, auto exposure and Unreal's filmic tone curve."""
    disabled = set(manifest.get("disabled") or [])  # diagnostics (-Disable), see diagnostic_manifest
    sun = manifest.get("sun")
    pitch, yaw, roll = sun["rotation"] if sun else (-50.0, 30.0, 0.0)
    actor = spawn(unreal.DirectionalLight, rotation=unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw))
    component = actor.light_component
    component.set_mobility(unreal.ComponentMobility.MOVABLE)
    component.set_editor_properties(
        {
            "intensity": 0.0 if "sun" in disabled else REALISTIC_SUN_LUX,
            "use_temperature": True,
            "temperature": REALISTIC_SUN_KELVIN,
            # The atmosphere tints and dims the sun by itself (warmer near the horizon) and draws its disc.
            "atmosphere_sun_light": True,
            # The real sun is 0.53° across: crisp but not razor-sharp shadows.
            "light_source_angle": 0.5357,
            "cast_shadows": True,
            "lighting_channels": SUN_CHANNELS,
        }
    )

    spawn_physical_sky(0.0 if "sky" in disabled else 1.0)

    # A light desert haze; the atmosphere already adds aerial perspective to the distant 3D skybox.
    fog = spawn(unreal.ExponentialHeightFog).get_component_by_class(unreal.ExponentialHeightFogComponent)
    fog.set_editor_properties({"fog_density": 0.004, "fog_height_falloff": 0.2, "start_distance": 1000.0})

    setup_local_lights(manifest, imported, physical=True)

    volume, settings, override = spawn_post_volume()
    low, high = REALISTIC_EV100_RANGE
    override("auto_exposure_method", unreal.AutoExposureMethod.AEM_HISTOGRAM)
    override("auto_exposure_apply_physical_camera_exposure", False)
    override("auto_exposure_min_brightness", low)
    override("auto_exposure_max_brightness", high)
    override("auto_exposure_bias", 0.0)
    # Unreal's filmic tone curve stays on (no tone_curve_amount override); local exposure keeps detail in deep
    # shade and bright sky like a camera would, a little bloom and vignetting finish the lens look.
    override("local_exposure_highlight_contrast_scale", 0.8)
    override("local_exposure_shadow_contrast_scale", 0.8)
    override("bloom_intensity", 0.3)
    override("vignette_intensity", 0.25)
    volume.set_editor_property("settings", settings)
    log("realistic: sun %.0f lux, physical sky, photometric local lights, auto exposure EV100 %.0f-%.0f" % (REALISTIC_SUN_LUX, low, high))


# Label prefix of the screenshot cameras, saved with the level so tools/unreal/shoot_views.py can reuse them.
CAMERA_PREFIX = "OFAST_cam_"


def spawn_camera(name, location, rotation):
    camera = actors.spawn_actor_from_class(unreal.CameraActor, unreal.Vector(*location), rotation)
    camera.tags = [TAG]
    camera.set_actor_label(CAMERA_PREFIX + name)
    component = camera.camera_component
    component.set_editor_property("field_of_view", HFOV_16_9)
    component.set_editor_property("aspect_ratio", 16 / 9)
    component.set_editor_property("constrain_aspect_ratio", True)
    return camera


def extra_views():
    """(name, camera) pairs for the views listed in the OFAST_VIEWS JSON file, if set: a list of
    {"name": ..., "setpos": [x, y, z], "setang": [pitch, yaw, roll]} with the values typed in CS2's console
    (`setpos` is the player's feet; the eye is 64 units higher), to compare with CS2 screenshots of the same spots."""
    path = os.environ.get("OFAST_VIEWS")
    if not path:
        return []
    with open(path) as f:
        listed = json.load(f)
    result = []
    for view in listed:
        x, y, z = view["setpos"]
        pitch, yaw, roll = view["setang"]
        location = [x * 2.54, -y * 2.54, (z + 64.0) * 2.54]
        rotation = unreal.Rotator(roll=roll, pitch=-pitch, yaw=-yaw)
        result.append((view["name"], spawn_camera(view["name"], location, rotation)))
    return result


def views(manifest, imported):
    """(name, camera) pairs: first CT spawn, first T spawn, the OFAST_VIEWS ones, and a high overview of the map."""
    result = extra_views()
    for team in ("ct", "t"):
        spawns = manifest.get("spawns", {}).get(team) or []
        if spawns:
            pitch, yaw, roll = spawns[0]["rotation_unreal"]
            rotation = unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw)
            result.append((team + "0", spawn_camera(team + "0", spawns[0]["eye_unreal"], rotation)))
    # Overview: above the centre of the playable map (3D skybox backdrop left out), looking down at 60°.
    boxes = [
        a.get_actor_bounds(False)
        for a in imported
        if isinstance(a, unreal.StaticMeshActor) and not a.get_actor_label().startswith(SKYBOX_PREFIX)
    ]
    if boxes:
        lo = [min(o.x - e.x for o, e in boxes), min(o.y - e.y for o, e in boxes), min(o.z - e.z for o, e in boxes)]
        hi = [max(o.x + e.x for o, e in boxes), max(o.y + e.y for o, e in boxes), max(o.z + e.z for o, e in boxes)]
        centre = [(a + b) / 2 for a, b in zip(lo, hi)]
        size = max(hi[0] - lo[0], hi[1] - lo[1])
        log("map bounds (cm): min %s max %s" % ([round(v) for v in lo], [round(v) for v in hi]))
        location = [centre[0] - size * 0.45, centre[1], hi[2] + size * 0.55]
        rotation = unreal.MathLibrary.find_look_at_rotation(unreal.Vector(*location), unreal.Vector(*centre))
        result.append(("top", spawn_camera("top", location, rotation)))
    return result


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(MANIFEST) as f:
        manifest = json.load(f)
    prepare_level()
    # Two-layer materials glTF cannot describe: their instances are set up before the scene import, while no mesh in
    # the level uses them yet, then put on the imported meshes.
    blends = layer_blend_instances()
    blends.update(height_blend_instances())
    unreal.EditorAssetLibrary.save_directory("/Game/OFAST", only_if_is_dirty=True, recursive=True)
    imported = import_scene()
    if blends:
        log("blend materials: %d mesh slots swapped" % swap_material_slots(imported, blends))
        unreal.EditorAssetLibrary.save_directory("/Game/OFAST/" + NAME, only_if_is_dirty=True, recursive=True)
    # Sunlight must not leak into interiors: CS2's light blockers and two-sided shadows for single-sided walls.
    hide_light_blockers(imported)
    add_shadow_proxies(imported)
    # The 3D skybox stays behind the world and out of its lighting, as in CS2.
    setup_skybox(imported)
    setup_environment(manifest, imported)
    shots = views(manifest, imported)
    save_changed_instances()
    save_level()
    # Enough frames per screenshot for Lumen and temporal AA to settle at the screenshot resolution.
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")

    state = {"ticks": 0, "index": 0, "task": None, "started": time.monotonic()}

    def tick(_delta):
        state["ticks"] += 1
        # Wall-clock, not ticks: while shaders compile the editor ticks fast but a screenshot waits for them.
        if time.monotonic() - state["started"] > SHOT_TIMEOUT_S:
            log("FAILED: screenshots did not finish (%s took over %d min)"
                % (state.get("path", "the first screenshot"), SHOT_TIMEOUT_S // 60))
            finish()
            return
        if state["ticks"] < WARM_UP_TICKS:
            return  # let textures stream in (after 60 ticks de_mirage's first shot still had blurry floor tiles)
        task = state["task"]
        if task is not None and not task.is_task_done():
            return
        if task is not None:
            log("screenshot written: %s (%.0f s)" % (state["path"], time.monotonic() - state["started"]))
            state["index"] += 1
        if state["index"] >= len(shots):
            finish()
            return
        name, camera = shots[state["index"]]
        state["path"] = os.path.join(SHOT_DIR, "%s_%s.png" % (LEVEL, name))
        state["started"] = time.monotonic()
        state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(
            WIDTH, HEIGHT, state["path"], camera=camera, delay=SHOT_DELAY_S
        )

    def finish():
        unreal.unregister_slate_post_tick_callback(handle)
        unreal.SystemLibrary.quit_editor()

    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
