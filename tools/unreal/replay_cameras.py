"""A round's cameras in its Level Sequence (M7, decision 021), from its cameras file (`<demo>_r<round>.cameras.json`,
written by `export::cameras`; format: docs/formats/cameras.md). Shared by import_demo.py (a round's first import)
and import_cameras.py (its cameras changed later, in seconds).

- Each player's eyes: their first-person camera ("POV <name>", made by import_demo.py) keyed on the file's eye
  positions and view angles (recoil and any smoothing are in them), linearly, as every player track.
- The shot and HLAE cameras: a CineCameraActor each (tagged OFAST_camera), keyed on every key of the file with cubic
  (auto) keys, so the path stays smooth between ticks in slow motion; its focal length keyed from the field of view
  on a 16:9 sensor (36 x 20.25 mm), depth of field off.
- The camera cut track from the file's cuts.
- The muzzle lights' visibility. Unreal cannot hide a light in one camera only, so a shooter's third-person muzzle
  light shows outside the cuts through their eyes and the one on their viewmodel inside them; they are keyed again
  whenever the cameras change.

What a player's own view must not show is hidden by owner instead, whatever camera a cut uses: import_demo.py makes
each POV camera the owner of that player's body and effects (bOwnerNoSee) and of their viewmodel (bOnlyOwnerSee).

Also the helpers both scripts key with (to_unreal, unwrap, reduce_keys, channels_by_name, key_visibility,
spawn_tagged).
"""

import json
import math
import os

import unreal

TAG = unreal.Name("OFAST_replay")
CAMERA_TAG = unreal.Name("OFAST_camera")
CAMERAS_FORMAT = "ofast-cameras"
CAMERAS_VERSION = 1
UNITS_TO_CM = 2.54
# Key reduction tolerances: 0.2 cm and 0.1 degree.
POSITION_TOLERANCE_CM = 0.2
ANGLE_TOLERANCE_DEG = 0.1
# The shot cameras' sensor: 16:9.
SENSOR_MM = (36.0, 20.25)
# Muzzle lights' tags: "OFAST_light_tp_<controller>" on the third-person light actor, "OFAST_light_fp_<controller>"
# on the light component of a viewmodel.
LIGHT_TAG = "OFAST_light_%s_%d"

actors = unreal.get_editor_subsystem(unreal.EditorActorSubsystem)


def log(message):
    unreal.log("OFAST: " + message)


def to_unreal(position):
    x, y, z = position
    return (x * UNITS_TO_CM, -y * UNITS_TO_CM, z * UNITS_TO_CM)


def unwrap(angles):
    """Degrees made continuous: each moved by whole turns to within 180 of the one before."""
    out = []
    for a in angles:
        if out:
            a = out[-1] + ((a - out[-1] + 180.0) % 360.0 - 180.0)
        out.append(a)
    return out


def reduce_keys(frames, values, tolerance):
    """Drops keys that linear interpolation between the kept neighbours reproduces within `tolerance`. Keeps the
    first and last key of every run (values are lists of floats of equal length)."""
    if len(frames) <= 2:
        return list(zip(frames, values))
    kept = [0]
    for i in range(1, len(frames) - 1):
        a, b = kept[-1], i + 1
        t = (frames[i] - frames[a]) / (frames[b] - frames[a])
        if any(abs(values[a][k] + (values[b][k] - values[a][k]) * t - values[i][k]) > tolerance
               for k in range(len(values[i]))):
            kept.append(i)
    kept.append(len(frames) - 1)
    return [(frames[i], values[i]) for i in kept]


def channels_by_name(section):
    return {str(c.channel_name): c for c in section.get_all_channels()}


def key_visibility(binding, changes, end_frame):
    """A visibility track from (frame, visible) pairs: one key per change."""
    track = binding.add_track(unreal.MovieSceneVisibilityTrack)
    track.set_property_name_and_path("bHiddenInGame", "bHiddenInGame")
    section = track.add_section()
    section.set_range(0, end_frame)
    channel = section.get_all_channels()[0]
    previous = None
    for frame, value in changes:
        if value != previous:
            channel.add_key(unreal.FrameNumber(frame), value)
            previous = value


def spawn_tagged(cls_or_mesh, label, location=(0.0, 0.0, 0.0)):
    if isinstance(cls_or_mesh, unreal.StaticMesh):
        actor = actors.spawn_actor_from_object(cls_or_mesh, unreal.Vector(*location))
    else:
        actor = actors.spawn_actor_from_class(cls_or_mesh, unreal.Vector(*location))
    actor.tags = [TAG]
    actor.set_actor_label(label)
    return actor


def hfov_16_9(fov):
    """CS2's field of view (horizontal at 4:3) as a 16:9 picture's horizontal one (90 -> 106.26)."""
    return math.degrees(2 * math.atan(math.tan(math.radians(fov / 2)) * (16 / 9) / (4 / 3)))


def cs2_fov(hfov):
    """hfov_16_9 the other way: a 16:9 picture's horizontal field of view as CS2 counts it (horizontal at 4:3)."""
    return math.degrees(2 * math.atan(math.tan(math.radians(hfov / 2)) / ((16 / 9) / (4 / 3))))


def focal_length(fov):
    """The focal length (mm) giving CS2's field of view `fov` across the 16:9 sensor."""
    return SENSOR_MM[0] / 2 / math.tan(math.radians(hfov_16_9(fov) / 2))


def cameras_path(replay_path):
    """The cameras file next to a replay file."""
    base = os.path.basename(replay_path)
    if base.endswith(".ofast-replay.json"):
        base = base[:-len(".ofast-replay.json")]
    return os.path.join(os.path.dirname(replay_path), base + ".cameras.json")


def load(path):
    with open(path, encoding="utf-8") as f:
        cameras = json.load(f)
    if cameras.get("format") != CAMERAS_FORMAT or cameras.get("version") != CAMERAS_VERSION:
        raise RuntimeError("%s is not an OFAST cameras file (version %d)" % (path, CAMERAS_VERSION))
    return cameras


def own_views(cameras):
    """controller -> the (start, end) frames of the cuts through that player's eyes, cuts that follow each other
    merged (two kills in a row by one player are one stretch)."""
    tick0 = cameras["start_tick"]
    out = {}
    for cut in sorted(cameras["cuts"], key=lambda c: c["start"]):
        if "eyes" not in cut:
            continue
        ranges = out.setdefault(cut["eyes"], [])
        start, end = cut["start"] - tick0, cut["end"] - tick0
        if ranges and ranges[-1][1] >= start:
            ranges[-1] = (ranges[-1][0], max(ranges[-1][1], end))
        else:
            ranges.append((start, end))
    return out


def add_key(channel, frame, value, interpolation):
    """A key at a frame that may fall between frames (a sub-frame key)."""
    whole = math.floor(frame)
    channel.add_key(unreal.FrameNumber(int(whole)), value, frame - whole, unreal.MovieSceneTimeUnit.DISPLAY_RATE,
                    interpolation)


def transform_section(binding, end_frame):
    track = binding.add_track(unreal.MovieScene3DTransformTrack)
    section = track.add_section()
    section.set_range(0, end_frame)
    channels = channels_by_name(section)
    for axis in ("X", "Y", "Z"):
        channels["Scale." + axis].set_default(1.0)
    return channels


def key_eyes(binding, rows, tick0, end_frame):
    """A POV camera's transform from the file's `eyes` rows [tick, x, y, z, pitch, yaw]: linear keys, after dropping
    those a straight line already gives. Returns the number of keys."""
    if not rows:
        return 0
    channels = transform_section(binding, end_frame)
    channels["Rotation.X"].set_default(0.0)
    linear = unreal.MovieSceneKeyInterpolation.LINEAR
    frames = [r[0] - tick0 for r in rows]
    locations = [to_unreal(r[1:4]) for r in rows]
    rotations = list(zip([-r[4] for r in rows], unwrap([-r[5] for r in rows])))
    count = 0
    for name, values, tolerance, axes in (("Location", locations, POSITION_TOLERANCE_CM, ("X", "Y", "Z")),
                                          ("Rotation", rotations, ANGLE_TOLERANCE_DEG, ("Y", "Z"))):
        for frame, value in reduce_keys(frames, values, tolerance):
            for axis, v in zip(axes, value):
                add_key(channels["%s.%s" % (name, axis)], frame, v, linear)
                count += 1
    return count


def spawn_shot_camera(camera):
    """A CineCameraActor for a camera of the file (its `name` as label), 16:9, depth of field off."""
    actor = spawn_tagged(unreal.CineCameraActor, camera["name"])
    actor.tags = [TAG, CAMERA_TAG]
    component = actor.get_cine_camera_component()
    filmback = component.get_editor_property("filmback")
    filmback.set_editor_property("sensor_width", SENSOR_MM[0])
    filmback.set_editor_property("sensor_height", SENSOR_MM[1])
    component.set_editor_property("filmback", filmback)
    focus = component.get_editor_property("focus_settings")
    focus.set_editor_property("focus_method", unreal.CameraFocusMethod.DISABLE)
    component.set_editor_property("focus_settings", focus)
    first = camera["keys"][0]
    component.set_editor_property("current_focal_length", focal_length(first[7]))
    return actor


def key_shot_camera(sequence, actor, camera, tick0, end_frame):
    """Keys a shot or HLAE camera from the file's rows [tick, x, y, z, pitch, yaw, roll, fov]: every key, cubic
    (auto tangents); the focal length where the field of view changes. Returns its binding."""
    binding = sequence.add_possessable(actor)
    channels = transform_section(binding, end_frame)
    cubic = unreal.MovieSceneKeyInterpolation.AUTO
    rows = camera["keys"]
    yaws = unwrap([-r[5] for r in rows])
    rolls = unwrap([r[6] for r in rows])
    for row, yaw, roll in zip(rows, yaws, rolls):
        frame = row[0] - tick0
        for axis, cm in zip(("X", "Y", "Z"), to_unreal(row[1:4])):
            add_key(channels["Location." + axis], frame, cm, cubic)
        add_key(channels["Rotation.X"], frame, roll, cubic)
        add_key(channels["Rotation.Y"], frame, -row[4], cubic)
        add_key(channels["Rotation.Z"], frame, yaw, cubic)
    component = sequence.add_possessable(actor.get_cine_camera_component())
    track = component.add_track(unreal.MovieSceneFloatTrack)
    track.set_property_name_and_path("CurrentFocalLength", "CurrentFocalLength")
    section = track.add_section()
    section.set_range(0, end_frame)
    channel = section.get_all_channels()[0]
    frames = [r[0] - tick0 for r in rows]
    for frame, (focal,) in reduce_keys(frames, [(focal_length(r[7]),) for r in rows], 0.01):
        add_key(channel, frame, focal, cubic)
    return binding


def add_cuts(sequence, cameras, eyes, shots):
    """The camera cut track of the file's cuts. `eyes`: controller -> binding of the player's POV camera; `shots`:
    the shot cameras' bindings, in the file's order. Returns the number of cuts."""
    tick0 = cameras["start_tick"]
    track = sequence.add_track(unreal.MovieSceneCameraCutTrack)
    count = 0
    for cut in cameras["cuts"]:
        if "eyes" in cut:
            binding = eyes.get(cut["eyes"])
        else:
            binding = shots[cut["camera"]] if cut["camera"] < len(shots) else None
        if binding is None:
            continue
        section = track.add_section()
        section.set_range(cut["start"] - tick0, cut["end"] - tick0)
        section.set_camera_binding_id(sequence.get_binding_id(binding))
        count += 1
    return count


def light_changes(own_view, first_person):
    """(frame, visible) of a muzzle light: the one on the viewmodel (`first_person`) inside the cuts through its
    shooter's eyes (own_views: merged, so no two changes fall on one frame), the third-person one outside them."""
    changes = [] if own_view and own_view[0][0] == 0 else [(0, not first_person)]
    for start, end in own_view:
        changes.append((start, first_person))
        changes.append((end, not first_person))
    return changes


def light_tag(controller, first_person):
    return unreal.Name(LIGHT_TAG % ("fp" if first_person else "tp", controller))


def tag_light(owner, controller, first_person, component=False):
    """Tags a muzzle light (its actor, or its `component` on a viewmodel) so a camera change finds it again
    (import_cameras.py)."""
    tag = light_tag(controller, first_person)
    if component:
        owner.set_editor_property("component_tags", list(owner.get_editor_property("component_tags")) + [tag])
    else:
        owner.tags = list(owner.tags) + [tag]
