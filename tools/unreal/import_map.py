"""Import an OFAST map export into Unreal and take screenshots from known viewpoints — no clicks needed.

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/import_map.py -Gltf <map>.gltf -Shot <folder> -Name <map>).

Inputs (environment variables):
    OFAST_GLTF   absolute path of <map>.gltf (the <map>.ofast.json manifest must sit next to it)
    OFAST_SHOT   folder for screenshots: <map>_ct0.png, <map>_t0.png, <map>_top.png
    OFAST_NAME   map name (asset folder /Game/OFAST/<name>, level /Game/OFAST/Maps/<name>)

Steps: fresh level from the Basic template (sky, sun, fog) → Interchange *scene* import of the glTF (one actor per
placement, lights included) → sun rotated like CS2's light_environment → camera at a spawn's eye position with
CS2's default field of view → screenshots → quit.
"""

import json
import math
import os

import unreal

GLTF = os.environ["OFAST_GLTF"]
SHOT_DIR = os.environ["OFAST_SHOT"]
if SHOT_DIR.lower().endswith(".png"):  # a file path was given: use its folder
    SHOT_DIR = os.path.dirname(SHOT_DIR)
NAME = os.environ.get("OFAST_NAME") or os.path.splitext(os.path.basename(GLTF))[0]
MANIFEST = os.path.join(os.path.dirname(GLTF), NAME + ".ofast.json")
WIDTH, HEIGHT = 1920, 1080
# CS2's default fov is 90° measured horizontally on a 4:3 screen; on 16:9 that is 2·atan(tan(45°)·(16/9)/(4/3)).
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))
TAG = unreal.Name("OFAST")


def log(message):
    unreal.log("OFAST: " + message)


levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)


def prepare_level():
    """Opens /Game/OFAST/Maps/<name> without previous OFAST actors (creates it from the Basic template once)."""
    level_path = "/Game/OFAST/Maps/" + NAME
    if unreal.EditorAssetLibrary.does_asset_exist(level_path):
        levels.load_level(level_path)
        old = [a for a in actors.get_all_level_actors() if TAG in a.tags]
        for actor in old:
            actors.destroy_actor(actor)
        log("reused level, removed %d old actors" % len(old))
    else:
        levels.new_level_from_template(level_path, "/Engine/Maps/Templates/Template_Default")
    # The template's floor would cut through the map.
    for actor in actors.get_all_level_actors():
        if isinstance(actor, unreal.StaticMeshActor) and TAG not in actor.tags and "Floor" in actor.get_actor_label():
            actors.destroy_actor(actor)


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


# Versioned name: bump when the graph changes so existing projects get the new master.
BLEND_MASTER = "/Game/OFAST/Materials/M_OFAST_LayerBlend_v2"
EXPORT_DIR = os.path.dirname(GLTF)


def import_texture(relative_png, folder, kind):
    """Imports one exported PNG as a texture asset. kind: 'color' (sRGB), 'normal', or 'linear' (masks)."""
    name = os.path.splitext(os.path.basename(relative_png))[0]
    path = folder + "/" + name
    if unreal.EditorAssetLibrary.does_asset_exist(path):
        return unreal.EditorAssetLibrary.load_asset(path)
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
    if kind == "normal":
        texture.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_NORMALMAP)
        texture.set_editor_property("srgb", False)
    elif kind == "linear":
        texture.set_editor_property("compression_settings", unreal.TextureCompressionSettings.TC_MASKS)
        texture.set_editor_property("srgb", False)
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


def apply_layer_blends(imported):
    """Replaces the glTF stand-in material (layer 1 only) of every two-layer CS2 material by a blend instance."""
    with open(GLTF) as f:
        document = json.load(f)
    layered = {}
    for material in document.get("materials", []):
        layers = material.get("extras", {}).get("ofast", {}).get("layers")
        if layers and layers["layer1"].get("color") and layers["layer2"].get("color"):
            layered[material["name"]] = layers
    if not layered:
        return
    folder = "/Game/OFAST/%s/Blend" % NAME
    tools = unreal.AssetToolsHelpers.get_asset_tools()
    mel = unreal.MaterialEditingLibrary
    instances = {}
    master = None
    for name, layers in layered.items():
        textures = {
            "c1": import_texture(layers["layer1"]["color"], folder, "color"),
            "c2": import_texture(layers["layer2"]["color"], folder, "color"),
            "n1": import_texture(layers["layer1"]["normal"], folder, "normal"),
            "n2": import_texture(layers["layer2"]["normal"], folder, "normal"),
            "r1": import_texture(layers["layer1"]["roughness"], folder, "linear"),
            "r2": import_texture(layers["layer2"]["roughness"], folder, "linear"),
            "mod": import_texture(layers["modulation"], folder, "linear") if layers.get("modulation") else None,
        }
        if textures["mod"] is None:
            textures["mod"] = textures["r1"]
        master = master or ensure_blend_master(textures)
        instance_path = folder + "/MI_" + name
        if unreal.EditorAssetLibrary.does_asset_exist(instance_path):
            instance = unreal.EditorAssetLibrary.load_asset(instance_path)
        else:
            instance = tools.create_asset(
                "MI_" + name, folder, unreal.MaterialInstanceConstant, unreal.MaterialInstanceConstantFactoryNew()
            )
        mel.set_material_instance_parent(instance, master)
        for param, key in (("Layer1Color", "c1"), ("Layer2Color", "c2"), ("Layer1Normal", "n1"),
                           ("Layer2Normal", "n2"), ("Layer1Rough", "r1"), ("Layer2Rough", "r2"),
                           ("BlendModulation", "mod")):
            mel.set_material_instance_texture_parameter_value(instance, param, textures[key])
        if layers.get("softness") is not None:
            mel.set_material_instance_scalar_parameter_value(instance, "BlendSoftness", float(layers["softness"]))
        mode = int(layers.get("fancy_blending") or 0)
        has_mask = layers.get("modulation") is not None
        mel.set_material_instance_scalar_parameter_value(instance, "UseModulation", 1.0 if (mode > 0 and has_mask) else 0.0)
        mel.set_material_instance_scalar_parameter_value(instance, "UseAlphaMask", 1.0 if mode == 3 else 0.0)
        mel.set_material_instance_scalar_parameter_value(instance, "UseParamSoftness", 1.0 if mode in (2, 3) else 0.0)
        instances[name] = instance

    # Swap materials on the imported static meshes (shared by every actor that uses them).
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
    log("layer blend: %d materials, %d mesh slots swapped" % (len(instances), swapped))
    # Save the blend material, its instances, the imported layer textures and the swapped meshes.
    unreal.EditorAssetLibrary.save_directory("/Game/OFAST", only_if_is_dirty=True, recursive=True)


def setup_sun(manifest):
    sun = manifest.get("sun")
    if not sun:
        return
    pitch, yaw, roll = sun["rotation"]
    for actor in actors.get_all_level_actors():
        if isinstance(actor, unreal.DirectionalLight):
            actor.set_actor_rotation(unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw), False)
            color = sun.get("color") or [1, 1, 1]
            actor.light_component.set_light_color(unreal.LinearColor(color[0], color[1], color[2], 1.0))
            log("sun rotation pitch %.1f yaw %.1f" % (pitch, yaw))


def spawn_camera(location, rotation):
    camera = actors.spawn_actor_from_class(unreal.CameraActor, unreal.Vector(*location), rotation)
    camera.tags = [TAG]
    component = camera.camera_component
    component.set_editor_property("field_of_view", HFOV_16_9)
    component.set_editor_property("aspect_ratio", 16 / 9)
    component.set_editor_property("constrain_aspect_ratio", True)
    return camera


def views(manifest, imported):
    """(name, camera) pairs: first CT spawn, first T spawn, and a high overview of the whole map."""
    result = []
    for team in ("ct", "t"):
        spawns = manifest.get("spawns", {}).get(team) or []
        if spawns:
            pitch, yaw, roll = spawns[0]["rotation_unreal"]
            rotation = unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw)
            result.append((team + "0", spawn_camera(spawns[0]["eye_unreal"], rotation)))
    # Overview: above the centre of everything imported, looking down at 60°.
    boxes = [a.get_actor_bounds(False) for a in imported if isinstance(a, unreal.StaticMeshActor)]
    if boxes:
        lo = [min(o.x - e.x for o, e in boxes), min(o.y - e.y for o, e in boxes), min(o.z - e.z for o, e in boxes)]
        hi = [max(o.x + e.x for o, e in boxes), max(o.y + e.y for o, e in boxes), max(o.z + e.z for o, e in boxes)]
        centre = [(a + b) / 2 for a, b in zip(lo, hi)]
        size = max(hi[0] - lo[0], hi[1] - lo[1])
        log("map bounds (cm): min %s max %s" % ([round(v) for v in lo], [round(v) for v in hi]))
        location = [centre[0] - size * 0.45, centre[1], hi[2] + size * 0.55]
        rotation = unreal.MathLibrary.find_look_at_rotation(unreal.Vector(*location), unreal.Vector(*centre))
        result.append(("top", spawn_camera(location, rotation)))
    return result


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(MANIFEST) as f:
        manifest = json.load(f)
    prepare_level()
    imported = import_scene()
    apply_layer_blends(imported)
    setup_sun(manifest)
    shots = views(manifest, imported)
    levels.save_current_level()

    state = {"ticks": 0, "index": 0, "task": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] > 36000:  # ~10 minutes at 60 fps: give up rather than hang
            log("FAILED: screenshots did not finish")
            finish()
            return
        if state["ticks"] < 60:
            return  # let the viewport come up
        task = state["task"]
        if task is not None and not task.is_task_done():
            return
        if task is not None:
            log("screenshot written: " + state["path"])
            state["index"] += 1
        if state["index"] >= len(shots):
            finish()
            return
        name, camera = shots[state["index"]]
        state["path"] = os.path.join(SHOT_DIR, "%s_%s.png" % (NAME, name))
        state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(
            WIDTH, HEIGHT, state["path"], camera=camera, delay=2.0
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
