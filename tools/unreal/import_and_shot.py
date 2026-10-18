"""Import an OFAST glTF export into Unreal, frame it with a camera and save a screenshot — no clicks needed.

Run by tools/unreal/run_shot.ps1, which starts the editor like this:

    UnrealEditor-Cmd.exe unreal/Sandbox/OFASTSandbox.uproject -ExecutePythonScript=<this file> -unattended ...

Inputs come from environment variables (UE passes no arguments to the script):

    OFAST_GLTF   absolute path of the .gltf to import
    OFAST_SHOT   absolute path of the PNG screenshot to write
    OFAST_NAME   asset folder name under /Game/OFAST (default: file name of the glTF)

The editor stays alive while the frame renders (a Slate tick callback waits for the screenshot task), then quits.
Everything printed is prefixed with "OFAST:" so the runner can pick it out of the log.
"""

import os

import unreal

GLTF = os.environ["OFAST_GLTF"]
SHOT = os.environ["OFAST_SHOT"]
NAME = os.environ.get("OFAST_NAME") or os.path.splitext(os.path.basename(GLTF))[0]
WIDTH, HEIGHT = 1920, 1080


def log(message):
    unreal.log("OFAST: " + message)


def import_gltf():
    """Imports the glTF with Interchange and returns the mesh: a static mesh, or a skeletal mesh when the glTF has a
    skin (agents, weapons with bones; Interchange makes the Skeleton and a PhysicsAsset with it)."""
    source = unreal.InterchangeManager.create_source_data(GLTF)
    params = unreal.ImportAssetParameters()
    params.is_automated = True
    params.replace_existing = True
    manager = unreal.InterchangeManager.get_interchange_manager_scripted()
    objects = manager.import_asset("/Game/OFAST/" + NAME, source, params)
    log("imported %d assets: %s" % (len(objects), ", ".join(o.get_name() for o in objects)))
    meshes = [o for o in objects if isinstance(o, unreal.StaticMesh)]
    meshes += [o for o in objects if isinstance(o, unreal.SkeletalMesh)]
    if not meshes:
        raise RuntimeError("no mesh in the import")
    return meshes[0]


# Bones whose positions are logged for a skeletal mesh: with CS2's bind pose (A-pose facing +X) the pelvis sits
# 42.8 in (108.8 cm) and the head 70.8 in (179.8 cm) above the feet.
CHECK_BONES = ("root_motion", "pelvis", "head_0", "hand_R", "hand_L", "ankle_R")


def log_bones(actor):
    """Logs the skeleton's size and where a few bones are relative to the actor (cm, Unreal axes)."""
    component = actor.skeletal_mesh_component
    names = [str(n) for n in component.get_bone_names()] if hasattr(component, "get_bone_names") else []
    log("skeletal mesh: %d bones" % component.get_num_bones())
    base = actor.get_actor_location()
    for bone in CHECK_BONES:
        if names and bone not in names:
            continue
        p = component.get_socket_location(bone) - base
        log("bone %s at (%.1f, %.1f, %.1f) cm" % (bone, p.x, p.y, p.z))


def build_scene(mesh):
    """New basic level (sky, sun, floor), the mesh above the floor, and a camera looking at its side."""
    levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    level_path = "/Game/OFAST/Maps/" + NAME
    if unreal.EditorAssetLibrary.does_asset_exist(level_path):
        # Re-run: reopen the level and remove what the previous run added (tagged "OFAST").
        levels.load_level(level_path)
        for old in actors.get_all_level_actors():
            if "OFAST" in [str(tag) for tag in old.tags]:
                actors.destroy_actor(old)
    elif not levels.new_level_from_template(level_path, "/Engine/Maps/Templates/Template_Default"):
        raise RuntimeError("could not create " + level_path)

    actor = actors.spawn_actor_from_object(mesh, unreal.Vector(0, 0, 0))
    actor.tags = [unreal.Name("OFAST")]
    origin, extent = actor.get_actor_bounds(False)
    # Lift the mesh so it floats 30 cm above the floor.
    actor.set_actor_location(unreal.Vector(-origin.x, -origin.y, extent.z - origin.z + 30.0), False, False)
    origin, extent = actor.get_actor_bounds(False)
    log("mesh bounds: origin %s, half-size %s (cm)" % (origin, extent))
    if isinstance(actor, unreal.SkeletalMeshActor):
        log_bones(actor)

    # Look at the long side: the camera sits along the shorter horizontal axis. Far enough for the height too: the
    # 35 mm lens sees about ±16° vertically, so a standing agent (2 m) needs ~1.9 × its half-height per metre.
    distance = max(extent.x, extent.y, extent.z * 1.9) * 2.6
    if extent.x >= extent.y:
        offset = unreal.Vector(0, -distance, extent.z * 0.6)
    else:
        offset = unreal.Vector(distance, 0, extent.z * 0.6)
    location = origin + offset
    rotation = unreal.MathLibrary.find_look_at_rotation(location, origin)
    camera = actors.spawn_actor_from_class(unreal.CineCameraActor, location, rotation)
    camera.tags = [unreal.Name("OFAST")]
    camera.get_cine_camera_component().set_editor_property("current_focal_length", 35.0)
    # Keep everything sharp: focus distance = distance to the mesh.
    focus = camera.get_cine_camera_component().focus_settings
    focus.manual_focus_distance = (location - origin).length()
    camera.get_cine_camera_component().set_editor_property("focus_settings", focus)

    # Light the mesh from the camera side (the template's sun may sit behind it): same heading as the camera,
    # a little to the side, pointing 35° down.
    for light in actors.get_all_level_actors():
        if isinstance(light, unreal.DirectionalLight):
            light.set_actor_rotation(unreal.Rotator(roll=0.0, pitch=-35.0, yaw=rotation.yaw + 25.0), False)
    # Never save an engine level: the editor starts on /Engine/Maps/Templates/Template_Default, and saving it would
    # change the template every new level is made from.
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world().get_path_name()
    if not world.startswith("/Game/"):
        raise RuntimeError("refusing to save " + world)
    levels.save_current_level()
    return camera


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    mesh = import_gltf()
    camera = build_scene(mesh)
    state = {"ticks": 0, "task": None}

    def tick(_delta):
        state["ticks"] += 1
        # Safety net: never keep the editor open forever if the screenshot task stalls (~1 minute at 60 fps).
        if state["ticks"] > 3600:
            log("FAILED: screenshot did not finish")
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.SystemLibrary.quit_editor()
            return
        # Give the editor a few frames to create the viewport and stream in textures.
        if state["ticks"] == 60:
            log("taking screenshot")
            state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(
                WIDTH, HEIGHT, SHOT, camera=camera, delay=2.0
            )
        elif state["task"] is not None and state["task"].is_task_done():
            log("screenshot written: " + SHOT)
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.SystemLibrary.quit_editor()

    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:  # report clearly, then let the editor exit
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
