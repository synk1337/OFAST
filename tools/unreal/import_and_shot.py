"""Import an OFAST glTF export into Unreal, frame it with a camera and save a screenshot — no clicks needed.

Run by tools/unreal/run_shot.ps1, which starts the editor like this:

    UnrealEditor-Cmd.exe unreal/Sandbox/OFASTSandbox.uproject -ExecutePythonScript=<this file> -unattended ...

Inputs come from environment variables (UE passes no arguments to the script):

    OFAST_GLTF        absolute path of the .gltf to import
    OFAST_SHOT        absolute path of the PNG screenshot to write
    OFAST_NAME        asset folder name under /Game/OFAST (default: file name of the glTF)
    OFAST_ANIM        optional: part of the name of an imported animation (a glTF from `ofast export-model --clip`)
                      to pose the skeletal mesh with
    OFAST_ANIM_TIMES  optional: comma-separated times in seconds (default "0"); one screenshot per time, named
                      <shot>_<time>.png, and the CHECK_BONES positions logged for each
    OFAST_CAMERA      optional: "x,y,z,pitch,yaw,fov" — a camera at (x, y, z) cm from the mesh's origin (Unreal axes;
                      an agent faces +X), turned by pitch and yaw (degrees) with a horizontal field of view of fov
                      degrees, instead of the automatic framing: to look at the mesh from where a CS2 player saw it
                      (CS2's 90° at 4:3 is 106.26° at 16:9)

The editor stays alive while the frames render (a Slate tick callback waits for each screenshot task), then quits.
Everything printed is prefixed with "OFAST:" so the runner can pick it out of the log.
"""

import os

import unreal

GLTF = os.environ["OFAST_GLTF"]
SHOT = os.environ["OFAST_SHOT"]
NAME = os.environ.get("OFAST_NAME") or os.path.splitext(os.path.basename(GLTF))[0]
ANIM = os.environ.get("OFAST_ANIM", "")
ANIM_TIMES = [float(t) for t in (os.environ.get("OFAST_ANIM_TIMES") or "0").split(",") if t.strip()]
CAMERA = [float(v) for v in (os.environ.get("OFAST_CAMERA") or "").split(",") if v.strip()]
WIDTH, HEIGHT = 1920, 1080


def log(message):
    unreal.log("OFAST: " + message)


def import_gltf():
    """Imports the glTF with Interchange and returns the mesh and its animations: a static mesh, or a skeletal mesh
    when the glTF has a skin (agents, weapons with bones; Interchange makes the Skeleton and a PhysicsAsset with it),
    plus one AnimSequence per glTF animation."""
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
    animations = [o for o in objects if isinstance(o, unreal.AnimSequence)]
    for animation in animations:
        log_animation(animation)
    return meshes[0], animations


def log_animation(animation):
    """Logs an AnimSequence's length, frame count and frame rate: the export keys every clip at 30 fps, so Unreal
    should read 30 fps and (keys − 1) frames."""
    length = animation.get_play_length()
    frames = unreal.AnimationLibrary.get_num_frames(animation)
    fps = frames / length if length > 0 else 0.0
    log("animation %s: %.4f s, %d frames, %.2f fps" % (animation.get_name(), length, frames, fps))


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

    if len(CAMERA) == 6:
        camera = place_camera(actors, actor.get_actor_location())
    else:
        camera = frame_camera(actors, origin, extent)

    # Light the mesh from the camera side (the template's sun may sit behind it): same heading as the camera,
    # a little to the side, pointing 35° down.
    for light in actors.get_all_level_actors():
        if isinstance(light, unreal.DirectionalLight):
            yaw = camera.get_actor_rotation().yaw
            light.set_actor_rotation(unreal.Rotator(roll=0.0, pitch=-35.0, yaw=yaw + 25.0), False)
    # Never save an engine level: the editor starts on /Engine/Maps/Templates/Template_Default, and saving it would
    # change the template every new level is made from.
    world = unreal.get_editor_subsystem(unreal.UnrealEditorSubsystem).get_editor_world().get_path_name()
    if not world.startswith("/Game/"):
        raise RuntimeError("refusing to save " + world)
    levels.save_current_level()
    return actor, camera


def place_camera(actors, base):
    """A camera at OFAST_CAMERA's offset from `base` (the mesh actor's location), with its angles and horizontal
    field of view, 16:9."""
    x, y, z, pitch, yaw, fov = CAMERA
    location = base + unreal.Vector(x, y, z)
    camera = actors.spawn_actor_from_class(
        unreal.CameraActor, location, unreal.Rotator(roll=0.0, pitch=pitch, yaw=yaw)
    )
    camera.tags = [unreal.Name("OFAST")]
    component = camera.camera_component
    component.set_editor_property("field_of_view", fov)
    component.set_editor_property("aspect_ratio", WIDTH / float(HEIGHT))
    component.set_editor_property("constrain_aspect_ratio", True)
    log("camera at %s, pitch %.1f, yaw %.1f, horizontal field of view %.2f" % (location, pitch, yaw, fov))
    return camera


def frame_camera(actors, origin, extent):
    """A 35 mm cine camera looking at the mesh's bounds from the side."""
    # Look at the long side: the camera sits along the shorter horizontal axis. Far enough for the height too: the
    # 35 mm lens sees about ±16° vertically, so a standing agent (2 m) needs ~1.9 × its half-height per metre.
    # An animated agent is seen from its left (CS2's agents face +X), where a stride shows best; its bind-pose arms
    # would otherwise make the front the long side.
    distance = max(extent.x, extent.y, extent.z * 1.9) * 2.6
    if ANIM or extent.x >= extent.y:
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
    return camera


def pose(actor, animation, time):
    """Poses a skeletal mesh actor with `animation` held at `time` seconds (single-node mode, not playing).

    Overriding the animation data again in single-node mode keeps the first pose: the saved position only reaches the
    single-node instance when the mode changes. So the mode goes through custom mode first, and the position is set
    on the instance as well."""
    component = actor.skeletal_mesh_component
    component.set_update_animation_in_editor(True)
    component.set_animation_mode(unreal.AnimationMode.ANIMATION_CUSTOM_MODE)
    component.override_animation_data(animation, False, False, time, 0.0)
    component.set_position(time, False)


def shot_path(time):
    """The screenshot for one time of the animation: <shot>_<time>.png (the plain shot without an animation)."""
    if not ANIM:
        return SHOT
    base, extension = os.path.splitext(SHOT)
    return "%s_%.2f%s" % (base, time, extension)


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    mesh, animations = import_gltf()
    actor, camera = build_scene(mesh)
    animation = None
    if ANIM:
        matches = [a for a in animations if ANIM.lower() in a.get_name().lower()]
        if not matches or not isinstance(actor, unreal.SkeletalMeshActor):
            raise RuntimeError("no animation named like %r on a skeletal mesh" % ANIM)
        animation = matches[0]
    # One screenshot per animation time (or a single one without an animation). Each goes through three steps:
    # "pose" sets the time, "settle" waits a few frames for the pose to reach the render, "shoot" waits for the
    # screenshot task.
    times = list(ANIM_TIMES) if animation else [0.0]
    total = len(times)
    state = {"ticks": 0, "step": "pose", "since": 0, "time": 0.0, "task": None}

    def finish(message):
        log(message)
        unreal.unregister_slate_post_tick_callback(handle)
        unreal.SystemLibrary.quit_editor()

    def tick(_delta):
        state["ticks"] += 1
        # Safety net: never keep the editor open forever if a screenshot task stalls (~1 minute per shot at 60 fps).
        if state["ticks"] > 3600 * total:
            finish("FAILED: screenshot did not finish")
            return
        # Give the editor a few frames to create the viewport and stream in textures.
        if state["ticks"] < 60:
            return
        state["since"] += 1
        if state["step"] == "pose":
            state["time"] = times.pop(0)
            if animation is not None:
                pose(actor, animation, state["time"])
            state["step"], state["since"] = "settle", 0
        elif state["step"] == "settle" and state["since"] >= 20:
            if animation is not None:
                log("%s at %.3f s:" % (animation.get_name(), state["time"]))
                log_bones(actor)
            log("taking screenshot")
            # Next step first: starting the screenshot ticks Slate, which calls this function again.
            state["step"], state["since"] = "shoot", 0
            state["task"] = unreal.AutomationLibrary.take_high_res_screenshot(
                WIDTH, HEIGHT, shot_path(state["time"]), camera=camera, delay=2.0
            )
        elif state["step"] == "shoot" and state["task"] is not None and state["task"].is_task_done():
            log("screenshot written: " + shot_path(state["time"]))
            if not times:
                finish("done")
                return
            state["step"], state["since"] = "pose", 0

    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:  # report clearly, then let the editor exit
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
