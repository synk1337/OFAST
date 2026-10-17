"""Re-shoot the views of an already imported map without importing it again (about a minute instead of three).

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/shoot_views.py -Name <map> -Shot <folder>).

Inputs (environment variables):
    OFAST_NAME   level name: opens /Game/OFAST/Maps/<name>, made by import_map.py (<map>, or <map>_realistic for
                 the realistic style)
    OFAST_SHOT   folder for the screenshots: <map>_<view>.png
    OFAST_ONLY   optional comma list of view names (ct0, t0, top, or names from the -Views file of the import)
    OFAST_VIEWS  optional JSON file of viewpoints given as CS2 setpos/setang, as for import_map.py: cameras for them
                 are added for this run only (a name the level already has is replaced), so new views can be tried
                 without importing again
    OFAST_TRY   optional JSON (or a .json file path) of what-if settings applied before shooting (never saved), to
                 try changes quickly:
                   "sun", "sky_light", "local": light component properties, e.g. {"indirect_lighting_intensity": 2}
                                                ("name*": multiply the current value)
                   "sun_rotation": [pitch, yaw, roll] of the sun (Unreal degrees)
                   "meshes": properties of the imported map meshes' components (not the sky sphere, proxies or blockers)
                   "blockers": properties of CS2's light blockers (e.g. {"cast_shadow": false})
                   "proxies": properties of the shadow proxies
                   (in these three groups, "visible": false hides and "material": "/Game/..." replaces every slot)
                   "post": PostProcessSettings fields (set with their override flag)
                   "console": console commands run before shooting, e.g. ["ShowFlag.Lighting 0"] (base colour only,
                              to compare with CS2's mat_fullbright 1)
                   "materials": material asset path -> properties of that material, recompiled in memory (shader
                                compilation can take minutes); an enum property takes the value's name, e.g.
                                {"/InterchangeAssets/gltf/M_Default":
                                 {"translucency_lighting_mode": "TLM_SURFACE_PER_PIXEL_LIGHTING"}}
                   "script": a Python file run last, for what-ifs the groups above cannot express (e.g. building a
                             test material and putting it on some slots); it sees `unreal`, `actors` (the level's
                             actors) and `log`, and must not save anything

It uses the cameras import_map.py saved with the level (labels OFAST_cam_<view>), so the viewpoints are exactly
the import's, plus the OFAST_VIEWS ones. The level is not changed or saved; its lighting is whatever the last import
saved (including a diagnostic -Disable run).
"""

import json
import math
import os
import time

import unreal

NAME = os.environ["OFAST_NAME"]
SHOT_DIR = os.environ["OFAST_SHOT"]
ONLY = {s.strip() for s in os.environ.get("OFAST_ONLY", "").split(",") if s.strip()}
WIDTH, HEIGHT = 1920, 1080
CAMERA_PREFIX = "OFAST_cam_"  # import_map.CAMERA_PREFIX
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))  # import_map.HFOV_16_9
# Frames to wait after opening the level, so meshes and textures have streamed in before the first screenshot.
WARM_UP_TICKS = 600
SHOT_TIMEOUT_S = 45 * 60  # import_map.SHOT_TIMEOUT_S: longest wait for one screenshot (shader compilation)
SHOT_DELAY_S = 6.0  # import_map.SHOT_DELAY_S: seconds on each camera before its screenshot (Lumen settling)


def log(message):
    unreal.log("OFAST: " + message)


def apply_try(actors):
    """Applies the OFAST_TRY what-if settings to the loaded level (in memory only)."""
    raw = os.environ.get("OFAST_TRY") or "{}"
    if raw.endswith(".json") and os.path.isfile(raw):
        with open(raw) as f:
            raw = f.read()
    settings = json.loads(raw)
    if not settings:
        return
    log("what-if settings: %s" % json.dumps(settings))
    groups = {
        "sun": (unreal.DirectionalLight,),
        "sky_light": (unreal.SkyLight,),
        "local": (unreal.PointLight, unreal.SpotLight, unreal.RectLight),
    }
    unknown = set(settings) - set(groups) - {"post", "sun_rotation", "meshes", "blockers", "proxies", "console",
                                             "materials", "script"}
    if unknown:
        raise RuntimeError(
            "unknown OFAST_TRY group: %s (sun, sun_rotation, sky_light, local, meshes, blockers, proxies, post, console,"
            " materials, script)" % ", ".join(sorted(unknown))
        )
    for command in settings.get("console", []):
        unreal.SystemLibrary.execute_console_command(None, command)
    # "materials": properties of material assets (masters, e.g. a translucency lighting mode, which instances cannot
    # override), changed in memory and recompiled; the assets are never saved.
    for path, props in settings.get("materials", {}).items():
        material = unreal.load_asset(path)
        if not isinstance(material, unreal.Material):
            raise RuntimeError("OFAST_TRY materials: %s is not a material" % path)
        for prop, value in props.items():
            current = material.get_editor_property(prop)
            if isinstance(current, unreal.EnumBase) and isinstance(value, str):
                value = getattr(type(current), value)
            material.set_editor_property(prop, value)
        unreal.MaterialEditingLibrary.recompile_material(material)

    def apply(component, props):
        # "visible" and "material" are not editor properties: they go through set_visibility and set_material
        # ("material": an asset path put on every slot, e.g. to see where shadow proxies are).
        props = dict(props)
        if "visible" in props:
            component.set_visibility(bool(props.pop("visible")))
        if "material" in props:
            material = unreal.load_asset(props.pop("material"))
            if material is None:
                raise RuntimeError("OFAST_TRY material not found")
            for index in range(component.get_num_materials()):
                component.set_material(index, material)
        if props:
            component.set_editor_properties(props)

    # "meshes": properties set on the imported map meshes (not the sky sphere, proxies or other OFAST helpers);
    # "blockers": on CS2's light blockers only (labels containing lightblocker_); "proxies": on the shadow proxies.
    for actor in actors:
        label = actor.get_actor_label()
        if not isinstance(actor, unreal.StaticMeshActor) or label.startswith("OFAST_"):
            continue
        if label.startswith("shadowproxy_"):
            group = "proxies"
        elif "lightblocker_" in label:
            group = "blockers"
        else:
            group = "meshes"
        if settings.get(group):
            apply(actor.static_mesh_component, settings[group])
    if "sun_rotation" in settings:
        # [pitch, yaw, roll] in Unreal degrees, e.g. to see whether a lit patch moves like sunlight through a window.
        pitch, yaw, roll = settings["sun_rotation"]
        for actor in actors:
            if isinstance(actor, unreal.DirectionalLight):
                actor.set_actor_rotation(unreal.Rotator(roll=roll, pitch=pitch, yaw=yaw), False)
    if any(prop.endswith("*") for prop in settings.get("post", {})):
        raise RuntimeError('"name*" (multiply) only works on light properties, not "post"')
    for group, classes in groups.items():
        for actor in actors:
            if isinstance(actor, classes):
                for prop, value in settings.get(group, {}).items():
                    # "name*" multiplies the current (numeric) value instead of replacing it.
                    if prop.endswith("*"):
                        prop = prop[:-1]
                        current = actor.light_component.get_editor_property(prop)
                        if not isinstance(current, (int, float)):
                            raise RuntimeError('"%s*" needs a numeric property' % prop)
                        value = current * value
                    actor.light_component.set_editor_property(prop, value)
                if group == "sky_light":
                    actor.light_component.recapture_sky()
    for actor in actors:
        if isinstance(actor, unreal.PostProcessVolume) and settings.get("post"):
            post = actor.settings
            for prop, value in settings["post"].items():
                post.set_editor_property("override_" + prop, True)
                post.set_editor_property(prop, value)
            actor.set_editor_property("settings", post)
    if settings.get("script"):
        with open(settings["script"], encoding="utf-8") as f:
            code = f.read()
        exec(compile(code, settings["script"], "exec"), {"unreal": unreal, "actors": actors, "log": log})


def extra_cameras():
    """Name -> camera for the OFAST_VIEWS file, spawned in the open level (never saved). The same conversion as
    import_map.extra_views: setpos is the player's feet in inches (eye 64 higher), Source axes to Unreal's."""
    path = os.environ.get("OFAST_VIEWS")
    if not path:
        return {}
    with open(path) as f:
        listed = json.load(f)
    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    cameras = {}
    for view in listed:
        x, y, z = view["setpos"]
        pitch, yaw, roll = view["setang"]
        location = unreal.Vector(x * 2.54, -y * 2.54, (z + 64.0) * 2.54)
        camera = actors.spawn_actor_from_class(unreal.CameraActor, location,
                                               unreal.Rotator(roll=roll, pitch=-pitch, yaw=-yaw))
        component = camera.camera_component
        component.set_editor_property("field_of_view", HFOV_16_9)
        component.set_editor_property("aspect_ratio", 16 / 9)
        component.set_editor_property("constrain_aspect_ratio", True)
        cameras[view["name"]] = camera
    log("%d cameras from %s" % (len(cameras), path))
    return cameras


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    level = "/Game/OFAST/Maps/" + NAME
    if not unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).load_level(level):
        raise RuntimeError("could not open %s (run import_map.py first)" % level)
    apply_try(unreal.get_editor_subsystem(unreal.EditorActorSubsystem).get_all_level_actors())
    cameras = {
        actor.get_actor_label()[len(CAMERA_PREFIX):]: actor
        for actor in unreal.get_editor_subsystem(unreal.EditorActorSubsystem).get_all_level_actors()
        if isinstance(actor, unreal.CameraActor) and actor.get_actor_label().startswith(CAMERA_PREFIX)
    }
    cameras.update(extra_cameras())
    if not cameras:
        raise RuntimeError("no %s* cameras in %s (re-run import_map.py)" % (CAMERA_PREFIX, level))
    missing = ONLY - set(cameras)
    if missing:
        raise RuntimeError("no camera for %s; the level has %s" % (", ".join(sorted(missing)), ", ".join(sorted(cameras))))
    shots = sorted((name, camera) for name, camera in cameras.items() if not ONLY or name in ONLY)
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
            return
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
        state["path"] = os.path.join(SHOT_DIR, "%s_%s.png" % (NAME, name))
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
