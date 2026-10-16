"""Re-shoot the views of an already imported map without importing it again (about a minute instead of three).

Run with tools/unreal/run_ue.ps1 (-Script tools/unreal/shoot_views.py -Name <map> -Shot <folder>).

Inputs (environment variables):
    OFAST_NAME   map name: opens /Game/OFAST/Maps/<name>, made by import_map.py
    OFAST_SHOT   folder for the screenshots: <map>_<view>.png
    OFAST_ONLY   optional comma list of view names (ct0, t0, top, or names from the -Views file of the import)

It uses the cameras import_map.py saved with the level (labels OFAST_cam_<view>), so the viewpoints are exactly
the import's. The level is not changed or saved; its lighting is whatever the last import saved (including a
diagnostic -Disable run).
"""

import os

import unreal

NAME = os.environ["OFAST_NAME"]
SHOT_DIR = os.environ["OFAST_SHOT"]
ONLY = {s.strip() for s in os.environ.get("OFAST_ONLY", "").split(",") if s.strip()}
WIDTH, HEIGHT = 1920, 1080
CAMERA_PREFIX = "OFAST_cam_"  # import_map.CAMERA_PREFIX
# Frames to wait after opening the level, so meshes and textures have streamed in before the first screenshot.
WARM_UP_TICKS = 600


def log(message):
    unreal.log("OFAST: " + message)


def main():
    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    level = "/Game/OFAST/Maps/" + NAME
    if not unreal.get_editor_subsystem(unreal.LevelEditorSubsystem).load_level(level):
        raise RuntimeError("could not open %s (run import_map.py first)" % level)
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
