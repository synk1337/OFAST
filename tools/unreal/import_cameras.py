"""Change the cameras of a round already built (M7, decision 021), in seconds instead of a full import.

Run with tools/unreal/run_ue.ps1 (the Studio's "Update cameras" does it for you):

    powershell -File tools/unreal/run_ue.ps1 -Script tools/unreal/import_cameras.py \
        -Cameras test-data/exports/demos/<demo>_r2.cameras.json

Inputs (environment variables):
    OFAST_CAMERAS    the round's cameras file, written by `ofast export-demo` (format: docs/formats/cameras.md)

The round must have been built by import_demo.py: its sequence /Game/OFAST/Replays/<demo>_r<round> in its map's
level /Game/OFAST/Maps/<map>. What changes (replay_cameras.py makes them as import_demo.py does):

- each player's first-person camera ("POV <name>") keyed again on the file's eyes (a new smoothing);
- the shot and HLAE cameras of earlier runs (actors tagged OFAST_camera) removed with their bindings, and the file's
  made;
- the camera cut track replaced by the file's cuts (cuts made by hand go too; cameras added by hand stay, ready to
  cut to again);
- the muzzle lights' visibility keyed again for the new cuts.

Nothing else is touched: the players, their animation, the effects. Their visibility needs no change, as each
player's own camera hides what it must by owner.
"""

import os
import sys

import unreal

sys.path.insert(0, os.path.dirname(os.path.abspath(__file__)))
import replay_cameras  # noqa: E402
from replay_cameras import CAMERA_TAG, key_visibility, log  # noqa: E402

CAMERAS_PATH = os.environ["OFAST_CAMERAS"]

levels = unreal.get_editor_subsystem(unreal.LevelEditorSubsystem)
actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)
assets = unreal.EditorAssetLibrary


def remove_tracks(binding, cls):
    for track in binding.find_tracks_by_type(cls):
        binding.remove_track(track)


def light_bindings(bindings, controller):
    """(binding, first_person) of a shooter's muzzle lights, found by their tags (replay_cameras.tag_light): the
    third-person light's actor, and the light component on their viewmodel (a binding under the viewmodel's)."""
    out = []
    third = replay_cameras.light_tag(controller, False)
    first = replay_cameras.light_tag(controller, True)
    for actor in actors.get_all_level_actors():
        label = actor.get_actor_label()
        if replay_cameras.TAG not in actor.tags or label not in bindings:
            continue
        if third in actor.tags:
            out.append((bindings[label], False))
        for component in actor.get_components_by_class(unreal.LightComponent):
            if first not in component.get_editor_property("component_tags"):
                continue
            name = str(component.get_name())
            child = next((c for c in bindings[label].get_child_possessables() if str(c.get_display_name()) == name),
                         None)
            if child is None:
                log("no binding for the light %s on %s: its visibility is left as it was" % (name, label))
            else:
                out.append((child, True))
    return out


def update(cameras):
    level = "/Game/OFAST/Maps/" + cameras["map"]
    path = "/Game/OFAST/Replays/%s_r%d" % (cameras["demo"], cameras["round"])
    if not assets.does_asset_exist(level) or not assets.does_asset_exist(path):
        raise RuntimeError("%s is not built yet (send the round to Unreal first)" % path)
    sequence = assets.load_asset(path)
    # A round built before owner visibility (decision 021) hides its players by keys that follow its old cuts: other
    # cameras would show it wrong.
    if not assets.get_metadata_tag(sequence, "OFAST_start_tick"):
        raise RuntimeError("%s was built by an older OFAST: send the round to Unreal again" % path)
    levels.load_level(level)
    tick0 = cameras["start_tick"]
    end_frame = sequence.get_playback_end()
    bindings = {str(b.get_display_name()): b for b in sequence.get_bindings()}
    names = {int(c): n for c, n in cameras["players"].items()}

    # The players' eyes.
    eyes = {}
    keys = 0
    for controller, name in names.items():
        binding = bindings.get("POV " + name)
        if binding is None:
            log("no POV camera for %s in %s" % (name, path))
            continue
        remove_tracks(binding, unreal.MovieScene3DTransformTrack)
        keys += replay_cameras.key_eyes(binding, cameras["eyes"].get(str(controller), []), tick0, end_frame)
        eyes[controller] = binding

    # The shot cameras: the old ones out (their bindings, the component bindings under them, the actors), the new in.
    old = [a for a in actors.get_all_level_actors() if CAMERA_TAG in a.tags]
    old_labels = {a.get_actor_label() for a in old}
    removed = [b for b in sequence.get_bindings() if str(b.get_display_name()) in old_labels]
    for binding in removed:
        for child in binding.get_child_possessables():
            child.remove()
        binding.remove()
    for actor in old:
        actors.destroy_actor(actor)
    shots = []
    for camera in cameras["cameras"]:
        actor = replay_cameras.spawn_shot_camera(camera)
        shots.append(replay_cameras.key_shot_camera(sequence, actor, camera, tick0, end_frame))

    # The cuts.
    for track in sequence.find_tracks_by_type(unreal.MovieSceneCameraCutTrack):
        sequence.remove_track(track)
    cuts = replay_cameras.add_cuts(sequence, cameras, eyes, shots)

    # The muzzle lights: the third-person one outside the cuts through its shooter's eyes, the viewmodel's inside.
    own_views = replay_cameras.own_views(cameras)
    lights = 0
    for controller in names:
        for binding, first_person in light_bindings(bindings, controller):
            remove_tracks(binding, unreal.MovieSceneVisibilityTrack)
            key_visibility(binding, replay_cameras.light_changes(own_views.get(controller, []), first_person),
                           end_frame)
            lights += 1

    assets.save_loaded_asset(sequence)
    levels.save_current_level()
    log("cameras of %s: %d eyes (%d keys), %d shot cameras (%d removed), %d cuts, %d muzzle lights"
        % (path, len(eyes), keys, len(shots), len(old), cuts, lights))
    # The Studio takes this line as the sign it worked.
    log("cameras updated %s" % sequence.get_path_name())


def main():
    cameras = replay_cameras.load(CAMERAS_PATH)
    update(cameras)


try:
    main()
except Exception as error:
    log("FAILED: %s" % error)
unreal.SystemLibrary.quit_editor()
