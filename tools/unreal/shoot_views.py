"""Re-shoot the views of an already imported map without importing it again (about a minute instead of three).

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/shoot_views.py -Name <map> -Shot <folder>).

Inputs (environment variables):
    OFAST_NAME   level name: opens /Game/OFAST/Maps/<name>, made by import_map.py (<map>, or <map>_realistic for
                 the realistic style)
    OFAST_SHOT   folder for the screenshots: <map>_<view>.png
    OFAST_ONLY   optional comma list of view names (ct0, t0, top, or names from the -Views file of the import)
    OFAST_TRY    optional JSON (or a .json file path) of what-if settings applied before shooting (never saved), to
                 try changes quickly:
                   "sun", "sky_light", "local": light component properties, e.g. {"indirect_lighting_intensity": 2}
                                                ("name*": multiply the current value)
                   "sun_rotation": [pitch, yaw, roll] of the sun (Unreal degrees)
                   "meshes": properties of the imported map meshes' components (not the sky sphere or proxies)
                   "post": PostProcessSettings fields (set with their override flag)

It uses the cameras import_map.py saved with the level (labels OFAST_cam_<view>), so the viewpoints are exactly
the import's. The level is not changed or saved; its lighting is whatever the last import saved (including a
diagnostic -Disable run).
"""

import json
import os
import time

import unreal

NAME = os.environ["OFAST_NAME"]
SHOT_DIR = os.environ["OFAST_SHOT"]
ONLY = {s.strip() for s in os.environ.get("OFAST_ONLY", "").split(",") if s.strip()}
WIDTH, HEIGHT = 1920, 1080
CAMERA_PREFIX = "OFAST_cam_"  # import_map.CAMERA_PREFIX
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
    unknown = set(settings) - set(groups) - {"post", "sun_rotation", "meshes"}
    if unknown:
        raise RuntimeError(
            "unknown OFAST_TRY group: %s (sun, sun_rotation, sky_light, local, meshes, post)" % ", ".join(sorted(unknown))
        )
    # "meshes": properties set on the imported map meshes (not the sky sphere, proxies or other OFAST helpers).
    for actor in actors:
        label = actor.get_actor_label()
        if (
            isinstance(actor, unreal.StaticMeshActor)
            and settings.get("meshes")
            and not label.startswith(("OFAST_", "shadowproxy_"))
        ):
            actor.static_mesh_component.set_editor_properties(settings["meshes"])
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
