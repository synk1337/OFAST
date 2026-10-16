"""Re-shoot views of an already imported map without importing it again (about a minute instead of three).

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/shoot_views.py -Name <map> -Shot <folder> -Views <views.json>).

Inputs (environment variables):
    OFAST_NAME   map name: opens /Game/OFAST/Maps/<name>, made by import_map.py
    OFAST_SHOT   folder for the screenshots: <map>_<view>.png
    OFAST_VIEWS  JSON list of {"name", "setpos", "setang"} with CS2 console values (see import_map.extra_views)

The level is never saved: cameras are added for the screenshots only. The lighting is whatever the last
import_map.py run saved (including a diagnostic OFAST_DISABLE run).
"""

import json
import math
import os

import unreal

NAME = os.environ["OFAST_NAME"]
SHOT_DIR = os.environ["OFAST_SHOT"]
WIDTH, HEIGHT = 1920, 1080
# CS2's default fov is 90° measured horizontally on a 4:3 screen; on 16:9 that is 2·atan(tan(45°)·(16/9)/(4/3)).
HFOV_16_9 = math.degrees(2 * math.atan(math.tan(math.radians(45)) * (16 / 9) / (4 / 3)))
# Frames to wait after opening the level, so meshes and textures have streamed in before the first screenshot.
WARM_UP_TICKS = 600


def log(message):
    unreal.log("OFAST: " + message)


def camera_at(view):
    """A camera at a CS2 setpos/setang (setpos is the feet; the eye is 64 units higher)."""
    x, y, z = view["setpos"]
    pitch, yaw, roll = view["setang"]
    location = unreal.Vector(x * 2.54, -y * 2.54, (z + 64.0) * 2.54)
    actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
    camera = actors.spawn_actor_from_class(
        unreal.CameraActor, location, unreal.Rotator(roll=roll, pitch=-pitch, yaw=-yaw)
    )
    component = camera.camera_component
    component.set_editor_property("field_of_view", HFOV_16_9)
    component.set_editor_property("aspect_ratio", 16 / 9)
    component.set_editor_property("constrain_aspect_ratio", True)
    return camera


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).load_level("/Game/OFAST/Maps/" + NAME)
    with open(os.environ["OFAST_VIEWS"]) as f:
        shots = [(view["name"], camera_at(view)) for view in json.load(f)]
    unreal.SystemLibrary.execute_console_command(None, "r.HighResScreenshotDelay 16")
    state = {"ticks": 0, "index": 0, "task": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] > 36000:  # ~10 minutes at 60 fps: give up rather than hang
            log("FAILED: screenshots did not finish")
            finish()
            return
        if state["ticks"] < WARM_UP_TICKS:
            return
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
