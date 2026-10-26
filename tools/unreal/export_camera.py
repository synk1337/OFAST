"""Write a camera made in Unreal to a cameras file (M7), for HLAE: `ofast camera-to-hlae` turns it into a
`mirv_campath` path or a `mirv_camio` recording to play in CS2.

Run with tools/unreal/run_ue.ps1:

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/export_camera.py \
        -Sequence /Game/OFAST/Replays/<demo>_r17 -Name "CineCameraActor" -Cameras my_camera.cameras.json
    ofast camera-to-hlae my_camera.cameras.json --out my_camera.xml

Inputs (environment variables):
    OFAST_SEQUENCE   the round's sequence (import_demo.py made it, and stored the demo's clock on it)
    OFAST_NAME       the camera: its actor's label in the map's level (its name in Sequencer)
    OFAST_CAMERAS    the cameras file to write (format: docs/formats/cameras.md; only `cameras`)

The camera is read as Sequencer plays it: the sequence set to each frame (one frame per tick) from the camera's
first key to its last (the whole sequence for a camera without keys), and the camera's place, rotation and
horizontal field of view read in the world, so a camera attached to something or aimed by look-at tracking comes
out as it shows.
"""

import json
import math
import os
import sys

import unreal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import replay_cameras  # noqa: E402
from replay_cameras import CAMERAS_FORMAT, CAMERAS_VERSION, UNITS_TO_CM, channels_by_name, log  # noqa: E402

LEVEL_TICKS = 300  # editor ticks between loading the level and opening the sequence (opening both at once crashed)

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def keyed_frames(sequence, label):
    """The first and last frames the camera's transform track keys, or the sequence's playback range."""
    binding = next((b for b in sequence.get_bindings() if str(b.get_display_name()) == label), None)
    frames = []
    for track in binding.find_tracks_by_type(unreal.MovieScene3DTransformTrack) if binding else []:
        for section in track.get_sections():
            for channel in channels_by_name(section).values():
                for key in channel.get_keys():
                    time = key.get_time()
                    frames.append(time.frame_number.value + time.sub_frame)
    if not frames:
        return sequence.get_playback_start(), sequence.get_playback_end() - 1
    return int(math.floor(min(frames))), int(math.ceil(max(frames)))


def horizontal_fov(camera):
    if isinstance(camera, unreal.CineCameraActor):
        return camera.get_cine_camera_component().get_horizontal_field_of_view()
    return camera.camera_component.get_editor_property("field_of_view")


def main():
    path = os.environ["OFAST_SEQUENCE"]
    label = os.environ["OFAST_NAME"]
    out = os.environ["OFAST_CAMERAS"]
    sequence = assets.load_asset(path)
    if sequence is None:
        raise RuntimeError("no sequence %s" % path)
    tag = lambda name: assets.get_metadata_tag(sequence, name)  # noqa: E731
    if not tag("OFAST_start_tick"):
        raise RuntimeError("%s has no demo clock: send the round to Unreal again (import_demo.py stores it)" % path)
    tick0 = int(tag("OFAST_start_tick"))
    levels.load_level("/Game/OFAST/Maps/" + tag("OFAST_map"))
    camera = next((a for a in actors.get_all_level_actors() if a.get_actor_label() == label), None)
    if not isinstance(camera, unreal.CameraActor):
        raise RuntimeError("no camera %r in the level" % label)
    first, last = keyed_frames(sequence, label)
    state = {"ticks": 0}

    def tick(_delta):
        state["ticks"] += 1
        if state["ticks"] == LEVEL_TICKS:
            unreal.LevelSequenceEditorBlueprintLibrary.open_level_sequence(sequence)
        if state["ticks"] < 2 * LEVEL_TICKS:
            return
        unreal.unregister_slate_post_tick_callback(handle)
        keys = []
        for frame in range(first, last + 1):
            unreal.LevelSequenceEditorBlueprintLibrary.set_current_time(frame)
            unreal.LevelSequenceEditorBlueprintLibrary.force_update()
            x, y, z = (v / UNITS_TO_CM for v in camera.get_actor_location().to_tuple())
            rotation = camera.get_actor_rotation()
            keys.append([tick0 + frame, x, -y, z, -rotation.pitch, -rotation.yaw, rotation.roll,
                         replay_cameras.cs2_fov(horizontal_fov(camera))])
        cameras = {
            "format": CAMERAS_FORMAT,
            "version": CAMERAS_VERSION,
            "demo": tag("OFAST_demo"),
            "map": tag("OFAST_map"),
            "round": int(tag("OFAST_round") or 0),
            "start_tick": tick0,
            "tick_interval": float(tag("OFAST_tick_interval")),
            "server_tick_offset": int(tag("OFAST_server_tick_offset")) if tag("OFAST_server_tick_offset") else None,
            "cameras": [{"name": label, "kind": "unreal", "subject": None, "keys": keys}],
        }
        with open(out, "w", encoding="utf-8") as f:
            json.dump(cameras, f)
        log("wrote %s: %s, %d keys (ticks %d - %d)" % (out, label, len(keys), tick0 + first, tick0 + last))
        unreal.LevelSequenceEditorBlueprintLibrary.close_level_sequence()
        unreal.SystemLibrary.quit_editor()

    unreal.EditorPythonScripting.set_keep_python_script_alive(True)
    handle = unreal.register_slate_post_tick_callback(tick)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
    unreal.SystemLibrary.quit_editor()
