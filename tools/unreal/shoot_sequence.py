"""Screenshots of a round already built, through the camera its cuts use at given ticks: to check a round as saved
(owner visibility, cameras changed by import_cameras.py), without building it again.

Run with tools/unreal/run_ue.ps1:

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/shoot_sequence.py \
        -Sequence /Game/OFAST/Replays/<demo>_r17 -Ticks 153200,154050 -Shot test-data/shots

Inputs (environment variables):
    OFAST_SEQUENCE   the round's sequence (import_demo.py stores its demo clock and map on it)
    OFAST_TICKS      demo ticks, "153200,154050"
    OFAST_POV_SHOTS  optional views through players' eyes, "tick:name,tick:name" (their "POV <name>" cameras, as
                     CS2's spec_player shows them: -PovShots "150500:XANTARES")
    OFAST_SHOT       the folder for the screenshots: <sequence>_tick<tick>_<camera label>.png
"""

import os
import re

import unreal

WIDTH, HEIGHT = 1920, 1080
LEVEL_TICKS = 300  # editor ticks between loading the level and opening the sequence (opening both at once crashed)
SHOT_DELAY_S = 6.0
SHOT_TIMEOUT_TICKS = 3000

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def log(message):
    unreal.log("OFAST: " + message)


def cut_camera(sequence, frame):
    """The label of the camera the cut track uses at `frame`, or None."""
    for track in sequence.find_tracks_by_type(unreal.MovieSceneCameraCutTrack):
        for section in track.get_sections():
            if section.get_start_frame() <= frame < section.get_end_frame():
                guid = unreal.GuidLibrary.conv_guid_to_string(
                    section.get_camera_binding_id().get_editor_property("guid"))
                for binding in sequence.get_bindings():
                    if unreal.GuidLibrary.conv_guid_to_string(binding.get_id()) == guid:
                        return str(binding.get_display_name())
    return None


def main():
    path = os.environ["OFAST_SEQUENCE"]
    folder = os.environ["OFAST_SHOT"]
    ticks = [int(t) for t in os.environ.get("OFAST_TICKS", "").split(",") if t.strip()]
    sequence = assets.load_asset(path)
    if sequence is None:
        raise RuntimeError("no sequence %s" % path)
    tick0 = int(assets.get_metadata_tag(sequence, "OFAST_start_tick") or 0)
    level = "/Game/OFAST/Maps/" + assets.get_metadata_tag(sequence, "OFAST_map")
    levels.load_level(level)
    by_label = {a.get_actor_label(): a for a in actors.get_all_level_actors()}
    name = path.rsplit("/", 1)[-1]
    shots = []
    for tick in ticks:
        label = cut_camera(sequence, tick - tick0)
        camera = by_label.get(label) if label else None
        if camera is None:
            log("tick %d: no camera cut (%s)" % (tick, label))
            continue
        safe = re.sub(r"[^A-Za-z0-9]+", "_", label)[:40]
        shots.append((tick - tick0, camera, os.path.join(folder, "%s_tick%d_%s.png" % (name, tick, safe))))
    for item in os.environ.get("OFAST_POV_SHOTS", "").split(","):
        if ":" not in item:
            continue
        tick, player = item.split(":", 1)
        label = "POV %s" % player.strip()
        camera = by_label.get(label)
        if camera is None:
            log("tick %s: no camera %s" % (tick, label))
            continue
        safe = re.sub(r"[^A-Za-z0-9]+", "_", label)[:40]
        shots.append((int(tick) - tick0, camera, os.path.join(folder, "%s_tick%s_%s.png" % (name, tick, safe))))
    state = {"ticks": 0, "index": 0, "requested": None}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] == LEVEL_TICKS:
            unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
        if state["ticks"] < 2 * LEVEL_TICKS:
            return
        if state["requested"] is not None:
            shot_path = shots[state["index"]][2]
            if not os.path.exists(shot_path) and state["ticks"] - state["requested"] < SHOT_TIMEOUT_TICKS:
                return
            log("screenshot %s: %s" % ("written" if os.path.exists(shot_path) else "FAILED (none written)",
                                       shot_path))
            state["index"] += 1
            state["requested"] = None
        if state["index"] >= len(shots):
            unreal.unregister_slate_post_tick_callback(handle)
            unreal.LevelSequenceEditorBlueprintLibrary.close_level_sequence()
            unreal.SystemLibrary.quit_editor()
            return
        frame, camera, shot_path = shots[state["index"]]
        if os.path.exists(shot_path):
            os.remove(shot_path)
        unreal.LevelSequenceEditorBlueprintLibrary.set_current_time(frame)
        unreal.LevelSequenceEditorBlueprintLibrary.force_update()
        unreal.AutomationLibrary.take_high_res_screenshot(WIDTH, HEIGHT, shot_path, camera=camera,
                                                          delay=SHOT_DELAY_S)
        state["requested"] = state["ticks"]

    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
