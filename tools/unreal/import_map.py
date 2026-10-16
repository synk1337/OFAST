"""Import an OFAST map export into Unreal and take screenshots from known viewpoints — no clicks needed.

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/import_map.py -Gltf <map>.gltf -Shot <folder> -Name <map>).

Inputs (environment variables):
    OFAST_GLTF   absolute path of <map>.gltf (the <map>.ofast.json manifest must sit next to it)
    OFAST_SHOT   folder for screenshots: <map>_ct0.png, <map>_t0.png, <map>_top.png
    OFAST_NAME   map name (asset folder /Game/OFAST/<name>, level /Game/OFAST/Maps/<name>)
    OFAST_VIEWS  optional JSON file of extra viewpoints given as CS2 setpos/setang (see extra_views)

Steps: level made once from the Basic template, its environment (sky, clouds, fog, sun, floor) removed →
Interchange *scene* import of the glTF (one actor per placement) → layer-blend materials → CS2's environment from the
manifest: sun, sky sphere + Sky Light from the exported HDR, local lights with their real shapes and ranges, fixed
exposure with no tone curve (docs/formats/lighting.md) → cameras at the spawns' eyes (and OFAST_VIEWS) with CS2's
default field of view → screenshots → quit. Everything the script adds is tagged "OFAST" and rebuilt on every run.
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
# Actor labels of 3D-skybox geometry start with this (SKYBOX_NODE_PREFIX in ofast-core's map exporter).
SKYBOX_PREFIX = "sky3d_"


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


def setup_fallback_sky():
    """Unreal's physical sky (atmosphere + real-time sky light), for exports without a CS2 sky."""
    spawn(unreal.SkyAtmosphere)
    light = spawn(unreal.SkyLight)
    light.light_component.set_mobility(unreal.ComponentMobility.MOVABLE)
    light.light_component.set_editor_property("real_time_capture", True)
    log("WARNING: no sky in the manifest; using Unreal's physical sky (not calibrated against CS2)")


def setup_sky(manifest):
    """Visible sky (a big sphere with the CS2 sky) and the Sky Light: lit by the same cube map, or by the flat sky
    colour's cube map when the map's ambient comes from `skycolor` (manifest ambient.hdr)."""
    sky = manifest.get("sky")
    if not sky:
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
            "indirect_lighting_intensity": ambient.get("indirect_intensity", 1.0),
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
        spawn(unreal.DirectionalLight, rotation=unreal.Rotator(roll=0.0, pitch=-50.0, yaw=30.0))
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
            "indirect_lighting_intensity": sun.get("indirect_intensity", 1.0),
            "atmosphere_sun_light": False,
            "use_temperature": False,
            "cast_shadows": True,
        }
    )
    log("sun: pitch %.1f yaw %.1f, %.2f lux" % (pitch, yaw, sun["illuminance_lux"]))


def setup_post(manifest):
    """Unbound post-process volume reproducing CS2's picture: fixed exposure (EV100 from the exporter, measured
    against CS2 screenshots), no tone curve (CS2's is a straight line), no bloom, no vignette."""
    post = manifest.get("post") or {}
    # Sunlit views sit at CS2's minimum exposure, the highest EV100.
    ev100 = post.get("ev100_max", 0.0)
    volume = spawn(unreal.PostProcessVolume)
    volume.set_actor_label("OFAST_PostProcess")
    volume.set_editor_properties({"unbound": True, "priority": 10.0, "blend_weight": 1.0, "enabled": True})
    settings = volume.settings

    def override(name, value):
        settings.set_editor_property("override_" + name, True)
        settings.set_editor_property(name, value)

    override("auto_exposure_method", unreal.AutoExposureMethod.AEM_MANUAL)
    override("auto_exposure_apply_physical_camera_exposure", False)
    # Manual exposure without the physical camera: picture = luminance × 2^bias, so bias = −EV100.
    override("auto_exposure_bias", -ev100)
    override("tone_curve_amount", 0.0)
    override("expand_gamut", 0.0)
    override("blue_correction", 0.0)
    override("bloom_intensity", 0.0)
    override("vignette_intensity", 0.0)
    volume.set_editor_property("settings", settings)
    log("post: manual exposure EV100 %.3f, tone curve off" % ev100)


LIGHT_CLASSES = {"point": unreal.PointLight, "spot": unreal.SpotLight, "rect": unreal.RectLight}


def setup_local_lights(manifest, imported):
    """Replaces the glTF lights (punctual approximations, and Interchange drops their range) with Unreal lights
    built from the manifest: real rect lights, CS2's fade-out range, source size and bounce strength."""
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
        settings = {
            "intensity_units": unreal.LightUnits.CANDELAS,
            "intensity": light["intensity_cd"],
            "indirect_lighting_intensity": light.get("indirect_intensity", 1.0),
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


def setup_environment(manifest, imported):
    setup_sun(manifest)
    setup_sky(manifest)
    setup_local_lights(manifest, imported)
    if manifest.get("sun") and manifest.get("sky"):
        setup_post(manifest)
    else:
        # The fixed exposure is only right for CS2's own sun and sky; let Unreal's auto exposure cope otherwise.
        log("WARNING: exposure left to Unreal (no calibrated sun and sky)")


def spawn_camera(location, rotation):
    camera = actors.spawn_actor_from_class(unreal.CameraActor, unreal.Vector(*location), rotation)
    camera.tags = [TAG]
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
        result.append((view["name"], spawn_camera(location, unreal.Rotator(roll=roll, pitch=-pitch, yaw=-yaw))))
    return result


def views(manifest, imported):
    """(name, camera) pairs: first CT spawn, first T spawn, the OFAST_VIEWS ones, and a high overview of the map."""
    result = extra_views()
    for team in ("ct", "t"):
        spawns = manifest.get("spawns", {}).get(team) or []
        if spawns:
            pitch, yaw, roll = spawns[0]["rotation_unreal"]
            rotation = unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw)
            result.append((team + "0", spawn_camera(spawns[0]["eye_unreal"], rotation)))
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
        result.append(("top", spawn_camera(location, rotation)))
    return result


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    with open(MANIFEST) as f:
        manifest = json.load(f)
    prepare_level()
    imported = import_scene()
    apply_layer_blends(imported)
    setup_environment(manifest, imported)
    shots = views(manifest, imported)
    save_level()
    # Enough frames per screenshot for Lumen and temporal AA to settle at the screenshot resolution.
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")

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
