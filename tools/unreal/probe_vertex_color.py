"""Probe: does Unreal hand glTF vertex colours (COLOR_0) to materials as they are, or sRGB-encoded?

OFAST sends per-vertex layer blend weights as COLOR_0 (linear floats in the glTF). If Unreal stores vertex colours
as sRGB bytes and the material's VertexColor node returns those bytes, a weight of 0.5 arrives as 0.735 and every
blend material must decode it first.

The probe writes a small glTF (five quads whose COLOR_0 red is 0.2, 0.4, 0.5, 0.6 and 0.8), imports it with
Interchange, gives it an unlit material that is white where VertexColor.R > 0.5 and black elsewhere, and takes a
screenshot. Linear: 2 white quads (0.6, 0.8). sRGB-encoded: 4 (the encodings are 0.48, 0.66, 0.74, 0.80, 0.91).

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/probe_vertex_color.py -Shot test-data/shots/probe_vertex_color.png

The script prints how many quads came out white.
"""

import base64
import json
import os
import struct
import tempfile

import unreal

SHOT = os.environ["OFAST_SHOT"]
VALUES = [0.2, 0.4, 0.5, 0.6, 0.8]
FOLDER = "/Game/OFAST/Probe"


def log(message):
    unreal.log("OFAST: " + message)


def write_gltf():
    """Five 0.8 m quads in a row in the glTF XY plane (facing +Z), one COLOR_0 value each."""
    positions, colors, indices = [], [], []
    for i, value in enumerate(VALUES):
        x = i * 1.0
        base = len(positions)
        positions += [(x, 0.0, 0.0), (x + 0.8, 0.0, 0.0), (x + 0.8, 0.8, 0.0), (x, 0.8, 0.0)]
        colors += [(value, value, value, 1.0)] * 4
        indices += [base, base + 1, base + 2, base, base + 2, base + 3]
    blob = b"".join(struct.pack("<3f", *p) for p in positions)
    color_offset = len(blob)
    blob += b"".join(struct.pack("<4f", *c) for c in colors)
    index_offset = len(blob)
    blob += b"".join(struct.pack("<I", i) for i in indices)
    document = {
        "asset": {"version": "2.0"},
        "scene": 0,
        "scenes": [{"nodes": [0]}],
        "nodes": [{"mesh": 0, "name": "probe_vertex_color"}],
        "meshes": [{"name": "probe_vertex_color", "primitives": [
            {"attributes": {"POSITION": 0, "COLOR_0": 1}, "indices": 2, "mode": 4}]}],
        "accessors": [
            {"bufferView": 0, "componentType": 5126, "count": len(positions), "type": "VEC3",
             "min": [0.0, 0.0, 0.0], "max": [4.8, 0.8, 0.0]},
            {"bufferView": 1, "componentType": 5126, "count": len(colors), "type": "VEC4"},
            {"bufferView": 2, "componentType": 5125, "count": len(indices), "type": "SCALAR"},
        ],
        "bufferViews": [
            {"buffer": 0, "byteOffset": 0, "byteLength": color_offset},
            {"buffer": 0, "byteOffset": color_offset, "byteLength": index_offset - color_offset},
            {"buffer": 0, "byteOffset": index_offset, "byteLength": len(blob) - index_offset},
        ],
        "buffers": [{"byteLength": len(blob),
                     "uri": "data:application/octet-stream;base64," + base64.b64encode(blob).decode()}],
    }
    path = os.path.join(tempfile.mkdtemp(), "probe_vertex_color.gltf")
    with open(path, "w") as f:
        json.dump(document, f)
    return path


def threshold_material():
    """Unlit, two-sided: emissive 1 where VertexColor.R > 0.5, else 0."""
    path = FOLDER + "/M_ProbeVertexColor"
    if unreal.EditorAssetLibrary.does_asset_exist(path):
        unreal.EditorAssetLibrary.delete_asset(path)
    mel = unreal.MaterialEditingLibrary
    material = unreal.AssetToolsHelpers.get_asset_tools().create_asset(
        "M_ProbeVertexColor", FOLDER, unreal.Material, unreal.MaterialFactoryNew())
    material.set_editor_property("shading_model", unreal.MaterialShadingModel.MSM_UNLIT)
    material.set_editor_property("two_sided", True)
    vertex = mel.create_material_expression(material, unreal.MaterialExpressionVertexColor, -600, 0)
    half = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -600, 200)
    half.set_editor_property("r", 0.5)
    one = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -600, 300)
    one.set_editor_property("r", 1.0)
    zero = mel.create_material_expression(material, unreal.MaterialExpressionConstant, -600, 400)
    zero.set_editor_property("r", 0.0)
    test = mel.create_material_expression(material, unreal.MaterialExpressionIf, -300, 0)
    mel.connect_material_expressions(vertex, "R", test, "A")
    mel.connect_material_expressions(half, "", test, "B")
    mel.connect_material_expressions(one, "", test, "A > B")
    mel.connect_material_expressions(zero, "", test, "A == B")
    mel.connect_material_expressions(zero, "", test, "A < B")
    mel.connect_material_property(test, "", unreal.MaterialProperty.MP_EMISSIVE_COLOR)
    mel.recompile_material(material)
    return material


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    levels.new_level(FOLDER + "/ProbeVertexColor")

    source = unreal.InterchangeManager.create_source_data(write_gltf())
    params = unreal.ImportAssetParameters()
    params.is_automated = True
    params.replace_existing = True
    objects = unreal.InterchangeManager.get_interchange_manager_scripted().import_asset(FOLDER, source, params)
    mesh = next(o for o in objects if isinstance(o, unreal.StaticMesh))
    actor = actors.spawn_actor_from_object(mesh, unreal.Vector(0, 0, 0))
    actor.static_mesh_component.set_material(0, threshold_material())
    origin, extent = actor.get_actor_bounds(False)
    log("quads: origin %s, half-size %s" % (origin, extent))
    # The quads face +Y in Unreal (glTF +Z); look at them from there.
    location = origin + unreal.Vector(0, max(extent.x, extent.z) * 3.0, 0)
    camera = actors.spawn_actor_from_class(
        unreal.CameraActor, location, unreal.MathLibrary.find_look_at_rotation(location, origin))
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 8")
    state = {"ticks": 0, "task": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] == 120:
            state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(1280, 400, SHOT, camera=camera,
                                                                               delay=1.0)
        elif state["task"] is not None and state["task"].is_task_done():
            log("screenshot written: " + SHOT)
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.SystemLibrary.quit_editor()
        elif state["ticks"] > 20000:
            log("FAILED: screenshot did not finish")
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.SystemLibrary.quit_editor()

    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
